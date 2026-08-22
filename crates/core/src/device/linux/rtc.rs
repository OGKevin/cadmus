//! Linux ioctl RTC implementation.

use anyhow::Error;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use nix::fcntl::{FcntlArg, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::{ioctl_none, ioctl_read, ioctl_write_ptr};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::mem;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::device::rtc::{Rtc, RtcTime, RtcWkalrm};

ioctl_read!(rtc_read_alarm, b'p', 0x10, RtcWkalrm);
ioctl_write_ptr!(rtc_write_alarm, b'p', 0x0f, RtcWkalrm);
ioctl_none!(rtc_disable_alarm, b'p', 0x02);
ioctl_read!(rtc_read_time, b'p', 0x09, RtcTime);
ioctl_write_ptr!(rtc_set_time, b'p', 0x0a, RtcTime);

/// Hardware RTC accessed through the Linux kernel RTC character device.
///
/// Opens a device path such as `/dev/rtc0` or `/dev/rtc` and drives it with
/// `RTC_RD_TIME`, `RTC_SET_TIME`, `RTC_ALM_READ`, `RTC_ALM_SET`, and
/// `RTC_AIE_OFF` ioctls (wrapped by the `rtc_*` helpers above). Concurrent
/// callers are serialized through an internal mutex guarding the open file
/// descriptor. Alarm IRQ waits use a separate `dup` of that fd so a blocking
/// `poll` does not hold the ioctl mutex.
///
/// # Examples
///
/// ```no_run
/// # use cadmus_core::device::LinuxRtc;
/// # use cadmus_core::device::rtc::Rtc;
/// let rtc = LinuxRtc::new("/dev/rtc0")?;
/// let now = rtc.read_time()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct LinuxRtc {
    file: Arc<Mutex<Option<File>>>,
    wait_file: Arc<Mutex<Option<File>>>,
    released: Arc<AtomicBool>,
    /// Cached RTC↔civil relationship and last `set_time` step.
    clock: Arc<Mutex<LinuxRtcClock>>,
}

/// Cached conversion state between the hardware RTC and the civil system clock.
#[derive(Debug)]
struct LinuxRtcClock {
    /// `RTC_now − system_now` from the last drift refresh (`new` / `set_time`).
    ///
    /// Positive means the RTC reads ahead of civil time. Used by [`Rtc::to_rtc`]
    /// / [`Rtc::to_civil`].
    drift: ChronoDuration,
    /// `new_time − old_time` from the latest [`Rtc::set_time`], if not yet taken.
    ///
    /// [`AlarmManager::sync`] consumes this via [`Rtc::take_pending_step`] to
    /// rebase relative and RTC-absolute alarms after a clock write.
    pending_step: Option<ChronoDuration>,
}

impl LinuxRtc {
    /// Opens the RTC device and creates a new interface handle.
    ///
    /// Opens the ioctl fd and a close-on-exec duplicate (`F_DUPFD_CLOEXEC`) used
    /// for IRQ `poll`/`read`.
    ///
    /// # Safety
    ///
    /// `F_DUPFD_CLOEXEC` returns a new fd number. That fd is wrapped in [`File`]
    /// via [`File::from_raw_fd`]; ownership transfers to this struct and it is
    /// closed on [`Self::release`] or drop.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the RTC device file (typically `/dev/rtc0` or `/dev/rtc`)
    ///
    /// # Returns
    ///
    /// A new [`LinuxRtc`] handle on success, or an error if the device cannot be opened.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cadmus_core::device::LinuxRtc;
    /// let rtc = LinuxRtc::new("/dev/rtc0")?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new<P: AsRef<Path>>(path: P) -> Result<LinuxRtc, Error> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let wait_fd = fcntl(&file, FcntlArg::F_DUPFD_CLOEXEC(0))?;
        let wait_file = unsafe { File::from_raw_fd(wait_fd) };
        let rtc = LinuxRtc {
            file: Arc::new(Mutex::new(Some(file))),
            wait_file: Arc::new(Mutex::new(Some(wait_file))),
            released: Arc::new(AtomicBool::new(false)),
            clock: Arc::new(Mutex::new(LinuxRtcClock {
                drift: ChronoDuration::zero(),
                pending_step: None,
            })),
        };
        rtc.refresh_drift_from_hardware()?;
        Ok(rtc)
    }

    /// Closes ioctl and IRQ wait file descriptors so another process can open the RTC.
    ///
    /// Idempotent. After release, RTC operations return an error until the handle
    /// is dropped.
    pub fn release(&self) -> Result<(), Error> {
        if self.released.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        {
            let mut file = self
                .file
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *file = None;
        }
        {
            let mut wait_file = self
                .wait_file
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *wait_file = None;
        }
        Ok(())
    }

    fn ioctl_file(&self) -> Result<std::sync::MutexGuard<'_, Option<File>>, Error> {
        self.file
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))
    }

    fn wait_file(&self) -> Result<std::sync::MutexGuard<'_, Option<File>>, Error> {
        self.wait_file
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))
    }

    fn require_file(file: &Option<File>) -> Result<&File, Error> {
        file.as_ref()
            .ok_or_else(|| anyhow::anyhow!("RTC device released"))
    }

    fn require_file_mut(file: &mut Option<File>) -> Result<&mut File, Error> {
        file.as_mut()
            .ok_or_else(|| anyhow::anyhow!("RTC device released"))
    }

    fn refresh_drift_from_hardware(&self) -> Result<(), Error> {
        match self.read_time() {
            Ok(rtc_now) => {
                self.clock
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
                    .drift = rtc_now.signed_duration_since(Utc::now());
                Ok(())
            }
            Err(_) if cfg!(test) => Ok(()),
            Err(err) => Err(err),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }
}

impl Rtc for LinuxRtc {
    /// Issues `RTC_ALM_READ` on the open device file.
    fn alarm(&self) -> Result<RtcWkalrm, Error> {
        let mut rwa = RtcWkalrm::default();
        let file = self.ioctl_file()?;
        let file = Self::require_file(&file)?;
        unsafe {
            rtc_read_alarm(file.as_raw_fd(), &mut rwa)
                .map(|_| rwa)
                .map_err(|e| e.into())
        }
    }

    /// Issues `RTC_ALM_SET` with the alarm enabled and `pending` cleared.
    fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let rwa = RtcWkalrm::for_wake_time(wake_time);
        let file = self.ioctl_file()?;
        let file = Self::require_file(&file)?;
        unsafe { rtc_write_alarm(file.as_raw_fd(), &rwa).map_err(|e| e.into()) }
    }

    /// Issues `RTC_AIE_OFF` to disable alarm interrupts.
    fn disable_alarm(&self) -> Result<i32, Error> {
        let file = self.ioctl_file()?;
        let file = Self::require_file(&file)?;
        unsafe { rtc_disable_alarm(file.as_raw_fd()).map_err(|e| e.into()) }
    }

    /// Issues `RTC_RD_TIME` and converts the kernel `struct rtc_time` to UTC.
    fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        let mut rt = unsafe { mem::zeroed::<RtcTime>() };
        let file = self.ioctl_file()?;
        let file = Self::require_file(&file)?;
        unsafe {
            rtc_read_time(file.as_raw_fd(), &mut rt)?;
        }
        rt.try_into()
    }

    /// Issues `RTC_SET_TIME`; requires write access to the device node.
    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let old_time = self.read_time()?;
        let rt: RtcTime = time.into();
        let file = self.ioctl_file()?;
        let file = Self::require_file(&file)?;
        unsafe {
            rtc_set_time(file.as_raw_fd(), &rt)?;
        }
        let mut clock = self
            .clock
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
        clock.drift = time.signed_duration_since(Utc::now());
        clock.pending_step = Some(time.signed_duration_since(old_time));
        Ok(())
    }

    fn drift(&self) -> Result<ChronoDuration, Error> {
        self.clock
            .lock()
            .map(|clock| clock.drift)
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))
    }

    fn take_pending_step(&self) -> Result<Option<ChronoDuration>, Error> {
        self.clock
            .lock()
            .map(|mut clock| clock.pending_step.take())
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))
    }

    fn wait_for_alarm_irq(&self, timeout: Option<Duration>) -> Result<Option<u32>, Error> {
        let mut wait_file = self.wait_file()?;
        let wait_file = Self::require_file_mut(&mut wait_file)?;
        let mut poll_fds = [PollFd::new(wait_file.as_fd(), PollFlags::POLLIN)];
        let poll_timeout = match timeout {
            Some(duration) => PollTimeout::try_from(duration).unwrap_or(PollTimeout::MAX),
            None => PollTimeout::NONE,
        };
        let ready = poll(&mut poll_fds, poll_timeout)?;
        if ready == 0 {
            return Ok(None);
        }
        let revents = poll_fds[0].revents().unwrap_or_else(PollFlags::empty);
        if !revents.contains(PollFlags::POLLIN) {
            return Err(anyhow::anyhow!(
                "RTC wait fd woke without POLLIN (revents={revents:?})"
            ));
        }
        let mut buf = [0u8; 4];
        wait_file.read_exact(&mut buf)?;
        Ok(Some(u32::from_ne_bytes(buf)))
    }

    fn release(&self) -> Result<(), Error> {
        LinuxRtc::release(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn wait_fd_is_close_on_exec() {
        use nix::fcntl::{FcntlArg, FdFlag, fcntl};

        let temp = NamedTempFile::new().unwrap();
        let rtc = LinuxRtc::new(temp.path()).unwrap();
        let wait_file = rtc.wait_file().unwrap();
        let file = LinuxRtc::require_file(&wait_file).unwrap();
        let flags = FdFlag::from_bits_truncate(fcntl(file, FcntlArg::F_GETFD).unwrap());
        assert!(flags.contains(FdFlag::FD_CLOEXEC));
    }

    #[test]
    fn release_closes_handles_and_is_idempotent() {
        let temp = NamedTempFile::new().unwrap();
        let rtc = LinuxRtc::new(temp.path()).unwrap();
        rtc.release().unwrap();
        assert!(rtc.is_released());
        assert!(rtc.read_time().is_err());
        rtc.release().unwrap();
    }
}

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

use crate::device::rtc::{HardwareWakeAlarm, Rtc, RtcTime, RtcWkalrm};

ioctl_read!(rtc_read_alarm, b'p', 0x10, RtcWkalrm);
ioctl_write_ptr!(rtc_write_alarm, b'p', 0x0f, RtcWkalrm);
ioctl_none!(rtc_disable_alarm, b'p', 0x02);
ioctl_read!(rtc_read_time, b'p', 0x09, RtcTime);
ioctl_write_ptr!(rtc_set_time, b'p', 0x0a, RtcTime);

/// Sysfs view of the single hardware wake-alarm slot.
const WAKEALARM_PATH: &str = "/sys/class/rtc/rtc0/wakealarm";

/// Sysfs node naming the RTC chip behind `/dev/rtc0`.
const RTC_NAME_PATH: &str = "/sys/class/rtc/rtc0/name";

/// Hardware RTC accessed through the Linux kernel RTC character device.
///
/// Opens a device path such as `/dev/rtc0` or `/dev/rtc` and drives it with
/// `RTC_RD_TIME`, `RTC_SET_TIME`, `RTC_WKALM_RD`, `RTC_WKALM_SET`, and
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
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let rtc = LinuxRtc::new("/dev/rtc0").await?;
/// let now = rtc.read_time().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct LinuxRtc {
    file: Arc<Mutex<Option<File>>>,
    wait_file: Arc<Mutex<Option<File>>>,
    released: Arc<AtomicBool>,
    /// Cached RTC↔civil relationship and last `set_time` step.
    clock: Arc<Mutex<LinuxRtcClock>>,
    /// Platform name read from [`RTC_NAME_PATH`] when the device was opened.
    name: Option<Arc<str>>,
    /// Longest wake alarm span the named chip can hold, when the chip is known.
    alarm_limit: Option<ChronoDuration>,
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let rtc = LinuxRtc::new("/dev/rtc0").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<LinuxRtc, Error> {
        let path = path.as_ref().to_owned();
        let (file, wait_fd) = crate::runtime::spawn_blocking(move || {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            let wait_fd = fcntl(&file, FcntlArg::F_DUPFD_CLOEXEC(0))?;
            Ok::<_, Error>((file, wait_fd))
        })
        .await??;
        let wait_file = unsafe { File::from_raw_fd(wait_fd) };
        let name = read_rtc_name().await;
        let alarm_limit = name.as_deref().and_then(alarm_limit_for);
        let rtc = LinuxRtc {
            file: Arc::new(Mutex::new(Some(file))),
            wait_file: Arc::new(Mutex::new(Some(wait_file))),
            released: Arc::new(AtomicBool::new(false)),
            clock: Arc::new(Mutex::new(LinuxRtcClock {
                drift: ChronoDuration::zero(),
                pending_step: None,
            })),
            name,
            alarm_limit,
        };
        log_alarm_capability(&rtc);
        rtc.refresh_drift_from_hardware().await?;
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

    async fn refresh_drift_from_hardware(&self) -> Result<(), Error> {
        match self.read_time().await {
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
    fn read_time_now(&self) -> Result<DateTime<Utc>, Error> {
        let mut rt = unsafe { mem::zeroed::<RtcTime>() };
        let file = self.ioctl_file()?;
        let file = Self::require_file(&file)?;
        unsafe {
            rtc_read_time(file.as_raw_fd(), &mut rt)?;
        }
        rt.try_into()
    }
}

impl Rtc for LinuxRtc {
    /// Issues `RTC_WKALM_RD` on the open device file.
    async fn alarm(&self) -> Result<RtcWkalrm, Error> {
        let this = self.clone();
        crate::runtime::spawn_blocking(move || {
            let mut rwa = RtcWkalrm::default();
            let file = this.ioctl_file()?;
            let file = Self::require_file(&file)?;
            unsafe {
                rtc_read_alarm(file.as_raw_fd(), &mut rwa)
                    .map(|_| rwa)
                    .map_err(|e| e.into())
            }
        })
        .await?
    }

    /// Reads the RTC's `wakealarm` sysfs node.
    ///
    /// The node reports an epoch while an alarm is enabled and is empty while
    /// none is, which is what the diagnostic comparison needs; `RTC_WKALM_RD`
    /// keeps the stale programmed time after the alarm is disabled.
    async fn wake_alarm(&self) -> HardwareWakeAlarm {
        match tokio::fs::read_to_string(WAKEALARM_PATH).await {
            Ok(raw) => parse_wakealarm(&raw),
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    path = WAKEALARM_PATH,
                    "wake alarm node unavailable"
                );
                HardwareWakeAlarm::Unavailable
            }
        }
    }

    fn name(&self) -> Option<Arc<str>> {
        self.name.clone()
    }

    fn alarm_limit(&self) -> Option<ChronoDuration> {
        self.alarm_limit
    }

    /// Issues `RTC_WKALM_SET` with the alarm enabled and `pending` cleared.
    async fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let this = self.clone();
        crate::runtime::spawn_blocking(move || {
            let rwa = RtcWkalrm::for_wake_time(wake_time);
            let file = this.ioctl_file()?;
            let file = Self::require_file(&file)?;
            unsafe { rtc_write_alarm(file.as_raw_fd(), &rwa).map_err(|e| e.into()) }
        })
        .await?
    }

    /// Issues `RTC_AIE_OFF` to disable alarm interrupts.
    async fn disable_alarm(&self) -> Result<i32, Error> {
        let this = self.clone();
        crate::runtime::spawn_blocking(move || {
            let file = this.ioctl_file()?;
            let file = Self::require_file(&file)?;
            unsafe { rtc_disable_alarm(file.as_raw_fd()).map_err(|e| e.into()) }
        })
        .await?
    }

    /// Issues `RTC_RD_TIME` and converts the kernel `struct rtc_time` to UTC.
    async fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        let this = self.clone();
        crate::runtime::spawn_blocking(move || this.read_time_now()).await?
    }

    /// Issues `RTC_SET_TIME`; requires write access to the device node.
    async fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let this = self.clone();
        crate::runtime::spawn_blocking(move || {
            let old_time: DateTime<Utc> = {
                let file = this.ioctl_file()?;
                let file = Self::require_file(&file)?;
                let mut rt = unsafe { mem::zeroed::<RtcTime>() };
                unsafe {
                    rtc_read_time(file.as_raw_fd(), &mut rt)?;
                    let old_time = rt.try_into()?;
                    rtc_set_time(file.as_raw_fd(), &RtcTime::from(time))?;
                    old_time
                }
            };
            let mut clock = this
                .clock
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
            clock.drift = time.signed_duration_since(Utc::now());
            clock.pending_step = Some(time.signed_duration_since(old_time));
            Ok(())
        })
        .await?
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

/// Reads the RTC chip name from [`RTC_NAME_PATH`].
async fn read_rtc_name() -> Option<Arc<str>> {
    match tokio::fs::read_to_string(RTC_NAME_PATH).await {
        Ok(raw) => parse_rtc_name(&raw).map(Arc::from),
        Err(error) => {
            tracing::debug!(
                error = %error,
                path = RTC_NAME_PATH,
                "RTC name node unavailable"
            );
            None
        }
    }
}

/// Returns the first word of the sysfs RTC name.
///
/// The node repeats the chip name before the device instance, e.g.
/// `bd70528-rtc bd70528-rtc.5.auto`.
fn parse_rtc_name(raw: &str) -> Option<&str> {
    raw.split_whitespace().next()
}

/// Returns the longest wake alarm each known RTC chip can hold.
///
/// | RTC name      | Limit    | Source                                                              |
/// | ------------- | -------- | ------------------------------------------------------------------- |
/// | `bd70528-rtc` | 23 h     | [`rtc-bd70528.c`], [`BD70528_MASK_ALM_EN`], [cadmus#1253]           |
/// | `pmic_rtc`    | 65 000 s | KOReader's [`dodgy_rtc`] workaround ([koreader#8039]); untested here |
/// | anything else | none     | relies on the early-fire warning to surface the real limit          |
///
/// The BD71828 driver enables comparison only for second, minute and hour
/// (`BD70528_MASK_ALM_EN`), so an alarm further ahead fires at the next matching
/// time of day. 23 h leaves an hour of margin under that window for write
/// latency and clock steps.
///
/// [`rtc-bd70528.c`]: https://github.com/torvalds/linux/blob/f93eb7cfc4bfa5d46340f360873726e3e77f2365/drivers/rtc/rtc-bd70528.c#L129
/// [`BD70528_MASK_ALM_EN`]: https://github.com/torvalds/linux/blob/f93eb7cfc4bfa5d46340f360873726e3e77f2365/include/linux/mfd/rohm-shared.h#L19
/// [`dodgy_rtc`]: https://github.com/koreader/koreader/blob/master/frontend/device/kobo/device.lua
/// [koreader#8039]: https://github.com/koreader/koreader/issues/8039
/// [cadmus#1253]: https://github.com/OGKevin/cadmus/issues/1253
fn alarm_limit_for(name: &str) -> Option<ChronoDuration> {
    match name {
        "bd70528-rtc" => Some(ChronoDuration::hours(23)),
        "pmic_rtc" => Some(ChronoDuration::seconds(65_000)),
        _ => None,
    }
}

/// Logs the RTC name and the alarm limit in force for it.
fn log_alarm_capability(rtc: &LinuxRtc) {
    match (rtc.name(), rtc.alarm_limit()) {
        (Some(name), Some(limit)) => {
            let limit = limit.to_std().unwrap_or_default();
            tracing::info!(
                rtc_name = %name,
                alarm_limit = %humantime::format_duration(limit),
                "RTC alarm limit"
            );
        }
        (Some(name), None) => tracing::info!(rtc_name = %name, "RTC has no known alarm limit"),
        (None, _) => tracing::info!("RTC name unavailable; no alarm limit"),
    }
}

/// Parses `/sys/class/rtc/rtc0/wakealarm` content.
///
/// Empty content means no alarm is armed; unparseable content means the node is
/// present but not understood.
fn parse_wakealarm(raw: &str) -> HardwareWakeAlarm {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return HardwareWakeAlarm::Disabled;
    }
    match trimmed
        .parse::<i64>()
        .ok()
        .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
    {
        Some(time) => HardwareWakeAlarm::Armed(time),
        None => HardwareWakeAlarm::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use tempfile::NamedTempFile;

    #[test]
    fn rtc_name_uses_first_word() {
        assert_eq!(
            parse_rtc_name("bd70528-rtc bd70528-rtc.5.auto\n"),
            Some("bd70528-rtc")
        );
        assert_eq!(parse_rtc_name("  pmic_rtc\n"), Some("pmic_rtc"));
        assert_eq!(parse_rtc_name("\n"), None);
    }

    #[test]
    fn alarm_limit_known_rtc_names() {
        assert_eq!(
            alarm_limit_for("bd70528-rtc"),
            Some(ChronoDuration::hours(23))
        );
        assert_eq!(
            alarm_limit_for("pmic_rtc"),
            Some(ChronoDuration::seconds(65_000))
        );
    }

    #[test]
    fn alarm_limit_unknown_rtc_name() {
        assert_eq!(alarm_limit_for("rk808-rtc"), None);
    }

    #[test]
    fn wakealarm_parses_epoch_seconds() {
        assert_eq!(
            parse_wakealarm("1700000000\n"),
            HardwareWakeAlarm::Armed(Utc.with_ymd_and_hms(2023, 11, 14, 22, 13, 20).unwrap())
        );
    }

    #[test]
    fn wakealarm_empty_is_disabled() {
        assert_eq!(parse_wakealarm(""), HardwareWakeAlarm::Disabled);
        assert_eq!(parse_wakealarm("\n"), HardwareWakeAlarm::Disabled);
    }

    #[test]
    fn wakealarm_malformed_is_unavailable() {
        assert_eq!(parse_wakealarm("garbage\n"), HardwareWakeAlarm::Unavailable);
        assert_eq!(
            parse_wakealarm("999999999999999999999999\n"),
            HardwareWakeAlarm::Unavailable
        );
    }

    #[tokio::test]
    async fn wait_fd_is_close_on_exec() {
        use nix::fcntl::{FcntlArg, FdFlag, fcntl};

        let temp = NamedTempFile::new().unwrap();
        let rtc = LinuxRtc::new(temp.path()).await.unwrap();
        let wait_file = rtc.wait_file().unwrap();
        let file = LinuxRtc::require_file(&wait_file).unwrap();
        let flags = FdFlag::from_bits_truncate(fcntl(file, FcntlArg::F_GETFD).unwrap());
        assert!(flags.contains(FdFlag::FD_CLOEXEC));
    }

    #[tokio::test]
    async fn release_closes_handles_and_is_idempotent() {
        let temp = NamedTempFile::new().unwrap();
        let rtc = LinuxRtc::new(temp.path()).await.unwrap();
        rtc.release().unwrap();
        assert!(rtc.is_released());
        assert!(rtc.read_time().await.is_err());
        rtc.release().unwrap();
    }
}

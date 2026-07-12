//! Linux ioctl RTC implementation.

use anyhow::Error;
use chrono::{DateTime, Utc};
use nix::{ioctl_none, ioctl_read, ioctl_write_ptr};
use std::fs::{File, OpenOptions};
use std::mem;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};

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
/// descriptor.
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
pub struct LinuxRtc(Arc<Mutex<File>>);

impl LinuxRtc {
    /// Opens the RTC device and creates a new interface handle.
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
        Ok(LinuxRtc(Arc::new(Mutex::new(file))))
    }
}

impl Rtc for LinuxRtc {
    /// Issues `RTC_ALM_READ` on the open device file.
    fn alarm(&self) -> Result<RtcWkalrm, Error> {
        let mut rwa = RtcWkalrm::default();
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe {
            rtc_read_alarm(file.as_raw_fd(), &mut rwa)
                .map(|_| rwa)
                .map_err(|e| e.into())
        }
    }

    /// Issues `RTC_ALM_SET` with the alarm enabled and `pending` cleared.
    fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let rwa = RtcWkalrm::for_wake_time(wake_time);
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe { rtc_write_alarm(file.as_raw_fd(), &rwa).map_err(|e| e.into()) }
    }

    /// Issues `RTC_AIE_OFF` to disable alarm interrupts.
    fn disable_alarm(&self) -> Result<i32, Error> {
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe { rtc_disable_alarm(file.as_raw_fd()).map_err(|e| e.into()) }
    }

    /// Issues `RTC_RD_TIME` and converts the kernel `struct rtc_time` to UTC.
    fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        let mut rt = unsafe { mem::zeroed::<RtcTime>() };
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe {
            rtc_read_time(file.as_raw_fd(), &mut rt)?;
        }
        rt.try_into()
    }

    /// Issues `RTC_SET_TIME`; requires write access to the device node.
    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let rt: RtcTime = time.into();
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe {
            rtc_set_time(file.as_raw_fd(), &rt)?;
        }
        Ok(())
    }
}

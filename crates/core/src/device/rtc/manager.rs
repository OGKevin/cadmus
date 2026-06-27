//! RTC trait definition.

use anyhow::Error;
use chrono::{DateTime, Utc};

use super::RtcWkalrm;

/// Battery-backed real-time clock and wake-alarm operations.
///
/// Abstracts the platform clock that keeps time while the device sleeps and can
/// raise a single hardware wake alarm. Implementations may be backed by real
/// hardware or an in-memory test double.
///
/// # Consumers
///
/// [`super::AlarmManager`] multiplexes logical alarms onto one wake alarm and
/// reads alarm state after wake to detect which alarms fired.
/// [`crate::time_manager::TimeManager`] writes NTP-synced time back to the RTC
/// after a successful sync so the battery-backed clock matches the system clock.
///
/// # Thread safety
///
/// Implementations are [`Send`] + [`Sync`] and must tolerate concurrent calls
/// from multiple threads. Callers should not assume re-entrancy on the same
/// thread.
pub trait Rtc: Send + Sync {
    /// Returns the current wake-alarm configuration.
    ///
    /// The returned [`RtcWkalrm`] reports whether a wake alarm is enabled,
    /// whether one is pending, and the programmed wake time. [`super::AlarmManager`]
    /// uses this after wake to decide whether the hardware alarm fired.
    ///
    /// # Errors
    ///
    /// Returns an error when the alarm state cannot be read.
    fn alarm(&self) -> Result<RtcWkalrm, Error>;

    /// Schedules a single-shot wake alarm at `wake_time`.
    ///
    /// Replaces any previously scheduled alarm. [`super::AlarmManager`] calls
    /// this whenever the earliest logical alarm changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the alarm cannot be programmed. On success, returns
    /// an implementation-defined status code (often zero).
    fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error>;

    /// Disables the wake alarm without clearing the stored wake time.
    ///
    /// [`super::AlarmManager`] calls this when no logical alarms remain scheduled.
    ///
    /// # Errors
    ///
    /// Returns an error when the alarm cannot be disabled. On success, returns
    /// an implementation-defined status code (often zero).
    fn disable_alarm(&self) -> Result<i32, Error>;

    /// Returns the current RTC time in UTC.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock cannot be read or the stored fields are
    /// invalid.
    fn read_time(&self) -> Result<DateTime<Utc>, Error>;

    /// Sets the RTC to `time`.
    ///
    /// [`crate::time_manager::TimeManager`] calls this after a successful NTP
    /// sync. May require elevated privileges on some platforms.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock cannot be updated.
    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error>;
}

impl<T: Rtc + ?Sized> Rtc for std::sync::Arc<T> {
    fn alarm(&self) -> Result<RtcWkalrm, Error> {
        (**self).alarm()
    }

    fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        (**self).set_alarm(wake_time)
    }

    fn disable_alarm(&self) -> Result<i32, Error> {
        (**self).disable_alarm()
    }

    fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        (**self).read_time()
    }

    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        (**self).set_time(time)
    }
}

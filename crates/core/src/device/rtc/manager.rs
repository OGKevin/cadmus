//! RTC trait definition.

use anyhow::Error;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::time::Duration;

use super::RtcWkalrm;

/// Battery-backed real-time clock and wake-alarm operations.
///
/// Abstracts the platform clock that keeps time while the device sleeps and can
/// raise a single hardware wake alarm. Implementations may be backed by real
/// hardware or an in-memory test double.
///
/// # Consumers
///
/// [`super::AlarmManager`] multiplexes logical alarms onto one wake alarm,
/// waits for alarm IRQs, and claims which logical alarms fired.
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
    /// This low-level operation records a pending step but does not synchronize
    /// logical alarms. App clock updates should call [`super::set_time`] so
    /// [`super::AlarmManager::sync`] consumes the step immediately. May require
    /// elevated privileges on some platforms.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock cannot be updated.
    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error>;

    /// Returns the RTC offset from the civil system clock (`RTC_now − system_now`).
    ///
    /// Positive means the RTC reads ahead of civil time. Implementations
    /// establish this value during initialization and refresh it only after
    /// [`Rtc::set_time`], while Cadmus retains exclusive RTC access.
    fn drift(&self) -> Result<ChronoDuration, Error>;

    /// Converts a civil system-clock instant to the RTC timeline.
    fn to_rtc(&self, civil: DateTime<Utc>) -> Result<DateTime<Utc>, Error> {
        Ok(civil + self.drift()?)
    }

    /// Converts an RTC-timeline instant to the civil system-clock timeline.
    fn to_civil(&self, rtc: DateTime<Utc>) -> Result<DateTime<Utc>, Error> {
        Ok(rtc - self.drift()?)
    }

    /// Takes the clock step recorded by the latest [`Rtc::set_time`] call.
    ///
    /// The returned duration is `new_time - old_time`. Taking it clears the
    /// pending value.
    fn take_pending_step(&self) -> Result<Option<ChronoDuration>, Error>;

    /// Blocks until an alarm IRQ is readable, or until `timeout` elapses.
    ///
    /// Returns `Ok(Some(data))` with the kernel IRQ data word when an alarm
    /// interrupt is delivered, `Ok(None)` when `timeout` expires without an
    /// IRQ, or an error on I/O failure. A `timeout` of `None` waits indefinitely.
    ///
    /// # Errors
    ///
    /// Returns an error when waiting or reading the IRQ fails.
    fn wait_for_alarm_irq(&self, timeout: Option<Duration>) -> Result<Option<u32>, Error>;

    /// Releases platform RTC resources held by this handle.
    ///
    /// Default implementation is a no-op. Linux closes exclusive `/dev/rtc0`
    /// opens so a subsequent Cadmus process can reopen the device.
    fn release(&self) -> Result<(), Error> {
        let _ = self;
        Ok(())
    }
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

    fn drift(&self) -> Result<ChronoDuration, Error> {
        (**self).drift()
    }

    fn take_pending_step(&self) -> Result<Option<ChronoDuration>, Error> {
        (**self).take_pending_step()
    }

    fn wait_for_alarm_irq(&self, timeout: Option<Duration>) -> Result<Option<u32>, Error> {
        (**self).wait_for_alarm_irq(timeout)
    }

    fn release(&self) -> Result<(), Error> {
        (**self).release()
    }
}

use anyhow::Error;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use nix::{ioctl_none, ioctl_read, ioctl_write_ptr};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::mem;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};

ioctl_read!(rtc_read_alarm, b'p', 0x10, RtcWkalrm);
ioctl_write_ptr!(rtc_write_alarm, b'p', 0x0f, RtcWkalrm);
ioctl_none!(rtc_disable_alarm, b'p', 0x02);
ioctl_read!(rtc_read_time, b'p', 0x09, RtcTime);
ioctl_write_ptr!(rtc_set_time, b'p', 0x0a, RtcTime);

#[repr(C)]
#[derive(Debug, Clone)]
pub struct RtcTime {
    tm_sec: libc::c_int,
    tm_min: libc::c_int,
    tm_hour: libc::c_int,
    tm_mday: libc::c_int,
    tm_mon: libc::c_int,
    tm_year: libc::c_int,
    tm_wday: libc::c_int,
    tm_yday: libc::c_int,
    tm_isdst: libc::c_int,
}

impl Default for RtcWkalrm {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct RtcWkalrm {
    enabled: libc::c_uchar,
    pending: libc::c_uchar,
    time: RtcTime,
}

impl RtcTime {
    fn year(&self) -> i32 {
        1900 + self.tm_year
    }
}

impl TryFrom<RtcTime> for DateTime<Utc> {
    type Error = Error;

    fn try_from(rt: RtcTime) -> Result<Self, Self::Error> {
        Utc.with_ymd_and_hms(
            rt.year(),
            (rt.tm_mon as u32) + 1,
            rt.tm_mday as u32,
            rt.tm_hour as u32,
            rt.tm_min as u32,
            rt.tm_sec as u32,
        )
        .single()
        .ok_or_else(|| anyhow::anyhow!("invalid RTC date/time fields"))
    }
}

impl From<DateTime<Utc>> for RtcTime {
    fn from(dt: DateTime<Utc>) -> Self {
        RtcTime {
            tm_sec: dt.second() as libc::c_int,
            tm_min: dt.minute() as libc::c_int,
            tm_hour: dt.hour() as libc::c_int,
            tm_mday: dt.day() as libc::c_int,
            tm_mon: dt.month0() as libc::c_int,
            tm_year: (dt.year() - 1900) as libc::c_int,
            tm_wday: -1,
            tm_yday: -1,
            tm_isdst: -1,
        }
    }
}

impl RtcWkalrm {
    /// Returns whether the alarm is currently enabled.
    pub fn enabled(&self) -> bool {
        self.enabled == 1
    }

    /// Returns the year field from the alarm's stored time.
    ///
    /// This is the full calendar year (e.g., 2024), not the offset from 1900.
    pub fn year(&self) -> i32 {
        self.time.year()
    }
}

/// Interface to the hardware real-time clock device.
///
/// `Rtc` provides access to both time and alarm functionality of the RTC.
/// Operations are serialized via an internal mutex to ensure thread-safe access
/// to the underlying device file.
pub struct Rtc(Arc<Mutex<File>>);

impl Clone for Rtc {
    fn clone(&self) -> Self {
        Rtc(Arc::clone(&self.0))
    }
}

impl Rtc {
    /// Opens the RTC device and creates a new interface handle.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the RTC device file (typically `/dev/rtc0` or `/dev/rtc`)
    ///
    /// # Returns
    ///
    /// A new `Rtc` handle on success, or an error if the device cannot be opened.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cadmus_core::rtc::Rtc;
    /// let rtc = Rtc::new("/dev/rtc0")?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Rtc, Error> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Rtc(Arc::new(Mutex::new(file))))
    }

    /// Reads the current alarm settings from the hardware.
    ///
    /// Returns information about the wake alarm, including whether it is enabled
    /// and any pending alarm status. The alarm time is stored as [`RtcWkalrm`].
    ///
    /// # Returns
    ///
    /// Alarm settings on success, or an error if the ioctl fails or the lock is poisoned.
    pub fn alarm(&self) -> Result<RtcWkalrm, Error> {
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

    /// Programs the hardware to wake at the specified time.
    ///
    /// Enables a single-shot alarm that will fire at the given UTC time.
    /// If an alarm is already scheduled, it is replaced.
    ///
    /// # Arguments
    ///
    /// * `wake_time` - The UTC time when the alarm should fire
    ///
    /// # Returns
    ///
    /// A status code on success (typically 0 if supported), or an error if
    /// the ioctl fails or the lock is poisoned.
    pub fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let rwa = RtcWkalrm {
            enabled: 1,
            pending: 0,
            time: wake_time.into(),
        };
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe { rtc_write_alarm(file.as_raw_fd(), &rwa).map_err(|e| e.into()) }
    }

    /// Disables the hardware alarm.
    ///
    /// Clears any pending alarm without affecting the alarm time itself.
    ///
    /// # Returns
    ///
    /// A status code on success (typically 0 if supported), or an error if
    /// the ioctl fails or the lock is poisoned.
    pub fn disable_alarm(&self) -> Result<i32, Error> {
        let file = self
            .0
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        unsafe { rtc_disable_alarm(file.as_raw_fd()).map_err(|e| e.into()) }
    }

    /// Reads the current time from the hardware RTC.
    ///
    /// # Returns
    ///
    /// The current UTC time on success, or an error if the ioctl fails,
    /// the RTC fields are invalid, or the lock is poisoned.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cadmus_core::rtc::Rtc;
    /// # let rtc = Rtc::new("/dev/rtc0")?;
    /// let now = rtc.read_time()?;
    /// println!("RTC time: {}", now);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn read_time(&self) -> Result<DateTime<Utc>, Error> {
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

    /// Sets the hardware RTC to the specified time.
    ///
    /// Updates the RTC with a new UTC time. This typically requires elevated privileges.
    ///
    /// # Arguments
    ///
    /// * `time` - The UTC time to set
    ///
    /// # Returns
    ///
    /// Success with no value, or an error if the ioctl fails or the lock is poisoned.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cadmus_core::rtc::Rtc;
    /// # use chrono::Utc;
    /// # let rtc = Rtc::new("/dev/rtc0")?;
    /// rtc.set_time(Utc::now())?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
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

/// Identifies a logical alarm managed by [`AlarmManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlarmType {
    AutoPowerOff,
    CalendarUpdate,
}

/// Describes what [`AlarmManager::ensure_scheduled`] should do when an alarm
/// exists in the map but its wake time is already in the past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PastDueAction {
    /// Cancel the stale alarm and reschedule it for `now + duration`.
    Reschedule,
    /// Cancel the stale alarm and return [`EnsureAlarmOutcome::PastDue`]
    /// so the caller can decide what to do.
    Cancel,
}

/// The outcome of an [`AlarmManager::ensure_scheduled`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureAlarmOutcome {
    /// No alarm of this type existed; one was freshly scheduled.
    Scheduled,
    /// An alarm of this type already existed and its wake time is in the future.
    AlreadyScheduled,
    /// An alarm of this type existed but was past-due; it has been cancelled.
    ///
    /// Only returned when [`PastDueAction::Cancel`] was requested. When
    /// [`PastDueAction::Reschedule`] is requested the stale alarm is replaced
    /// and [`EnsureAlarmOutcome::Scheduled`] is returned instead.
    PastDue,
}

impl AlarmType {
    pub fn alarms_to_cancel_after_resume() -> [Self; 2] {
        [Self::AutoPowerOff, Self::CalendarUpdate]
    }
}

pub struct ScheduledAlarm {
    pub alarm_type: AlarmType,
    pub wake_time: DateTime<Utc>,
}

/// Multiplexes multiple logical alarms onto a single hardware RTC alarm.
///
/// The hardware RTC supports only one wake alarm at a time. `AlarmManager`
/// maintains a map of logical alarms keyed by [`AlarmType`] and always
/// programs the hardware with the earliest upcoming wake time. After each
/// wake, [`AlarmManager::check_fired_alarms`] determines which logical alarms fired and
/// reschedules the hardware for any remaining ones.
pub struct AlarmManager {
    rtc: Rtc,
    scheduled_alarms: BTreeMap<AlarmType, ScheduledAlarm>,
}

impl AlarmManager {
    pub fn new(rtc: Rtc) -> Self {
        AlarmManager {
            rtc,
            scheduled_alarms: BTreeMap::new(),
        }
    }

    /// Schedule a logical alarm to fire `duration` from now.
    ///
    /// If an alarm of the same type is already scheduled it is replaced.
    /// The hardware RTC is updated to reflect the new earliest wake time.
    pub fn schedule_alarm(
        &mut self,
        alarm_type: AlarmType,
        duration: Duration,
    ) -> Result<(), Error> {
        let wake_time = Utc::now() + duration;
        self.scheduled_alarms.insert(
            alarm_type,
            ScheduledAlarm {
                alarm_type,
                wake_time,
            },
        );
        self.update_hardware_alarm()?;
        Ok(())
    }

    /// Cancel a previously scheduled logical alarm.
    ///
    /// If no alarm of that type is scheduled this is a no-op. The hardware
    /// RTC is updated to reflect the new earliest remaining wake time.
    pub fn cancel_alarm(&mut self, alarm_type: AlarmType) -> Result<(), Error> {
        self.scheduled_alarms.remove(&alarm_type);
        self.update_hardware_alarm()?;
        Ok(())
    }

    /// Returns `true` if an alarm of `alarm_type` is scheduled for a future time.
    pub fn is_alarm_scheduled(&self, alarm_type: AlarmType) -> bool {
        self.scheduled_alarms
            .get(&alarm_type)
            .map(|alarm| alarm.wake_time > Utc::now())
            .unwrap_or(false)
    }

    /// Returns `true` if an alarm of `alarm_type` exists in the schedule.
    pub fn has_alarm(&self, alarm_type: AlarmType) -> bool {
        self.scheduled_alarms.contains_key(&alarm_type)
    }

    /// Ensures an alarm of `alarm_type` is active and scheduled for the future.
    ///
    /// - If no alarm exists, one is scheduled for `now + duration`.
    /// - If an alarm exists and is in the future, nothing changes.
    /// - If an alarm exists but is past-due, the stale entry is always
    ///   cancelled. `past_due_action` then controls whether a fresh alarm is
    ///   scheduled: [`PastDueAction::Reschedule`] schedules a new one and
    ///   returns [`EnsureAlarmOutcome::Scheduled`]; [`PastDueAction::Cancel`]
    ///   stops there and returns [`EnsureAlarmOutcome::PastDue`] so the caller
    ///   can decide what action to take.
    pub fn ensure_scheduled(
        &mut self,
        alarm_type: AlarmType,
        duration: Duration,
        past_due_action: PastDueAction,
    ) -> Result<EnsureAlarmOutcome, Error> {
        if !self.has_alarm(alarm_type) {
            self.schedule_alarm(alarm_type, duration)?;
            return Ok(EnsureAlarmOutcome::Scheduled);
        }

        if self.is_alarm_scheduled(alarm_type) {
            return Ok(EnsureAlarmOutcome::AlreadyScheduled);
        }

        self.cancel_alarm(alarm_type)?;

        match past_due_action {
            PastDueAction::Reschedule => {
                self.schedule_alarm(alarm_type, duration)?;
                Ok(EnsureAlarmOutcome::Scheduled)
            }
            PastDueAction::Cancel => Ok(EnsureAlarmOutcome::PastDue),
        }
    }

    /// Returns the number of seconds until `alarm_type` fires, or `None` if
    /// it is not scheduled.
    pub fn time_until_alarm(&self, alarm_type: AlarmType) -> Option<i64> {
        self.scheduled_alarms.get(&alarm_type).map(|alarm| {
            alarm
                .wake_time
                .signed_duration_since(Utc::now())
                .num_seconds()
        })
    }

    /// Determines which logical alarms fired during the last sleep cycle.
    ///
    /// `before` is the timestamp just before the device went to sleep and
    /// `after` is the timestamp just after it woke. A hardware alarm is
    /// considered fired when it is disabled or when the sleep duration is
    /// within 3 seconds of the expected wake time (accounting for RTC
    /// granularity). Any fired logical alarms are removed from the schedule
    /// and the hardware is reprogrammed for the next earliest alarm.
    pub fn check_fired_alarms(
        &mut self,
        before: DateTime<Utc>,
        after: DateTime<Utc>,
    ) -> Result<Vec<AlarmType>, Error> {
        let mut fired_types = Vec::new();

        if let Some((_, earliest_alarm)) = self
            .scheduled_alarms
            .iter()
            .min_by_key(|(_, alarm)| &alarm.wake_time)
        {
            let expected_duration = earliest_alarm.wake_time.signed_duration_since(before);

            let rwa = self.rtc.alarm()?;
            let hardware_alarm_fired = !rwa.enabled()
                || (rwa.year() <= 1970
                    && ((after - before) - expected_duration).num_seconds().abs() < 3);

            if hardware_alarm_fired {
                let mut removed: Vec<(AlarmType, ScheduledAlarm)> = Vec::new();

                for (alarm_type, scheduled_alarm) in &self.scheduled_alarms {
                    if (after - scheduled_alarm.wake_time).abs().num_milliseconds() <= 3000 {
                        fired_types.push(*alarm_type);
                        removed.push((
                            *alarm_type,
                            ScheduledAlarm {
                                alarm_type: scheduled_alarm.alarm_type,
                                wake_time: scheduled_alarm.wake_time,
                            },
                        ));
                    }
                }

                for (alarm_type, _) in &removed {
                    self.scheduled_alarms.remove(alarm_type);
                }

                if let Err(e) = self.update_hardware_alarm() {
                    for (alarm_type, alarm) in removed {
                        self.scheduled_alarms.insert(alarm_type, alarm);
                    }
                    return Err(e);
                }

                return Ok(fired_types);
            }
        }

        self.update_hardware_alarm()?;
        Ok(fired_types)
    }

    fn update_hardware_alarm(&self) -> Result<(), Error> {
        let now = Utc::now();

        if let Some((_, earliest_alarm)) = self
            .scheduled_alarms
            .iter()
            .filter(|(_, alarm)| alarm.wake_time > now)
            .min_by_key(|(_, alarm)| &alarm.wake_time)
        {
            self.rtc.set_alarm(earliest_alarm.wake_time)?;
        } else {
            self.rtc.disable_alarm()?;
        }

        Ok(())
    }
}

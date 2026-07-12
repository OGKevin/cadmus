//! Real-time clock and alarm management.

mod manager;

#[cfg(any(
    test,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
mod test;

pub use manager::Rtc;

#[cfg(any(
    test,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
pub use test::TestRtc;

use anyhow::Error;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use std::collections::BTreeMap;
use std::mem;
use std::sync::Arc;

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
    pub(crate) fn for_wake_time(wake_time: DateTime<Utc>) -> Self {
        Self {
            enabled: 1,
            pending: 0,
            time: wake_time.into(),
        }
    }

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
pub struct AlarmManager<R: Rtc> {
    rtc: Arc<R>,
    scheduled_alarms: BTreeMap<AlarmType, ScheduledAlarm>,
}

impl<R: Rtc> AlarmManager<R> {
    pub fn new(rtc: Arc<R>) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_alarm_manager() -> (TestRtc, AlarmManager<TestRtc>) {
        let rtc = TestRtc::new();
        let manager = AlarmManager::new(Arc::new(rtc.clone()));
        (rtc, manager)
    }

    #[test]
    fn ensure_scheduled_fresh() {
        let (_rtc, mut manager) = test_alarm_manager();
        let outcome = manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::Scheduled);
        assert!(manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[test]
    fn ensure_scheduled_already_scheduled() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .unwrap();
        let outcome = manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::AlreadyScheduled);
    }

    #[test]
    fn ensure_scheduled_past_due_reschedule() {
        let (rtc, mut manager) = test_alarm_manager();
        let past = Utc::now() - Duration::hours(2);
        rtc.set_current_time(past + Duration::minutes(30));
        manager
            .schedule_alarm(AlarmType::CalendarUpdate, Duration::minutes(-90))
            .unwrap();
        rtc.set_current_time(Utc::now());
        let outcome = manager
            .ensure_scheduled(
                AlarmType::CalendarUpdate,
                Duration::minutes(5),
                PastDueAction::Reschedule,
            )
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::Scheduled);
        assert!(manager.is_alarm_scheduled(AlarmType::CalendarUpdate));
    }

    #[test]
    fn ensure_scheduled_past_due_cancel() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_alarm(AlarmType::AutoPowerOff, Duration::seconds(-10))
            .unwrap();
        rtc.set_current_time(Utc::now());
        let outcome = manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::PastDue);
        assert!(!manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[test]
    fn check_fired_alarms_detects_fired() {
        let (rtc, mut manager) = test_alarm_manager();
        let before = Utc::now();
        manager
            .schedule_alarm(AlarmType::AutoPowerOff, Duration::minutes(5))
            .unwrap();
        rtc.simulate_alarm_fired();
        let after = before + Duration::minutes(5);
        let fired = manager.check_fired_alarms(before, after).unwrap();
        assert!(fired.contains(&AlarmType::AutoPowerOff));
    }

    #[test]
    fn check_fired_alarms_not_fired() {
        let (rtc, mut manager) = test_alarm_manager();
        let before = Utc::now();
        manager
            .schedule_alarm(AlarmType::AutoPowerOff, Duration::hours(1))
            .unwrap();
        let after = before + Duration::minutes(1);
        let fired = manager.check_fired_alarms(before, after).unwrap();
        assert!(fired.is_empty());
        assert!(!rtc.alarm_enabled() || manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[test]
    fn multiplexing_earliest_alarm_wins() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_alarm(AlarmType::AutoPowerOff, Duration::hours(2))
            .unwrap();
        manager
            .schedule_alarm(AlarmType::CalendarUpdate, Duration::minutes(30))
            .unwrap();
        let wake = rtc.scheduled_wake_time().unwrap();
        let expected = Utc::now() + Duration::minutes(30);
        assert!((wake - expected).num_seconds().abs() < 2);
    }
}

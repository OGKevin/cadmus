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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration as StdDuration;

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

    #[cfg(any(feature = "emulator", docsrs))]
    pub(crate) fn from_parts(enabled: bool, wake_time: DateTime<Utc>) -> Self {
        Self {
            enabled: u8::from(enabled),
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
///
/// # Auto Suspend
///
/// [`AlarmType::AutoSuspend`] is the wall-clock idle deadline for entering the
/// suspend flow. Cadmus schedules it from the Auto Suspend setting (minutes from
/// now) and **reschedules on user activity** so the deadline tracks real idle
/// time. Soft sleep freezes monotonic clocks (`Instant::elapsed`), so Auto
/// Suspend must not treat monotonic idle as authority — the RTC wake time is
/// the source of truth. When the hardware IRQ fires, [`AlarmManager`]'s listener
/// claims due alarms and emits [`crate::view::Event::RtcAlarmFired`]; lifecycle
/// then calls `begin_suspend`. Entering an explicit suspend cancels Auto
/// Suspend; returning to interactive use reschedules it. Unlike
/// [`AlarmType::AutoPowerOff`] / [`AlarmType::CalendarUpdate`], Auto Suspend is
/// not listed in [`AlarmType::alarms_to_cancel_after_resume`] — cancel-on-resume
/// would drop the idle timer instead of re-arming it for the next idle window.
///
/// # Suspend
///
/// [`AlarmType::Suspend`] is the wall-clock delay after PrepareSuspend before
/// entering sleep (`handle_suspend`). Power Released / long-hold cancel it to
/// abort the cycle. Not listed in [`AlarmType::alarms_to_cancel_after_resume`].
///
/// # Wake Debounce
///
/// [`AlarmType::WakeDebounce`] is the wall-clock re-sleep deadline after leaving
/// classic `state=mem`. Userspace `thread::sleep` cannot drive re-sleep under
/// autosleep. When it fires, lifecycle calls `begin_suspend`. Power Released /
/// long-hold cancel it. Not in [`AlarmType::alarms_to_cancel_after_resume`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlarmType {
    AutoPowerOff,
    /// Idle timeout that starts the suspend flow.
    AutoSuspend,
    /// Enter sleep after PrepareSuspend unless Power cancels.
    Suspend,
    /// Re-enter suspend after wake unless Power cancels.
    WakeDebounce,
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
/// programs the hardware with the earliest upcoming wake time. An owned IRQ
/// listener thread waits on [`Rtc::wait_for_alarm_irq`], claims due alarms via
/// [`AlarmManager::claim_due_alarms`], and delivers them through the callback
/// passed to [`AlarmManager::start_irq_listener`]. After resume,
/// [`AlarmManager::check_fired_alarms`] reuses the same claim path when the
/// hardware likely fired during sleep.
pub struct AlarmManager<R: Rtc> {
    rtc: Arc<R>,
    scheduled_alarms: BTreeMap<AlarmType, ScheduledAlarm>,
    irq_thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl<R: Rtc> AlarmManager<R> {
    pub fn new(rtc: Arc<R>) -> Self {
        AlarmManager {
            rtc,
            scheduled_alarms: BTreeMap::new(),
            irq_thread: None,
            stop: Arc::new(AtomicBool::new(false)),
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
                return self.claim_due_alarms_at(after);
            }
        }

        self.update_hardware_alarm()?;
        Ok(Vec::new())
    }

    /// Removes and returns logical alarms that are due at the current wall clock.
    ///
    /// An alarm is due when its wake time is at or before `at`. Reprograms the
    /// hardware for any remaining future alarms.
    pub fn claim_due_alarms(&mut self) -> Result<Vec<AlarmType>, Error> {
        self.claim_due_alarms_at(Utc::now())
    }

    fn claim_due_alarms_at(&mut self, at: DateTime<Utc>) -> Result<Vec<AlarmType>, Error> {
        let mut fired_types = Vec::new();
        let to_remove: Vec<AlarmType> = self
            .scheduled_alarms
            .iter()
            .filter(|(_, alarm)| alarm.wake_time <= at)
            .map(|(alarm_type, _)| *alarm_type)
            .collect();

        for alarm_type in to_remove {
            self.scheduled_alarms.remove(&alarm_type);
            fired_types.push(alarm_type);
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

impl<R: Rtc + 'static> AlarmManager<R> {
    /// Starts the owned IRQ listener thread once. Idempotent.
    ///
    /// `send` delivers each fired logical alarm to the hub. The join handle is
    /// stored on this manager and joined on drop after the stop flag is set.
    pub fn start_irq_listener(
        manager: &Arc<Mutex<Self>>,
        send: impl Fn(AlarmType) + Send + 'static,
    ) {
        let guard = manager.lock().unwrap_or_else(|e| e.into_inner());
        if guard.irq_thread.is_some() {
            return;
        }

        let stop = Arc::clone(&guard.stop);
        let rtc = Arc::clone(&guard.rtc);
        drop(guard);

        let manager_for_thread = Arc::clone(manager);
        let handle = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match rtc.wait_for_alarm_irq(Some(StdDuration::from_secs(1))) {
                    Ok(Some(_)) => {
                        let due = match manager_for_thread.lock() {
                            Ok(mut locked) => locked.claim_due_alarms(),
                            Err(poisoned) => poisoned.into_inner().claim_due_alarms(),
                        };
                        match due {
                            Ok(alarm_types) => {
                                for alarm_type in alarm_types {
                                    send(alarm_type);
                                }
                            }
                            Err(error) => {
                                tracing::error!(error = %error, "claim_due_alarms failed");
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::warn!("RTC alarm wait returned with no IRQ data");
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "wait_for_alarm_irq failed");
                        thread::sleep(StdDuration::from_secs(1));
                    }
                }
            }
        });

        manager.lock().unwrap_or_else(|e| e.into_inner()).irq_thread = Some(handle);
    }
}

impl<R: Rtc> Drop for AlarmManager<R> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.irq_thread.take() {
            let _ = handle.join();
        }
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
        let after = before + Duration::minutes(5) + Duration::seconds(1);
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

    #[test]
    fn auto_suspend_can_be_earliest_alarm() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_alarm(AlarmType::AutoPowerOff, Duration::hours(2))
            .unwrap();
        manager
            .schedule_alarm(AlarmType::AutoSuspend, Duration::minutes(10))
            .unwrap();
        let wake = rtc.scheduled_wake_time().unwrap();
        let expected = Utc::now() + Duration::minutes(10);
        assert!((wake - expected).num_seconds().abs() < 2);
        assert!(manager.is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn auto_suspend_not_in_cancel_after_resume_list() {
        assert!(
            !AlarmType::alarms_to_cancel_after_resume()
                .into_iter()
                .any(|a| a == AlarmType::AutoSuspend)
        );
    }

    #[test]
    fn suspend_alarm_not_in_cancel_after_resume_list() {
        assert!(
            !AlarmType::alarms_to_cancel_after_resume()
                .into_iter()
                .any(|a| a == AlarmType::Suspend)
        );
    }

    #[test]
    fn wake_debounce_not_in_cancel_after_resume_list() {
        assert!(
            !AlarmType::alarms_to_cancel_after_resume()
                .into_iter()
                .any(|a| a == AlarmType::WakeDebounce)
        );
    }

    #[test]
    fn claim_due_alarms_returns_past_due_once() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_alarm(AlarmType::AutoSuspend, Duration::seconds(-10))
            .unwrap();
        let first = manager.claim_due_alarms().unwrap();
        assert_eq!(first, vec![AlarmType::AutoSuspend]);
        let second = manager.claim_due_alarms().unwrap();
        assert!(second.is_empty());
        assert!(!manager.has_alarm(AlarmType::AutoSuspend));
    }

    #[test]
    fn claim_due_alarms_ignores_near_future_wake() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_alarm(AlarmType::AutoSuspend, Duration::seconds(2))
            .unwrap();
        let claimed = manager.claim_due_alarms().unwrap();
        assert!(claimed.is_empty());
        assert!(manager.is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn irq_listener_claims_past_due_on_simulated_irq() {
        use std::sync::{Arc, Mutex};

        let (rtc, manager) = test_alarm_manager();
        let manager = Arc::new(Mutex::new(manager));
        {
            let mut locked = manager.lock().unwrap();
            locked
                .schedule_alarm(AlarmType::AutoSuspend, Duration::seconds(-10))
                .unwrap();
        }

        let claimed = Arc::new(Mutex::new(Vec::new()));
        let claimed_for_send = Arc::clone(&claimed);
        AlarmManager::start_irq_listener(&manager, move |alarm_type| {
            claimed_for_send.lock().unwrap().push(alarm_type);
        });

        std::thread::sleep(StdDuration::from_millis(50));
        rtc.simulate_alarm_fired();

        let deadline = std::time::Instant::now() + StdDuration::from_secs(3);
        while claimed.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(StdDuration::from_millis(50));
        }

        assert_eq!(*claimed.lock().unwrap(), vec![AlarmType::AutoSuspend]);
        assert!(!manager.lock().unwrap().has_alarm(AlarmType::AutoSuspend));
    }

    #[test]
    fn wait_for_alarm_irq_wakes_on_simulate() {
        let (rtc, _manager) = test_alarm_manager();
        let rtc_waiter = rtc.clone();
        let handle = std::thread::spawn(move || {
            rtc_waiter
                .wait_for_alarm_irq(Some(StdDuration::from_secs(2)))
                .unwrap()
        });
        std::thread::sleep(StdDuration::from_millis(50));
        rtc.simulate_alarm_fired();
        let data = handle.join().unwrap();
        assert_eq!(data, Some(0x20));
    }
}

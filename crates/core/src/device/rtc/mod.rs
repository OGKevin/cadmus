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

/// Instant on either the hardware RTC timeline or the civil system wall clock.
///
/// **Civil** means the system wall clock (`Utc::now()` / `Local`) — what the UI
/// and NTP use — not the hardware RTC register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockInstant {
    /// Instant already expressed on the hardware RTC timeline.
    Rtc(DateTime<Utc>),
    /// Instant on the civil system wall clock; convert with [`Rtc::to_rtc`].
    Civil(DateTime<Utc>),
}

/// Describes when a logical alarm should fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmWhen {
    /// Fires after the duration elapses on the RTC timeline.
    In(Duration),
    /// Fires at an absolute [`ClockInstant`] (RTC or civil).
    At(ClockInstant),
}

impl From<Duration> for AlarmWhen {
    fn from(duration: Duration) -> Self {
        Self::In(duration)
    }
}

impl From<ClockInstant> for AlarmWhen {
    fn from(instant: ClockInstant) -> Self {
        Self::At(instant)
    }
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

/// One logical alarm retained by [`AlarmManager`] until claimed or cancelled.
pub struct ScheduledAlarm {
    /// Logical alarm identity multiplexed onto the single hardware wake slot.
    pub alarm_type: AlarmType,
    /// Original scheduling intent; kept so [`AlarmManager::sync`] can rebase
    /// after an RTC clock step (`In` / `At(Rtc)` shift by the step, `At(Civil)`
    /// reconverts through refreshed drift).
    pub when: AlarmWhen,
    /// Resolved wake instant on the RTC timeline (what hardware is programmed with).
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

    /// Schedules a logical alarm with an explicit relative or absolute intent.
    ///
    /// Relative alarms use RTC time as their base. If the RTC cannot be read,
    /// the operation uses system time consistently as its fallback.
    pub fn schedule(&mut self, alarm_type: AlarmType, when: AlarmWhen) -> Result<(), Error> {
        let now = self.authority_now();
        self.schedule_at(alarm_type, when, now)?;
        self.update_hardware_alarm_at(now)
    }

    /// Schedules a logical alarm to fire after `duration` on the RTC timeline.
    pub fn schedule_in(&mut self, alarm_type: AlarmType, duration: Duration) -> Result<(), Error> {
        self.schedule(alarm_type, AlarmWhen::In(duration))
    }

    /// Cancel a previously scheduled logical alarm.
    ///
    /// If no alarm of that type is scheduled this is a no-op. The hardware
    /// RTC is updated to reflect the new earliest remaining wake time.
    pub fn cancel_alarm(&mut self, alarm_type: AlarmType) -> Result<(), Error> {
        let now = self.authority_now();
        self.scheduled_alarms.remove(&alarm_type);
        self.update_hardware_alarm_at(now)
    }

    /// Returns `true` if an alarm of `alarm_type` is scheduled for a future time.
    pub fn is_alarm_scheduled(&self, alarm_type: AlarmType) -> bool {
        let now = self.authority_now();
        self.scheduled_alarms
            .get(&alarm_type)
            .map(|alarm| alarm.wake_time > now)
            .unwrap_or(false)
    }

    /// Returns `true` if an alarm of `alarm_type` exists in the schedule.
    pub fn has_alarm(&self, alarm_type: AlarmType) -> bool {
        self.scheduled_alarms.contains_key(&alarm_type)
    }

    /// Ensures an alarm is active and scheduled for the future.
    ///
    /// Accepts either an [`AlarmWhen`] or a relative [`Duration`]. A stale
    /// alarm is handled according to `past_due_action`.
    pub fn ensure_scheduled<W: Into<AlarmWhen>>(
        &mut self,
        alarm_type: AlarmType,
        when: W,
        past_due_action: PastDueAction,
    ) -> Result<EnsureAlarmOutcome, Error> {
        let now = self.authority_now();
        let when = when.into();
        let outcome = match self.scheduled_alarms.get(&alarm_type) {
            None => {
                self.schedule_at(alarm_type, when, now)?;
                EnsureAlarmOutcome::Scheduled
            }
            Some(alarm) if alarm.wake_time > now => EnsureAlarmOutcome::AlreadyScheduled,
            Some(_) => {
                self.scheduled_alarms.remove(&alarm_type);
                match past_due_action {
                    PastDueAction::Reschedule => {
                        self.schedule_at(alarm_type, when, now)?;
                        EnsureAlarmOutcome::Scheduled
                    }
                    PastDueAction::Cancel => EnsureAlarmOutcome::PastDue,
                }
            }
        };
        self.update_hardware_alarm_at(now)?;
        Ok(outcome)
    }

    /// Returns the number of seconds until `alarm_type` fires, or `None` if
    /// it is not scheduled.
    pub fn time_until_alarm(&self, alarm_type: AlarmType) -> Option<i64> {
        let now = self.authority_now();
        self.scheduled_alarms
            .get(&alarm_type)
            .map(|alarm| alarm.wake_time.signed_duration_since(now).num_seconds())
    }

    /// Determines which logical alarms fired during the last sleep cycle.
    ///
    /// `before` and `after` are tagged with [`ClockInstant`] so both ends are
    /// normalized to the RTC timeline before comparing sleep length to the
    /// expected wake. Prefer the same variant for both ends of one call.
    pub fn check_fired_alarms(
        &mut self,
        before: ClockInstant,
        after: ClockInstant,
    ) -> Result<Vec<AlarmType>, Error> {
        let now = self.authority_now();
        let before_rtc = self.resolve_instant(before)?;
        let after_rtc = self.resolve_instant(after)?;
        if let Some((_, earliest_alarm)) = self
            .scheduled_alarms
            .iter()
            .min_by_key(|(_, alarm)| &alarm.wake_time)
        {
            let expected_duration = earliest_alarm.wake_time.signed_duration_since(before_rtc);

            let rwa = self.rtc.alarm()?;
            let hardware_alarm_fired = !rwa.enabled()
                || (rwa.year() <= 1970
                    && ((after_rtc - before_rtc) - expected_duration)
                        .num_seconds()
                        .abs()
                        < 3);

            if hardware_alarm_fired {
                return self.claim_due_alarms_at(now);
            }
        }

        self.update_hardware_alarm_at(now)?;
        Ok(Vec::new())
    }

    /// Removes and returns logical alarms due on the RTC authority clock.
    ///
    /// Falls back to system time for the entire claim operation when RTC time
    /// cannot be read.
    pub fn claim_due_alarms(&mut self) -> Result<Vec<AlarmType>, Error> {
        let now = self.authority_now();
        self.claim_due_alarms_at(now)
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

        self.update_hardware_alarm_at(at)?;
        Ok(fired_types)
    }

    /// Rebases scheduled alarms after an RTC clock write and reprograms hardware.
    ///
    /// Relative and RTC-absolute intents retain their remaining RTC duration.
    /// Civil intents are converted again using the RTC's refreshed drift.
    pub fn sync(&mut self) -> Result<(), Error> {
        if let Some(step) = self.rtc.take_pending_step()? {
            for alarm in self.scheduled_alarms.values_mut() {
                alarm.wake_time = match alarm.when {
                    AlarmWhen::In(_) | AlarmWhen::At(ClockInstant::Rtc(_)) => {
                        alarm.wake_time + step
                    }
                    AlarmWhen::At(ClockInstant::Civil(civil)) => self.rtc.to_rtc(civil)?,
                };
            }
        }
        let now = self.authority_now();
        self.update_hardware_alarm_at(now)
    }

    fn authority_now(&self) -> DateTime<Utc> {
        self.rtc.read_time().unwrap_or_else(|error| {
            tracing::warn!(error = %error, "RTC read failed; using system clock");
            Utc::now()
        })
    }

    fn resolve_instant(&self, instant: ClockInstant) -> Result<DateTime<Utc>, Error> {
        match instant {
            ClockInstant::Rtc(time) => Ok(time),
            ClockInstant::Civil(civil) => self.rtc.to_rtc(civil),
        }
    }

    fn schedule_at(
        &mut self,
        alarm_type: AlarmType,
        when: AlarmWhen,
        now: DateTime<Utc>,
    ) -> Result<(), Error> {
        let wake_time = match when {
            AlarmWhen::In(duration) => now + duration,
            AlarmWhen::At(instant) => self.resolve_instant(instant)?,
        };
        tracing::debug!(
            alarm_type = ?alarm_type,
            wake_at = %wake_time,
            "Scheduled logical RTC alarm"
        );
        self.scheduled_alarms.insert(
            alarm_type,
            ScheduledAlarm {
                alarm_type,
                when,
                wake_time,
            },
        );
        Ok(())
    }

    fn update_hardware_alarm_at(&self, now: DateTime<Utc>) -> Result<(), Error> {
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

/// Sets RTC time and synchronizes logical alarms before returning.
///
/// App clock synchronization paths, including NTP, should use this helper so
/// relative and civil alarm intents remain coherent across an RTC clock step.
pub fn set_time<R: Rtc>(
    rtc: &R,
    alarms: &mut AlarmManager<R>,
    time: DateTime<Utc>,
) -> Result<(), Error> {
    rtc.set_time(time)?;
    alarms.sync()
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
    fn relative_schedule_uses_rtc_now() {
        let (rtc, mut manager) = test_alarm_manager();
        let rtc_now = Utc::now() - Duration::hours(4);
        rtc.set_current_time(rtc_now);

        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::minutes(10))
            .unwrap();

        let wake_time = manager
            .scheduled_alarms
            .get(&AlarmType::AutoSuspend)
            .unwrap()
            .wake_time;
        assert_eq!(wake_time, rtc_now + Duration::minutes(10));
        assert_eq!(rtc.scheduled_wake_time(), Some(wake_time));
    }

    #[test]
    fn rtc_lag_does_not_disable_future_hardware_alarm() {
        let (rtc, mut manager) = test_alarm_manager();
        rtc.set_current_time(Utc::now() - Duration::hours(1));

        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::minutes(10))
            .unwrap();
        let claimed = manager.claim_due_alarms().unwrap();

        assert!(claimed.is_empty());
        assert!(manager.has_alarm(AlarmType::AutoSuspend));
        assert!(rtc.alarm_enabled());
    }

    #[test]
    fn helper_preserves_relative_remaining_time() {
        let (rtc, mut manager) = test_alarm_manager();
        let old_time = Utc::now() - Duration::hours(2);
        rtc.set_current_time(old_time);
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::minutes(20))
            .unwrap();

        let new_time = old_time + Duration::hours(1);
        set_time(&rtc, &mut manager, new_time).unwrap();

        let wake_time = manager
            .scheduled_alarms
            .get(&AlarmType::AutoPowerOff)
            .unwrap()
            .wake_time;
        assert_eq!(
            wake_time.signed_duration_since(new_time),
            Duration::minutes(20)
        );
        assert_eq!(rtc.scheduled_wake_time(), Some(wake_time));
    }

    #[test]
    fn civil_alarm_rebases_with_updated_drift() {
        let (rtc, mut manager) = test_alarm_manager();
        let rtc_now = Utc::now() - Duration::hours(3);
        rtc.set_time(rtc_now).unwrap();
        rtc.take_pending_step().unwrap();
        let civil = Utc::now() + Duration::hours(2);

        manager
            .schedule(
                AlarmType::CalendarUpdate,
                AlarmWhen::At(ClockInstant::Civil(civil)),
            )
            .unwrap();
        let initial_wake = manager
            .scheduled_alarms
            .get(&AlarmType::CalendarUpdate)
            .unwrap()
            .wake_time;

        set_time(&rtc, &mut manager, rtc_now + Duration::hours(1)).unwrap();

        let wake_time = manager
            .scheduled_alarms
            .get(&AlarmType::CalendarUpdate)
            .unwrap()
            .wake_time;
        assert_ne!(wake_time, initial_wake);
        assert_eq!(wake_time, rtc.to_rtc(civil).unwrap());
        assert_eq!(rtc.to_civil(wake_time).unwrap(), civil);
    }

    #[test]
    fn read_time_does_not_refresh_drift() {
        let rtc = TestRtc::new();
        let drift = rtc.drift().unwrap();
        rtc.set_current_time(Utc::now() + Duration::days(1));

        rtc.read_time().unwrap();

        assert_eq!(rtc.drift().unwrap(), drift);
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
            .schedule_in(AlarmType::CalendarUpdate, Duration::minutes(-90))
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
            .schedule_in(AlarmType::AutoPowerOff, Duration::seconds(-10))
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
        let before = rtc.read_time().unwrap();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::minutes(5))
            .unwrap();
        rtc.simulate_alarm_fired();
        let after = before + Duration::minutes(5) + Duration::seconds(1);
        rtc.set_current_time(after);
        let fired = manager
            .check_fired_alarms(ClockInstant::Rtc(before), ClockInstant::Rtc(after))
            .unwrap();
        assert!(fired.contains(&AlarmType::AutoPowerOff));
    }

    #[test]
    fn check_fired_alarms_civil_window_under_rtc_lag() {
        let (rtc, mut manager) = test_alarm_manager();
        let civil_before = Utc::now();
        rtc.set_time(civil_before - Duration::hours(1)).unwrap();
        let _ = rtc.take_pending_step().unwrap();

        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::minutes(5))
            .unwrap();
        rtc.simulate_alarm_fired();

        let civil_after = civil_before + Duration::minutes(5) + Duration::seconds(1);
        rtc.set_current_time(rtc.to_rtc(civil_after).unwrap());

        let fired = manager
            .check_fired_alarms(
                ClockInstant::Civil(civil_before),
                ClockInstant::Civil(civil_after),
            )
            .unwrap();
        assert!(fired.contains(&AlarmType::AutoPowerOff));
    }

    #[test]
    fn check_fired_alarms_not_fired() {
        let (rtc, mut manager) = test_alarm_manager();
        let before = rtc.read_time().unwrap();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(1))
            .unwrap();
        let after = before + Duration::minutes(1);
        let fired = manager
            .check_fired_alarms(ClockInstant::Rtc(before), ClockInstant::Rtc(after))
            .unwrap();
        assert!(fired.is_empty());
        assert!(!rtc.alarm_enabled() || manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[test]
    fn multiplexing_earliest_alarm_wins() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(2))
            .unwrap();
        manager
            .schedule_in(AlarmType::CalendarUpdate, Duration::minutes(30))
            .unwrap();
        let wake = rtc.scheduled_wake_time().unwrap();
        let expected = Utc::now() + Duration::minutes(30);
        assert!((wake - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn auto_suspend_can_be_earliest_alarm() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(2))
            .unwrap();
        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::minutes(10))
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
            .schedule_in(AlarmType::AutoSuspend, Duration::seconds(-10))
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
            .schedule_in(AlarmType::AutoSuspend, Duration::seconds(2))
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
                .schedule_in(AlarmType::AutoSuspend, Duration::seconds(-10))
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

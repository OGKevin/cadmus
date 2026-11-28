//! Real-time clock and alarm management.

mod manager;
mod wake_alarm;

pub use wake_alarm::HardwareWakeAlarm;
#[cfg(any(feature = "kobo", docsrs))]
pub(crate) use wake_alarm::PreSleepAlarms;

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
use std::sync::atomic::{AtomicBool, Ordering};
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
    #[cfg(any(target_os = "linux", docsrs))]
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
    /// Resolved wake instant on the RTC timeline.
    ///
    /// This is the alarm's own instant; the hardware may hold an earlier
    /// re-arm step when the RTC's alarm limit is shorter than the wait.
    pub wake_time: DateTime<Utc>,
}

/// Multiplexes multiple logical alarms onto a single hardware RTC alarm.
///
/// The hardware RTC supports only one wake alarm at a time. `AlarmManager`
/// maintains a map of logical alarms keyed by [`AlarmType`] and always
/// programs the hardware with the earliest upcoming wake time, no further
/// ahead than the RTC's [`Rtc::alarm_limit`]. An owned IRQ
/// listener thread waits on [`Rtc::wait_for_alarm_irq`], claims due alarms via
/// [`AlarmManager::claim_due_alarms`], and delivers them through the callback
/// passed to [`AlarmManager::start_irq_listener`]. After resume,
/// [`AlarmManager::check_fired_alarms`] reuses the same claim path when the
/// hardware likely fired during sleep.
pub struct AlarmManager<R: Rtc> {
    rtc: Arc<R>,
    scheduled_alarms: BTreeMap<AlarmType, ScheduledAlarm>,
    /// Hardware wake time when the programmed alarm is a re-arm step.
    ///
    /// Set while the earliest logical alarm is further ahead than
    /// [`Rtc::alarm_limit`], in which case the slot holds `now + limit` instead
    /// of the alarm's own wake time. Cleared once the alarm fits.
    rearm_step: Option<DateTime<Utc>>,
    irq_thread: Option<tokio::task::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

/// Runs one complete async alarm transaction while holding the manager lock.
///
/// The separate runtime task protects a submitted transaction from cancellation
/// of its caller. Dropping the outer future abandons only the wait for its result,
/// not the callback's logical alarm changes or hardware reprogramming. Awaiting
/// `operation` directly would cancel those changes with the caller's future.
/// A worker join failure returns `None` for callers that tolerate unavailable RTC I/O.
pub(crate) async fn alarm_transaction<
    R: Rtc + Send + Sync + 'static,
    O: Send + 'static,
    F: FnOnce(tokio::sync::OwnedMutexGuard<AlarmManager<R>>) -> Fut + Send + 'static,
    Fut: Future<Output = O> + Send + 'static,
>(
    manager: &Arc<tokio::sync::Mutex<AlarmManager<R>>>,
    operation: F,
) -> Option<O> {
    let manager = Arc::clone(manager);
    crate::runtime::spawn(async move {
        let guard = manager.lock_owned().await;
        operation(guard).await
    })
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "RTC transaction worker failed");
    })
    .ok()
}

impl<R: Rtc> AlarmManager<R> {
    pub fn new(rtc: Arc<R>) -> Self {
        AlarmManager {
            rtc,
            scheduled_alarms: BTreeMap::new(),
            rearm_step: None,
            irq_thread: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Schedules a logical alarm with an explicit relative or absolute intent.
    ///
    /// Relative alarms use RTC time as their base. If the RTC cannot be read,
    /// the operation uses system time consistently as its fallback.
    pub async fn schedule(&mut self, alarm_type: AlarmType, when: AlarmWhen) -> Result<(), Error> {
        let now = self.authority_now().await;
        self.schedule_at(alarm_type, when, now)?;
        self.update_hardware_alarm_at(now).await
    }

    /// Schedules a logical alarm to fire after `duration` on the RTC timeline.
    pub async fn schedule_in(
        &mut self,
        alarm_type: AlarmType,
        duration: Duration,
    ) -> Result<(), Error> {
        self.schedule(alarm_type, AlarmWhen::In(duration)).await
    }

    /// Cancel a previously scheduled logical alarm.
    ///
    /// If no alarm of that type is scheduled this is a no-op. The hardware
    /// RTC is updated to reflect the new earliest remaining wake time.
    pub async fn cancel_alarm(&mut self, alarm_type: AlarmType) -> Result<(), Error> {
        let now = self.authority_now().await;
        self.scheduled_alarms.remove(&alarm_type);
        self.update_hardware_alarm_at(now).await
    }

    /// Stops the IRQ listener thread and joins it. Idempotent and safe if never started.
    /// Signals the IRQ listener to exit. Does not join; see [`shutdown_rtc`].
    pub fn stop_irq_listener(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Stops and awaits the IRQ listener task.
    pub async fn join_irq_listener(&mut self) {
        self.stop_irq_listener();
        if let Some(handle) = self.irq_thread.take() {
            let _ = handle.await;
        }
    }

    /// Clears every logical alarm and disables the hardware wake alarm.
    pub async fn clear_all_alarms(&mut self) -> Result<(), Error> {
        let cleared: Vec<AlarmType> = self.scheduled_alarms.keys().copied().collect();
        self.scheduled_alarms.clear();
        self.rtc.disable_alarm().await?;
        self.rearm_step = None;
        if cleared.is_empty() {
            tracing::debug!("Cleared RTC alarms on shutdown (none scheduled)");
        } else {
            tracing::info!(
                count = cleared.len(),
                alarm_types = ?cleared,
                "Cleared all RTC alarms on shutdown"
            );
        }
        Ok(())
    }

    /// Returns `true` if an alarm of `alarm_type` is scheduled for a future time.
    pub async fn is_alarm_scheduled(&self, alarm_type: AlarmType) -> bool {
        let now = self.authority_now().await;
        self.scheduled_alarms
            .get(&alarm_type)
            .map(|alarm| alarm.wake_time > now)
            .unwrap_or(false)
    }

    /// Returns `true` if an alarm of `alarm_type` exists in the schedule.
    pub fn has_alarm(&self, alarm_type: AlarmType) -> bool {
        self.scheduled_alarms.contains_key(&alarm_type)
    }

    /// Returns the earliest logical alarm that is still ahead, if any.
    ///
    /// A past-due alarm stays in the schedule but is not written to hardware, so
    /// it is skipped. With an RTC that reports an [`Rtc::alarm_limit`], the
    /// hardware slot may instead hold an earlier re-arm step (see
    /// [`AlarmManager::rearm_step`]).
    #[cfg(any(feature = "kobo", docsrs, test))]
    pub(crate) async fn next_scheduled_alarm(&self) -> Option<(AlarmType, DateTime<Utc>)> {
        let now = self.authority_now().await;
        self.scheduled_alarms
            .values()
            .filter(|alarm| alarm.wake_time > now)
            .min_by_key(|alarm| alarm.wake_time)
            .map(|alarm| (alarm.alarm_type, alarm.wake_time))
    }

    /// Ensures an alarm is active and scheduled for the future.
    ///
    /// Accepts either an [`AlarmWhen`] or a relative [`Duration`]. A stale
    /// alarm is handled according to `past_due_action`.
    pub async fn ensure_scheduled<W: Into<AlarmWhen>>(
        &mut self,
        alarm_type: AlarmType,
        when: W,
        past_due_action: PastDueAction,
    ) -> Result<EnsureAlarmOutcome, Error> {
        let now = self.authority_now().await;
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
        self.update_hardware_alarm_at(now).await?;
        Ok(outcome)
    }

    /// Returns the number of seconds until `alarm_type` fires, or `None` if
    /// it is not scheduled.
    pub async fn time_until_alarm(&self, alarm_type: AlarmType) -> Option<i64> {
        let now = self.authority_now().await;
        self.scheduled_alarms
            .get(&alarm_type)
            .map(|alarm| alarm.wake_time.signed_duration_since(now).num_seconds())
    }

    /// Determines which logical alarms fired during the last sleep cycle.
    ///
    /// The kernel disables the wake alarm when it delivers an RTC interrupt, so
    /// a disabled slot means the hardware fired. An RTC that raises an interrupt
    /// without the kernel delivering it (its alarm was set beyond the chip's
    /// range) is reported and re-armed by the wake classifier; it must not claim
    /// its logical alarms here.
    pub async fn check_fired_alarms(&mut self) -> Result<Vec<AlarmType>, Error> {
        let now = self.authority_now().await;
        if !self.rtc.alarm().await?.enabled() {
            return self.claim_due_alarms_at(now).await;
        }

        self.update_hardware_alarm_at(now).await?;
        Ok(Vec::new())
    }

    /// Removes and returns logical alarms due on the RTC authority clock.
    ///
    /// Falls back to system time for the entire claim operation when RTC time
    /// cannot be read.
    pub async fn claim_due_alarms(&mut self) -> Result<Vec<AlarmType>, Error> {
        let now = self.authority_now().await;
        self.claim_due_alarms_at(now).await
    }

    async fn claim_due_alarms_at(&mut self, at: DateTime<Utc>) -> Result<Vec<AlarmType>, Error> {
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

        self.update_hardware_alarm_at(at).await?;
        Ok(fired_types)
    }

    /// Rebases scheduled alarms after an RTC clock write and reprograms hardware.
    ///
    /// Relative and RTC-absolute intents retain their remaining RTC duration.
    /// Civil intents are converted again using the RTC's refreshed drift.
    pub async fn sync(&mut self) -> Result<(), Error> {
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
        let now = self.authority_now().await;
        self.update_hardware_alarm_at(now).await
    }

    async fn authority_now(&self) -> DateTime<Utc> {
        self.rtc.read_time().await.unwrap_or_else(|error| {
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

    async fn update_hardware_alarm_at(&mut self, now: DateTime<Utc>) -> Result<(), Error> {
        if let Some((alarm_type, wake_time)) = self
            .scheduled_alarms
            .values()
            .filter(|alarm| alarm.wake_time > now)
            .min_by_key(|alarm| alarm.wake_time)
            .map(|alarm| (alarm.alarm_type, alarm.wake_time))
        {
            let step = match self.rtc.alarm_limit() {
                Some(limit) => wake_time.min(now + limit),
                None => wake_time,
            };
            self.rtc.set_alarm(step).await?;
            self.rearm_step = (step < wake_time).then_some(step);
            if let Some(rearm_step) = self.rearm_step() {
                tracing::info!(
                    alarm_type = ?alarm_type,
                    requested = %wake_time,
                    programmed = %rearm_step,
                    "RTC alarm limited to its hardware range"
                );
            }
        } else {
            self.rtc.disable_alarm().await?;
            self.rearm_step = None;
        }

        Ok(())
    }

    /// Returns the hardware wake time when the programmed alarm is a re-arm step.
    ///
    /// `None` when the slot is disabled or holds the earliest logical alarm's
    /// own wake time. The wake classifier reports a wake that reaches this time
    /// as an Unattended alarm re-arm instead of an early fire.
    pub(crate) fn rearm_step(&self) -> Option<DateTime<Utc>> {
        self.rearm_step
    }
}

/// Sets RTC time and synchronizes logical alarms before returning.
///
/// App clock synchronization paths, including NTP, should use this helper so
/// relative and civil alarm intents remain coherent across an RTC clock step.
pub async fn set_time<R: Rtc>(
    rtc: &R,
    alarms: &mut AlarmManager<R>,
    time: DateTime<Utc>,
) -> Result<(), Error> {
    rtc.set_time(time).await?;
    alarms.sync().await
}

/// Stops the IRQ listener and clears all logical and hardware RTC alarms.
async fn shutdown_alarm_manager<R: Rtc>(
    alarm_manager: &Option<Arc<tokio::sync::Mutex<AlarmManager<R>>>>,
) {
    let Some(alarm_manager) = alarm_manager else {
        return;
    };
    let join_handle = {
        let mut manager = alarm_manager.lock().await;
        manager.stop_irq_listener();
        manager.irq_thread.take()
    };
    if let Some(handle) = join_handle {
        let _ = handle.await;
    }
    let mut manager = alarm_manager.lock().await;
    if let Err(error) = manager.clear_all_alarms().await {
        tracing::error!(error = %error, "Failed to clear RTC alarms on shutdown");
    }
}

/// Stops the RTC IRQ listener, clears all logical alarms, and releases hardware RTC FDs.
///
/// Invoked from the application main loop after background tasks stop and before
/// device `on_shutdown`. Failures are logged and do not abort shutdown.
pub async fn shutdown_rtc<D>(context: &crate::context::Context<D>)
where
    D: crate::device::Device,
{
    shutdown_alarm_manager(&context.alarm_manager).await;
    match context.device.rtc() {
        Ok(rtc) => {
            if let Err(error) = rtc.release() {
                tracing::error!(error = %error, "Failed to release RTC device on shutdown");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "Failed to access RTC device on shutdown");
        }
    }
}

impl<R: Rtc + 'static> AlarmManager<R> {
    /// Starts the owned IRQ listener thread once. Idempotent.
    ///
    /// `send` delivers each fired logical alarm to the hub. The join handle is
    /// stored on this manager and joined on drop after the stop flag is set.
    pub async fn start_irq_listener<F, Fut>(manager: &Arc<tokio::sync::Mutex<Self>>, send: F)
    where
        F: Fn(AlarmType) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let guard = manager.lock().await;
        if guard.irq_thread.is_some() {
            return;
        }

        let stop = Arc::clone(&guard.stop);
        let rtc = Arc::clone(&guard.rtc);
        drop(guard);

        let manager_weak = Arc::downgrade(manager);
        let handle = crate::runtime::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let irq = {
                    let rtc = Arc::clone(&rtc);
                    crate::runtime::spawn_blocking(move || {
                        rtc.wait_for_alarm_irq(Some(StdDuration::from_secs(1)))
                    })
                    .await
                };
                match irq {
                    Ok(Ok(Some(_))) => {
                        let Some(manager) = manager_weak.upgrade() else {
                            break;
                        };
                        let due = alarm_transaction(&manager, async move |mut manager| {
                            manager.claim_due_alarms().await
                        })
                        .await;
                        match due {
                            Some(Ok(alarm_types)) => {
                                for alarm_type in alarm_types {
                                    send(alarm_type).await;
                                }
                            }
                            Some(Err(error)) => {
                                tracing::error!(error = %error, "claim_due_alarms failed");
                            }
                            None => {}
                        }
                    }
                    Ok(Ok(None)) => {
                        tracing::trace!("RTC alarm wait returned with no IRQ data");
                    }
                    Ok(Err(error)) => {
                        tracing::error!(error = %error, "wait_for_alarm_irq failed");
                        tokio::time::sleep(StdDuration::from_secs(1)).await;
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "RTC IRQ wait worker failed");
                        tokio::time::sleep(StdDuration::from_secs(1)).await;
                    }
                }
            }
        });

        manager.lock().await.irq_thread = Some(handle);
    }
}

impl<R: Rtc> Drop for AlarmManager<R> {
    fn drop(&mut self) {
        self.stop_irq_listener();
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

    #[tokio::test]
    async fn relative_schedule_uses_rtc_now() {
        let (rtc, mut manager) = test_alarm_manager();
        let rtc_now = Utc::now() - Duration::hours(4);
        rtc.set_current_time(rtc_now);

        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::minutes(10))
            .await
            .unwrap();

        let wake_time = manager
            .scheduled_alarms
            .get(&AlarmType::AutoSuspend)
            .unwrap()
            .wake_time;
        assert_eq!(wake_time, rtc_now + Duration::minutes(10));
        assert_eq!(rtc.scheduled_wake_time(), Some(wake_time));
    }

    #[tokio::test]
    async fn rtc_lag_does_not_disable_future_hardware_alarm() {
        let (rtc, mut manager) = test_alarm_manager();
        rtc.set_current_time(Utc::now() - Duration::hours(1));

        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::minutes(10))
            .await
            .unwrap();
        let claimed = manager.claim_due_alarms().await.unwrap();

        assert!(claimed.is_empty());
        assert!(manager.has_alarm(AlarmType::AutoSuspend));
        assert!(rtc.alarm_enabled());
    }

    #[tokio::test]
    async fn helper_preserves_relative_remaining_time() {
        let (rtc, mut manager) = test_alarm_manager();
        let old_time = Utc::now() - Duration::hours(2);
        rtc.set_current_time(old_time);
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::minutes(20))
            .await
            .unwrap();

        let new_time = old_time + Duration::hours(1);
        set_time(&rtc, &mut manager, new_time).await.unwrap();

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

    #[tokio::test]
    async fn civil_alarm_rebases_with_updated_drift() {
        let (rtc, mut manager) = test_alarm_manager();
        let rtc_now = Utc::now() - Duration::hours(3);
        rtc.set_time(rtc_now).await.unwrap();
        rtc.take_pending_step().unwrap();
        let civil = Utc::now() + Duration::hours(2);

        manager
            .schedule(
                AlarmType::CalendarUpdate,
                AlarmWhen::At(ClockInstant::Civil(civil)),
            )
            .await
            .unwrap();
        let initial_wake = manager
            .scheduled_alarms
            .get(&AlarmType::CalendarUpdate)
            .unwrap()
            .wake_time;

        set_time(&rtc, &mut manager, rtc_now + Duration::hours(1))
            .await
            .unwrap();

        let wake_time = manager
            .scheduled_alarms
            .get(&AlarmType::CalendarUpdate)
            .unwrap()
            .wake_time;
        assert_ne!(wake_time, initial_wake);
        assert_eq!(wake_time, rtc.to_rtc(civil).unwrap());
        assert_eq!(rtc.to_civil(wake_time).unwrap(), civil);
    }

    #[tokio::test]
    async fn read_time_does_not_refresh_drift() {
        let rtc = TestRtc::new();
        let drift = rtc.drift().unwrap();
        rtc.set_current_time(Utc::now() + Duration::days(1));

        rtc.read_time().await.unwrap();

        assert_eq!(rtc.drift().unwrap(), drift);
    }

    #[tokio::test]
    async fn ensure_scheduled_fresh() {
        let (_rtc, mut manager) = test_alarm_manager();
        let outcome = manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .await
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::Scheduled);
        assert!(manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[tokio::test]
    async fn ensure_scheduled_already_scheduled() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .await
            .unwrap();
        let outcome = manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .await
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::AlreadyScheduled);
    }

    #[tokio::test]
    async fn ensure_scheduled_past_due_reschedule() {
        let (rtc, mut manager) = test_alarm_manager();
        let past = Utc::now() - Duration::hours(2);
        rtc.set_current_time(past + Duration::minutes(30));
        manager
            .schedule_in(AlarmType::CalendarUpdate, Duration::minutes(-90))
            .await
            .unwrap();
        rtc.set_current_time(Utc::now());
        let outcome = manager
            .ensure_scheduled(
                AlarmType::CalendarUpdate,
                Duration::minutes(5),
                PastDueAction::Reschedule,
            )
            .await
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::Scheduled);
        assert!(manager.is_alarm_scheduled(AlarmType::CalendarUpdate).await);
    }

    #[tokio::test]
    async fn ensure_scheduled_past_due_cancel() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::seconds(-10))
            .await
            .unwrap();
        rtc.set_current_time(Utc::now());
        let outcome = manager
            .ensure_scheduled(
                AlarmType::AutoPowerOff,
                Duration::hours(1),
                PastDueAction::Cancel,
            )
            .await
            .unwrap();
        assert_eq!(outcome, EnsureAlarmOutcome::PastDue);
        assert!(!manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[tokio::test]
    async fn check_fired_alarms_detects_fired() {
        let (rtc, mut manager) = test_alarm_manager();
        let before = rtc.read_time().await.unwrap();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::minutes(5))
            .await
            .unwrap();
        rtc.simulate_alarm_fired();
        rtc.set_current_time(before + Duration::minutes(5) + Duration::seconds(1));
        let fired = manager.check_fired_alarms().await.unwrap();
        assert!(fired.contains(&AlarmType::AutoPowerOff));
    }

    #[tokio::test]
    async fn check_fired_alarms_not_fired() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(1))
            .await
            .unwrap();
        let fired = manager.check_fired_alarms().await.unwrap();
        assert!(fired.is_empty());
        assert!(rtc.alarm_enabled());
        assert!(manager.has_alarm(AlarmType::AutoPowerOff));
    }

    #[tokio::test]
    async fn multiplexing_earliest_alarm_wins() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(2))
            .await
            .unwrap();
        manager
            .schedule_in(AlarmType::CalendarUpdate, Duration::minutes(30))
            .await
            .unwrap();
        let wake = rtc.scheduled_wake_time().unwrap();
        let expected = Utc::now() + Duration::minutes(30);
        assert!((wake - expected).num_seconds().abs() < 2);
    }

    #[tokio::test]
    async fn next_scheduled_alarm_is_none_when_empty() {
        let (_rtc, manager) = test_alarm_manager();
        assert_eq!(manager.next_scheduled_alarm().await, None);
    }

    #[tokio::test]
    async fn next_scheduled_alarm_returns_earliest_and_matches_hardware() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(2))
            .await
            .unwrap();
        manager
            .schedule_in(AlarmType::CalendarUpdate, Duration::minutes(30))
            .await
            .unwrap();
        let (alarm_type, wake) = manager.next_scheduled_alarm().await.unwrap();
        assert_eq!(alarm_type, AlarmType::CalendarUpdate);
        assert_eq!(Some(wake), rtc.scheduled_wake_time());
    }

    #[tokio::test]
    async fn next_scheduled_alarm_skips_past_due_and_matches_hardware() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::seconds(-10))
            .await
            .unwrap();
        manager
            .schedule_in(AlarmType::CalendarUpdate, Duration::minutes(30))
            .await
            .unwrap();
        let (alarm_type, wake) = manager.next_scheduled_alarm().await.unwrap();
        assert_eq!(alarm_type, AlarmType::CalendarUpdate);
        assert_eq!(Some(wake), rtc.scheduled_wake_time());
    }

    #[tokio::test]
    async fn next_scheduled_alarm_is_none_when_only_past_due_remains() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::seconds(-10))
            .await
            .unwrap();
        assert_eq!(manager.next_scheduled_alarm().await, None);
        assert!(!rtc.alarm_enabled());
    }

    #[tokio::test]
    async fn auto_suspend_can_be_earliest_alarm() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(2))
            .await
            .unwrap();
        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::minutes(10))
            .await
            .unwrap();
        let wake = rtc.scheduled_wake_time().unwrap();
        let expected = Utc::now() + Duration::minutes(10);
        assert!((wake - expected).num_seconds().abs() < 2);
        assert!(manager.is_alarm_scheduled(AlarmType::AutoSuspend).await);
    }

    #[tokio::test]
    async fn auto_suspend_not_in_cancel_after_resume_list() {
        assert!(
            !AlarmType::alarms_to_cancel_after_resume()
                .into_iter()
                .any(|a| a == AlarmType::AutoSuspend)
        );
    }

    #[tokio::test]
    async fn suspend_alarm_not_in_cancel_after_resume_list() {
        assert!(
            !AlarmType::alarms_to_cancel_after_resume()
                .into_iter()
                .any(|a| a == AlarmType::Suspend)
        );
    }

    #[tokio::test]
    async fn wake_debounce_not_in_cancel_after_resume_list() {
        assert!(
            !AlarmType::alarms_to_cancel_after_resume()
                .into_iter()
                .any(|a| a == AlarmType::WakeDebounce)
        );
    }

    #[tokio::test]
    async fn claim_due_alarms_returns_past_due_once() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::seconds(-10))
            .await
            .unwrap();
        let first = manager.claim_due_alarms().await.unwrap();
        assert_eq!(first, vec![AlarmType::AutoSuspend]);
        let second = manager.claim_due_alarms().await.unwrap();
        assert!(second.is_empty());
        assert!(!manager.has_alarm(AlarmType::AutoSuspend));
    }

    #[tokio::test]
    async fn claim_due_alarms_ignores_near_future_wake() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::AutoSuspend, Duration::seconds(2))
            .await
            .unwrap();
        let claimed = manager.claim_due_alarms().await.unwrap();
        assert!(claimed.is_empty());
        assert!(manager.is_alarm_scheduled(AlarmType::AutoSuspend).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn irq_listener_claims_past_due_on_simulated_irq() {
        use std::sync::{Arc, Mutex};

        let (rtc, manager) = test_alarm_manager();
        let manager = Arc::new(tokio::sync::Mutex::new(manager));
        {
            let mut locked = manager.lock().await;
            locked
                .schedule_in(AlarmType::AutoSuspend, Duration::seconds(-10))
                .await
                .unwrap();
        }

        let claimed = Arc::new(Mutex::new(Vec::new()));
        let claimed_for_send = Arc::clone(&claimed);
        AlarmManager::start_irq_listener(&manager, move |alarm_type| {
            let claimed_for_send = Arc::clone(&claimed_for_send);
            async move {
                claimed_for_send.lock().unwrap().push(alarm_type);
            }
        })
        .await;

        tokio::time::sleep(StdDuration::from_millis(50)).await;
        rtc.simulate_alarm_fired();

        let deadline = std::time::Instant::now() + StdDuration::from_secs(3);
        while claimed.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(StdDuration::from_millis(50)).await;
        }

        assert_eq!(*claimed.lock().unwrap(), vec![AlarmType::AutoSuspend]);
        assert!(!manager.lock().await.has_alarm(AlarmType::AutoSuspend));
        manager.lock().await.stop_irq_listener();
    }

    #[tokio::test]
    async fn wait_for_alarm_irq_wakes_on_simulate() {
        let (rtc, _manager) = test_alarm_manager();
        let rtc_waiter = rtc.clone();
        let handle = crate::runtime::spawn_blocking(move || {
            rtc_waiter
                .wait_for_alarm_irq(Some(StdDuration::from_secs(2)))
                .unwrap()
        });
        tokio::time::sleep(StdDuration::from_millis(50)).await;
        rtc.simulate_alarm_fired();
        let data = handle.await.unwrap();
        assert_eq!(data, Some(0x20));
    }

    #[tokio::test]
    async fn clear_all_alarms_empties_schedule_and_disables_hardware() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::WakeDebounce, Duration::minutes(1))
            .await
            .unwrap();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(1))
            .await
            .unwrap();
        assert!(rtc.alarm_enabled());

        manager.clear_all_alarms().await.unwrap();

        assert!(manager.scheduled_alarms.is_empty());
        assert!(!rtc.alarm_enabled());
    }

    #[tokio::test]
    async fn clear_all_alarms_is_idempotent_when_empty() {
        let (rtc, mut manager) = test_alarm_manager();
        manager.clear_all_alarms().await.unwrap();
        manager.clear_all_alarms().await.unwrap();
        assert!(manager.scheduled_alarms.is_empty());
        assert!(!rtc.alarm_enabled());
    }

    #[tokio::test]
    async fn clear_all_alarms_clears_logical_schedule_when_disable_fails() {
        let (rtc, mut manager) = test_alarm_manager();
        manager
            .schedule_in(AlarmType::WakeDebounce, Duration::minutes(1))
            .await
            .unwrap();
        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(1))
            .await
            .unwrap();
        rtc.set_fail_disable(true);

        assert!(manager.clear_all_alarms().await.is_err());

        assert!(manager.scheduled_alarms.is_empty());
        assert!(!manager.has_alarm(AlarmType::WakeDebounce));
        assert!(!manager.has_alarm(AlarmType::AutoPowerOff));
        assert!(
            rtc.alarm_enabled(),
            "hardware alarm should stay armed when disable_alarm fails"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_rtc_clears_logical_alarms_when_disable_fails() {
        use crate::context::test_helpers::create_test_context;
        use crate::device::DeviceHardware as _;

        let context = create_test_context().await;
        let rtc = context.device.rtc().unwrap();
        {
            let mut manager = context.alarm_manager.as_ref().unwrap().lock().await;
            manager
                .schedule_in(AlarmType::WakeDebounce, Duration::minutes(1))
                .await
                .unwrap();
        }
        rtc.set_fail_disable(true);

        shutdown_rtc(&context).await;

        let manager = context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(!manager.has_alarm(AlarmType::WakeDebounce));
        assert!(
            rtc.alarm_enabled(),
            "hardware alarm should stay armed when disable_alarm fails"
        );
    }

    /// A submitted transaction finishes even when its caller drops the join handle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn alarm_transaction_completes_after_caller_drop() {
        let (_rtc, manager) = test_alarm_manager();
        let manager = std::sync::Arc::new(tokio::sync::Mutex::new(manager));
        manager
            .lock()
            .await
            .schedule_in(AlarmType::Suspend, Duration::minutes(1))
            .await
            .unwrap();

        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let finish = std::sync::Arc::new(tokio::sync::Notify::new());
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let started_in_task = std::sync::Arc::clone(&started);
        let finish_in_task = std::sync::Arc::clone(&finish);
        let mut transaction = Box::pin(alarm_transaction(&manager, async move |manager| {
            started_in_task.notify_one();
            finish_in_task.notified().await;
            let _ = done_tx.send(manager.has_alarm(AlarmType::Suspend));
        }));

        tokio::select! {
            _ = started.notified() => {},
            _ = &mut transaction => {
                panic!("transaction finished before caller dropped it");
            }
        }
        drop(transaction);
        finish.notify_one();

        let result = tokio::time::timeout(StdDuration::from_secs(1), done_rx)
            .await
            .expect("callback did not finish")
            .expect("callback did not send its result");
        assert!(result);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_irq_listener_is_idempotent() {
        let (_rtc, mut manager) = test_alarm_manager();
        manager.stop_irq_listener();
        manager.stop_irq_listener();
    }

    fn fixed_base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 1, 6, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn unlimited_time_of_day_rtc_fires_a_long_alarm_a_day_early() {
        let (rtc, mut manager) = test_alarm_manager();
        let base = fixed_base();
        rtc.set_current_time(base);
        rtc.set_time_of_day_alarms(true);

        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(36))
            .await
            .unwrap();

        assert_eq!(rtc.scheduled_wake_time(), Some(base + Duration::hours(36)));
        assert_eq!(rtc.hardware_fire_time(), Some(base + Duration::hours(12)));
        assert_eq!(manager.rearm_step(), None);

        let early_wake = base + Duration::hours(12) + Duration::seconds(1);
        rtc.set_current_time(early_wake);
        rtc.simulate_alarm_fired();
        let fired = manager.check_fired_alarms().await.unwrap();
        assert!(fired.is_empty());
    }

    #[tokio::test]
    async fn long_alarm_rearms_once_then_fires_on_time() {
        let (rtc, mut manager) = test_alarm_manager();
        let base = fixed_base();
        rtc.set_current_time(base);
        rtc.set_time_of_day_alarms(true);
        rtc.set_alarm_limit(Some(Duration::hours(23)));

        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(36))
            .await
            .unwrap();
        let step = base + Duration::hours(23);
        assert_eq!(rtc.scheduled_wake_time(), Some(step));
        assert_eq!(rtc.hardware_fire_time(), Some(step));
        assert_eq!(manager.rearm_step(), Some(step));

        let step_wake = step + Duration::seconds(1);
        rtc.set_current_time(step_wake);
        rtc.simulate_alarm_fired();
        let fired = manager.check_fired_alarms().await.unwrap();
        assert!(fired.is_empty());
        assert_eq!(rtc.scheduled_wake_time(), Some(base + Duration::hours(36)));
        assert_eq!(rtc.hardware_fire_time(), Some(base + Duration::hours(36)));
        assert_eq!(manager.rearm_step(), None);

        let due = base + Duration::hours(36) + Duration::seconds(1);
        rtc.set_current_time(due);
        rtc.simulate_alarm_fired();
        let fired = manager.check_fired_alarms().await.unwrap();
        assert_eq!(fired, vec![AlarmType::AutoPowerOff]);
        assert!(!rtc.alarm_enabled());
    }

    #[tokio::test]
    async fn alarm_within_limit_gets_no_rearm_step() {
        let (rtc, mut manager) = test_alarm_manager();
        let base = fixed_base();
        rtc.set_current_time(base);
        rtc.set_time_of_day_alarms(true);
        rtc.set_alarm_limit(Some(Duration::hours(23)));

        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(13))
            .await
            .unwrap();

        assert_eq!(rtc.scheduled_wake_time(), Some(base + Duration::hours(13)));
        assert_eq!(rtc.hardware_fire_time(), Some(base + Duration::hours(13)));
        assert_eq!(manager.rearm_step(), None);
    }

    #[tokio::test]
    async fn rtc_without_limit_programs_requested_time() {
        let (rtc, mut manager) = test_alarm_manager();
        let base = fixed_base();
        rtc.set_current_time(base);

        manager
            .schedule_in(AlarmType::AutoPowerOff, Duration::hours(36))
            .await
            .unwrap();

        assert_eq!(rtc.scheduled_wake_time(), Some(base + Duration::hours(36)));
        assert_eq!(rtc.hardware_fire_time(), Some(base + Duration::hours(36)));
        assert_eq!(manager.rearm_step(), None);
    }
}

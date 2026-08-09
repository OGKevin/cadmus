//! Suspend preparation, sleep/wake, post-wake alarm handling, and auto-suspend.
//!
//! # Auto Suspend via RTC
//!
//! Auto Suspend is scheduled as [`AlarmType::AutoSuspend`] on the device RTC
//! through [`AlarmManager`]. Activity calls [`reschedule_auto_suspend_alarm`];
//! when the IRQ listener claims the alarm it emits [`Event::RtcAlarmFired`],
//! which starts [`begin_suspend`]. Monotonic idle (`Instant::elapsed`) is not
//! authoritative: soft sleep does not advance it, so the wall-clock RTC
//! deadline is the idle source of truth.

use super::helpers::is_suspend_active;
use super::{SUSPEND_WAIT_DELAY, begin_suspend, show_power_off_intermission};
use crate::AlarmType;
use crate::chrono::{Duration as ChronoDuration, Local, Timelike};
use crate::device::DeviceHardware as _;
use crate::device::power::PowerManager;
use crate::device::rtc::{EnsureAlarmOutcome, PastDueAction};
use crate::device::{AppContext, DeviceRuntime, DeviceTaskId, EventOutcome, ExitStatus};
use crate::framebuffer::Framebuffer as _;
use crate::frontlight::Frontlight as _;
use crate::settings::IntermKind;
use crate::view::common::locate;
use crate::view::intermission::Intermission;
use crate::view::{Event, Hub, RenderData, RenderQueue, View, wait_for_all};

/// Converts Auto Power Off days to a chrono duration of at least one second.
fn auto_power_off_chrono_duration(days: f32) -> ChronoDuration {
    let secs = (days * 86_400.0).max(0.0);
    let duration = ChronoDuration::from_std(std::time::Duration::from_secs_f32(secs))
        .unwrap_or_else(|_| ChronoDuration::milliseconds(((secs * 1000.0) as i64).max(1)));
    if duration.num_seconds() < 1 {
        ChronoDuration::seconds(1)
    } else {
        duration
    }
}

/// Schedules or cancels [`AlarmType::AutoSuspend`] from the Auto Suspend setting.
///
/// When `auto_suspend` is `0`, cancels any pending alarm. Otherwise replaces the
/// alarm with a wake time of *now + timeout* (setting is minutes). Call on
/// startup, user activity, settings Submit, and when returning to interactive use
/// after suspend cancel so the idle window restarts from the current moment.
pub(super) fn reschedule_auto_suspend_alarm(context: &mut AppContext) {
    context.reschedule_auto_suspend_alarm();
}

/// Cancels [`AlarmType::AutoSuspend`] when entering an explicit suspend cycle.
pub(super) fn cancel_auto_suspend_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = alarm_manager.cancel_alarm(AlarmType::AutoSuspend) {
        tracing::error!(error = %error, "failed to cancel AutoSuspend alarm");
    }
}

/// Schedules [`AlarmType::WakeDebounce`] for [`super::SUSPEND_WAIT_DELAY`] after leave sleep.
pub(super) fn schedule_wake_debounce_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    let duration = ChronoDuration::from_std(super::SUSPEND_WAIT_DELAY)
        .unwrap_or_else(|_| ChronoDuration::seconds(15));
    if let Err(error) = alarm_manager.schedule_alarm(AlarmType::WakeDebounce, duration) {
        tracing::error!(error = %error, "failed to schedule WakeDebounce alarm");
    }
}

/// Cancels [`AlarmType::WakeDebounce`] when staying interactive or entering suspend.
pub(super) fn cancel_wake_debounce_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = alarm_manager.cancel_alarm(AlarmType::WakeDebounce) {
        tracing::error!(error = %error, "failed to cancel WakeDebounce alarm");
    }
}

/// Returns whether [`AlarmType::WakeDebounce`] is armed in the alarm map.
///
/// True for future and past-due entries until the alarm is cancelled or claimed.
pub(super) fn is_wake_debounce_scheduled(context: &AppContext) -> bool {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return false;
    };
    let alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    alarm_manager.has_alarm(AlarmType::WakeDebounce)
}

/// Schedules [`AlarmType::Suspend`] after PrepareSuspend (classic enter-sleep delay).
pub(super) fn schedule_suspend_alarm(context: &mut AppContext, delay: std::time::Duration) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    let duration = ChronoDuration::from_std(delay).unwrap_or_else(|_| ChronoDuration::seconds(15));
    if let Err(error) = alarm_manager.schedule_alarm(AlarmType::Suspend, duration) {
        tracing::error!(error = %error, "failed to schedule Suspend alarm");
    }
}

/// Cancels [`AlarmType::Suspend`] when aborting prepare→sleep or entering a new cycle.
pub(super) fn cancel_suspend_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = alarm_manager.cancel_alarm(AlarmType::Suspend) {
        tracing::error!(error = %error, "failed to cancel Suspend alarm");
    }
}

/// Returns whether [`AlarmType::Suspend`] is armed in the alarm map.
///
/// True for future and past-due entries until the alarm is cancelled or claimed.
pub(super) fn is_suspend_alarm_scheduled(context: &AppContext) -> bool {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return false;
    };
    let alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    alarm_manager.has_alarm(AlarmType::Suspend)
}

/// Returns whether Suspend or WakeDebounce RTC is armed in the alarm map.
pub(super) fn is_suspend_rtc_pending(context: &AppContext) -> bool {
    is_suspend_alarm_scheduled(context) || is_wake_debounce_scheduled(context)
}

/// Cancels both Suspend and WakeDebounce RTCs.
pub(super) fn cancel_suspend_rtcs(context: &mut AppContext) {
    cancel_suspend_alarm(context);
    cancel_wake_debounce_alarm(context);
}

/// Dispatches suspend-related lifecycle events.
pub(super) fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::PrepareSuspend => handle_prepare_suspend(hub, rq, context, runtime),
        Event::Suspend => handle_suspend(hub, bus, rq, context, runtime),
        Event::RtcAlarmFired(alarm_type) => {
            handle_rtc_alarm_fired(*alarm_type, hub, bus, rq, context, runtime)
        }
        _ => EventOutcome::Unhandled,
    }
}

/// Handles a claimed RTC logical alarm delivered by the IRQ listener.
fn handle_rtc_alarm_fired(
    alarm_type: AlarmType,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match alarm_type {
        AlarmType::AutoSuspend => handle_auto_suspend_fired(hub, bus, rq, context, runtime),
        AlarmType::Suspend => handle_suspend(hub, bus, rq, context, runtime),
        AlarmType::WakeDebounce => handle_wake_debounce_fired(hub, bus, rq, context, runtime),
        AlarmType::AutoPowerOff => {
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            );
            EventOutcome::Exit(ExitStatus::PowerOff)
        }
        AlarmType::CalendarUpdate => {
            refresh_calendar_intermission(rq, context, runtime);
            EventOutcome::Handled
        }
    }
}

/// Starts suspend when [`AlarmType::AutoSuspend`] fires while interactive.
///
/// When USB share is active or a suspend cycle is already pending, pushes the
/// deadline forward instead of suspending.
fn handle_auto_suspend_fired(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.settings.auto_suspend <= 0.0 {
        return EventOutcome::Handled;
    }

    if context.shared || is_suspend_active(context, runtime.tasks) {
        reschedule_auto_suspend_alarm(context);
        return EventOutcome::Handled;
    }

    begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    EventOutcome::Handled
}

/// Re-enters sleep when [`AlarmType::WakeDebounce`] fires after a brief wake.
///
/// Clears the fired alarm first (IRQ claim already removed it in production;
/// synthetic test events may not). Reuses the existing intermission via
/// [`handle_suspend`] instead of stacking another [`begin_suspend`].
fn handle_wake_debounce_fired(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    cancel_wake_debounce_alarm(context);
    if context.shared {
        return EventOutcome::Handled;
    }
    handle_suspend(hub, bus, rq, context, runtime)
}

fn refresh_calendar_intermission(
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) {
    if context.settings.intermissions[IntermKind::Suspend]
        != crate::settings::IntermissionDisplay::Calendar
    {
        return;
    }
    tracing::debug!("CalendarUpdate alarm fired; refreshing calendar intermission");
    if let Some(index) = locate::<Intermission>(runtime.view.as_mut()) {
        runtime.view.children_mut().remove(index);
        tracing::debug!("old calendar intermission removed");
    }
    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        IntermKind::Suspend,
        context,
    );
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        crate::framebuffer::UpdateMode::Full,
    ));
    runtime.view.children_mut().push(Box::new(interm));
}

/// Handles [`Event::PrepareSuspend`]: persists state and schedules full suspend.
///
/// Clears the prepare task, saves settings, turns off frontlight and WiFi,
/// then schedules [`Event::Suspend`] after [`super::SUSPEND_WAIT_DELAY`].
fn handle_prepare_suspend(
    _hub: &Hub,
    _rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    runtime
        .tasks
        .retain(|task| task.id != DeviceTaskId::PrepareSuspend);
    wait_for_all(runtime.updating, context);
    if let Some(settings_manager) = runtime.settings_manager {
        settings_manager
            .save(&context.settings)
            .map_err(|error| tracing::error!(error = %error, "Can't save settings"))
            .ok();
    }

    if context.settings.frontlight {
        context.settings.frontlight_levels = context.device.frontlight().levels();
        if let Err(error) = context.device.frontlight_mut().turn_off() {
            tracing::error!(error = %error, "failed to turn off frontlight for suspend");
        }
    }
    if context.settings.wifi != crate::settings::WifiMode::Off {
        if let Err(error) = context.wifi_session.disable_radio() {
            tracing::error!(error = %error, "Failed to disable WiFi on suspend");
        }
        context.online = false;
    }
    schedule_suspend_alarm(context, SUSPEND_WAIT_DELAY);

    EventOutcome::Handled
}

/// Handles [`Event::Suspend`]: schedules alarms, sleeps, and processes wake events.
///
/// A late [`AlarmType::Suspend`] after user cancel has no intermission; treat that
/// as stale and skip hardware sleep.
fn handle_suspend(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    cancel_suspend_alarm(context);
    if locate::<Intermission>(runtime.view.as_ref()).is_none() {
        return EventOutcome::Handled;
    }

    if let Some(outcome) = schedule_alarms_before_sleep(context, runtime) {
        return outcome;
    }

    let (before, after) = perform_suspend_resume(hub, context, runtime);
    handle_post_wake(before, after, hub, bus, rq, context, runtime)
}

/// Schedules auto-power-off and calendar-update alarms before sleep.
///
/// Returns [`EventOutcome::Exit(ExitStatus::PowerOff)`] when a past-due
/// auto-power-off alarm is detected.
fn schedule_alarms_before_sleep(
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Option<EventOutcome> {
    let alarm_manager = context.alarm_manager.as_ref()?;
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());

    if context.settings.auto_power_off > 0.0 {
        let duration = auto_power_off_chrono_duration(context.settings.auto_power_off);
        match alarm_manager.ensure_scheduled(
            AlarmType::AutoPowerOff,
            duration,
            PastDueAction::Cancel,
        ) {
            Ok(EnsureAlarmOutcome::PastDue) => {
                tracing::info!("AutoPowerOff alarm is past due, powering off");
                drop(alarm_manager);
                show_power_off_intermission(
                    context,
                    runtime.view.as_mut(),
                    runtime.history,
                    runtime.updating,
                );
                return Some(EventOutcome::Exit(ExitStatus::PowerOff));
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(error = %error, "Can't schedule auto power off alarm")
            }
        }
    }

    if context.settings.intermissions[IntermKind::Suspend]
        == crate::settings::IntermissionDisplay::Calendar
    {
        let now = Local::now();
        let seconds_into_current_5min = (now.minute() as i64 % 5) * 60 + now.second() as i64;
        let seconds_until_next_5min = 300 - seconds_into_current_5min + 1;
        alarm_manager
            .ensure_scheduled(
                AlarmType::CalendarUpdate,
                ChronoDuration::seconds(seconds_until_next_5min),
                PastDueAction::Reschedule,
            )
            .map_err(
                |error| tracing::error!(error = %error, "Can't schedule calendar update alarm"),
            )
            .ok();
    }

    None
}

/// Suspends and resumes the device, then schedules wake-debounce RTC.
fn perform_suspend_resume(
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> (chrono::DateTime<Local>, chrono::DateTime<Local>) {
    let before = Local::now();
    tracing::info!(
        "{}",
        before.format("Went to sleep on %B %-d, %Y at %H:%M:%S.")
    );
    match context.device.power_manager() {
        Ok(power) => {
            if let Err(error) = power.suspend() {
                tracing::error!(error = %error, "Failed to suspend device");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "power_manager() initialization failed for suspend");
        }
    }
    let after = Local::now();
    tracing::info!("{}", after.format("Woke up on %B %-d, %Y at %H:%M:%S."));
    match context.device.power_manager() {
        Ok(power) => {
            if let Err(error) = power.resume() {
                tracing::error!(error = %error, "Failed to resume device");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "power_manager() initialization failed for resume");
        }
    }
    let pending_task_ids: Vec<_> = runtime.tasks.iter().map(|t| t.id).collect();
    tracing::debug!(pending_tasks = ?pending_task_ids, "task state after wake");
    let _ = hub;
    schedule_wake_debounce_alarm(context);
    (before, after)
}

/// Processes fired RTC alarms after wake and refreshes the calendar intermission.
fn handle_post_wake(
    before: chrono::DateTime<Local>,
    after: chrono::DateTime<Local>,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if let Some(alarm_manager) = context.alarm_manager.as_ref() {
        let fired_alarms = {
            let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
            match alarm_manager.check_fired_alarms(before.to_utc(), after.to_utc()) {
                Ok(fired) => {
                    tracing::info!(alarms = ?fired, "Checked fired alarms after wake");
                    fired
                }
                Err(error) => {
                    tracing::error!(error = %error, "Error checking fired alarms");
                    Vec::new()
                }
            }
        };
        if fired_alarms.contains(&AlarmType::AutoPowerOff) {
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            );
            return EventOutcome::Exit(ExitStatus::PowerOff);
        }
        if fired_alarms.contains(&AlarmType::WakeDebounce) {
            return handle_wake_debounce_fired(hub, bus, rq, context, runtime);
        }
        if fired_alarms.contains(&AlarmType::CalendarUpdate) {
            refresh_calendar_intermission(rq, context, runtime);
        }
    }

    EventOutcome::Handled
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::helpers::{cancel_suspend_if_pending, has_task};
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::device::rtc::AlarmManager;
    use crate::frontlight::{Frontlight, LightLevel};
    use crate::settings::IntermissionDisplay;
    use std::time::Duration;

    fn lock_alarms(
        harness: &mut LifecycleHarness,
    ) -> std::sync::MutexGuard<'_, AlarmManager<crate::device::rtc::TestRtc>> {
        harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
    }

    #[test]
    fn handle_prepare_suspend_schedules_suspend_rtc() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        harness.context.settings.wifi = crate::settings::WifiMode::AlwaysOn;
        harness.context.online = true;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::Suspend));
        assert!(!harness.context.online);
        assert!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .was_disable_called()
        );
    }

    #[test]
    fn handle_prepare_suspend_turns_off_frontlight() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.frontlight = true;
        harness
            .context
            .device
            .frontlight_mut()
            .set_intensity(50.0.into())
            .unwrap();
        harness
            .context
            .device
            .frontlight_mut()
            .set_warmth(30.0.into())
            .unwrap();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        let levels = harness.context.device.frontlight().levels();
        assert_eq!(levels.intensity, LightLevel::off());
        assert_eq!(levels.warmth, LightLevel::off());
    }

    #[test]
    fn schedule_alarms_past_due_auto_power_off_exits() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_power_off = 1.0;
        lock_alarms(&mut harness)
            .schedule_alarm(AlarmType::AutoPowerOff, ChronoDuration::seconds(-10))
            .unwrap();
        let outcome = harness.with_runtime_only(schedule_alarms_before_sleep);
        assert_eq!(outcome, Some(EventOutcome::Exit(ExitStatus::PowerOff)));
    }

    #[test]
    fn schedule_alarms_calendar_when_intermission_calendar() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
        let outcome = harness.with_runtime_only(schedule_alarms_before_sleep);
        assert!(outcome.is_none());
        assert!(lock_alarms(&mut harness).has_alarm(AlarmType::CalendarUpdate));
    }

    #[test]
    fn handle_post_wake_auto_power_off_exit() {
        let mut harness = LifecycleHarness::new();
        let before = Local::now();
        {
            lock_alarms(&mut harness)
                .schedule_alarm(AlarmType::AutoPowerOff, ChronoDuration::minutes(5))
                .unwrap();
            if let Ok(rtc) = harness.context.device.rtc() {
                rtc.simulate_alarm_fired();
            }
        }
        let after = before + ChronoDuration::minutes(5) + ChronoDuration::seconds(1);
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_post_wake(before, after, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
    }

    #[test]
    fn perform_suspend_resume_schedules_wake_debounce() {
        let mut harness = LifecycleHarness::new();
        let (_before, _after) = harness.with_parts(|hub, _bus, _rq, context, runtime| {
            perform_suspend_resume(hub, context, runtime)
        });
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
        let power = harness.context.device.power_manager_for_test();
        assert!(power.was_suspend_called());
        assert!(power.was_resume_called());
        assert_eq!(power.suspend_call_count(), 1);
        assert_eq!(power.resume_call_count(), 1);
    }

    #[test]
    fn wake_debounce_reenters_sleep_without_begin_suspend() {
        use crate::view::common::locate;
        use crate::view::intermission::Intermission;

        let mut harness = LifecycleHarness::new();
        harness.with_parts(|hub, bus, rq, context, runtime| {
            begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
        });
        harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime);
        });
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::RtcAlarmFired(AlarmType::Suspend),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
        assert_eq!(
            harness
                .context
                .device
                .power_manager_for_test()
                .suspend_call_count(),
            1
        );

        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::RtcAlarmFired(AlarmType::WakeDebounce),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert_eq!(
            harness
                .view
                .children()
                .iter()
                .filter(|child| child.as_ref().is::<Intermission>())
                .count(),
            1
        );
        assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
        assert_eq!(
            harness
                .context
                .device
                .power_manager_for_test()
                .suspend_call_count(),
            2
        );
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    }

    #[test]
    fn handle_rtc_auto_suspend_future_noop_when_not_fired_via_event() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 5.0;
        reschedule_auto_suspend_alarm(&mut harness.context);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn handle_rtc_auto_suspend_fired_begins_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::RtcAlarmFired(AlarmType::AutoSuspend),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_rtc_auto_power_off_exits() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::RtcAlarmFired(AlarmType::AutoPowerOff),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
    }

    #[test]
    fn handle_rtc_auto_suspend_blocked_when_shared_reschedules() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.context.shared = true;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::RtcAlarmFired(AlarmType::AutoSuspend),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn reschedule_auto_suspend_zero_cancels() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        reschedule_auto_suspend_alarm(&mut harness.context);
        assert!(lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
        harness.context.settings.auto_suspend = 0.0;
        reschedule_auto_suspend_alarm(&mut harness.context);
        assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
    }

    #[test]
    fn reschedule_auto_suspend_moves_deadline_forward() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        reschedule_auto_suspend_alarm(&mut harness.context);
        let first = lock_alarms(&mut harness)
            .time_until_alarm(AlarmType::AutoSuspend)
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        reschedule_auto_suspend_alarm(&mut harness.context);
        let second = lock_alarms(&mut harness)
            .time_until_alarm(AlarmType::AutoSuspend)
            .unwrap();
        assert!(second >= first);
        assert!((second - 30 * 60).abs() < 2);
    }

    #[test]
    fn reschedule_auto_suspend_sub_minute_clamps_nonzero() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 0.01;
        assert_eq!(
            (harness.context.settings.auto_suspend * 60.0) as i64,
            0,
            "precondition: whole-second cast truncates this setting to zero"
        );
        reschedule_auto_suspend_alarm(&mut harness.context);
        assert!(
            lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend),
            "sub-minute AutoSuspend must schedule a future alarm"
        );
        let until = lock_alarms(&mut harness)
            .time_until_alarm(AlarmType::AutoSuspend)
            .unwrap();
        assert!(until >= 0);
    }

    #[test]
    fn begin_suspend_cancels_auto_suspend_alarm() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        reschedule_auto_suspend_alarm(&mut harness.context);
        harness.with_parts(|hub, bus, rq, context, runtime| {
            begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
        });
        assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn cancel_prepare_suspend_reschedules_auto_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.with_parts(|hub, bus, rq, context, runtime| {
            begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
        });
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
        harness.with_parts(|hub, _bus, rq, context, runtime| {
            cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
        });
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn cancel_suspend_rtc_reschedules_auto_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.with_parts(|hub, bus, rq, context, runtime| {
            begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
        });
        harness.tasks.clear();
        {
            let mut alarms = lock_alarms(&mut harness);
            alarms
                .schedule_alarm(AlarmType::Suspend, ChronoDuration::seconds(15))
                .unwrap();
        }
        harness.with_parts(|hub, _bus, rq, context, runtime| {
            cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
        });
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
        assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
    }

    #[test]
    fn stale_suspend_rtc_after_cancel_skips_hardware_sleep() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.with_parts(|hub, bus, rq, context, runtime| {
            begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
        });
        harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime);
        });
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::Suspend));
        harness.with_parts(|hub, _bus, rq, context, runtime| {
            cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
        });
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::RtcAlarmFired(AlarmType::Suspend),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        let power = harness.context.device.power_manager_for_test();
        assert!(
            !power.was_suspend_called(),
            "stale Suspend after cancel must not call power.suspend"
        );
        assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
        assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
    }
}

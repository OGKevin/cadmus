//! Suspend preparation, sleep/wake, post-wake alarm handling, and auto-suspend.

use super::helpers::is_suspend_active;
use super::{SUSPEND_WAIT_DELAY, begin_suspend, schedule_device_task, show_power_off_intermission};
use crate::AlarmType;
use crate::chrono::{Duration as ChronoDuration, Local, Timelike};
use crate::device::DeviceHardware as _;
use crate::device::power::PowerManager;
use crate::device::rtc::{EnsureAlarmOutcome, PastDueAction};
use crate::device::wifi::WifiManager;
use crate::device::{AppContext, DeviceRuntime, DeviceTaskId, EventOutcome, ExitStatus};
use crate::framebuffer::Framebuffer as _;
use crate::frontlight::Frontlight as _;
use crate::settings::IntermKind;
use crate::view::common::locate;
use crate::view::intermission::Intermission;
use crate::view::{Event, Hub, RenderData, RenderQueue, View, wait_for_all};
use std::thread;
use std::time::{Duration, Instant};

pub(super) const AUTO_SUSPEND_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Spawns a background thread that periodically sends [`Event::MightSuspend`].
pub(super) fn spawn_auto_suspend_poller(hub: &Hub, auto_suspend: f32) {
    if auto_suspend <= 0.0 {
        return;
    }

    let hub = hub.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(AUTO_SUSPEND_REFRESH_INTERVAL);
            hub.send(Event::MightSuspend).ok();
        }
    });
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
        Event::Suspend => handle_suspend(hub, rq, context, runtime),
        Event::MightSuspend => handle_might_suspend(hub, bus, rq, context, runtime),
        _ => EventOutcome::Unhandled,
    }
}

fn handle_might_suspend(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.settings.auto_suspend <= 0.0 {
        return EventOutcome::Unhandled;
    }

    if context.shared || is_suspend_active(runtime.tasks) {
        *runtime.inactive_since = Instant::now();
        return EventOutcome::Handled;
    }

    let seconds = 60.0 * context.settings.auto_suspend;
    if runtime.inactive_since.elapsed() > Duration::from_secs_f32(seconds) {
        begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    }

    EventOutcome::Handled
}

/// Handles [`Event::PrepareSuspend`]: persists state and schedules full suspend.
///
/// Clears the prepare task, saves settings, turns off frontlight and WiFi,
/// then schedules [`Event::Suspend`] after [`super::SUSPEND_WAIT_DELAY`].
fn handle_prepare_suspend(
    hub: &Hub,
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
    if context.settings.wifi {
        if let Ok(wifi) = context.device.wifi_manager()
            && let Err(error) = wifi.disable()
        {
            tracing::error!(error = %error, "Failed to disable WiFi on suspend");
        }
        context.online = false;
    }
    schedule_device_task(
        DeviceTaskId::Suspend,
        Event::Suspend,
        SUSPEND_WAIT_DELAY,
        hub,
        runtime.tasks,
    );

    EventOutcome::Handled
}

/// Handles [`Event::Suspend`]: schedules alarms, sleeps, and processes wake events.
fn handle_suspend(
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if let Some(outcome) = schedule_alarms_before_sleep(context, runtime) {
        return outcome;
    }

    let (before, after) = perform_suspend_resume(hub, context, runtime);
    handle_post_wake(before, after, hub, rq, context, runtime)
}

/// Schedules auto-power-off and calendar-update alarms before sleep.
///
/// Returns [`EventOutcome::Exit(ExitStatus::PowerOff)`] when a past-due
/// auto-power-off alarm is detected.
fn schedule_alarms_before_sleep(
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Option<EventOutcome> {
    let alarm_manager = context.alarm_manager.as_mut()?;

    if context.settings.auto_power_off > 0.0 {
        let duration = ChronoDuration::seconds((context.settings.auto_power_off * 86_400.0) as i64);
        match alarm_manager.ensure_scheduled(
            AlarmType::AutoPowerOff,
            duration,
            PastDueAction::Cancel,
        ) {
            Ok(EnsureAlarmOutcome::PastDue) => {
                tracing::info!("AutoPowerOff alarm is past due, powering off");
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

/// Suspends and resumes the device, then reschedules the suspend task.
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
    *runtime.inactive_since = Instant::now();
    let pending_task_ids: Vec<_> = runtime.tasks.iter().map(|t| t.id).collect();
    tracing::debug!(pending_tasks = ?pending_task_ids, "task state after wake");
    schedule_device_task(
        DeviceTaskId::Suspend,
        Event::Suspend,
        SUSPEND_WAIT_DELAY,
        hub,
        runtime.tasks,
    );
    (before, after)
}

/// Processes fired RTC alarms after wake and refreshes the calendar intermission.
fn handle_post_wake(
    before: chrono::DateTime<Local>,
    after: chrono::DateTime<Local>,
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    let _ = hub;
    if let Some(alarm_manager) = context.alarm_manager.as_mut() {
        match alarm_manager.check_fired_alarms(before.to_utc(), after.to_utc()) {
            Ok(fired_alarms) => {
                tracing::info!(alarms = ?fired_alarms, "Checked fired alarms after wake");
                if fired_alarms.contains(&AlarmType::AutoPowerOff) {
                    show_power_off_intermission(
                        context,
                        runtime.view.as_mut(),
                        runtime.history,
                        runtime.updating,
                    );
                    return EventOutcome::Exit(ExitStatus::PowerOff);
                }
                if fired_alarms.contains(&AlarmType::CalendarUpdate)
                    && context.settings.intermissions[IntermKind::Suspend]
                        == crate::settings::IntermissionDisplay::Calendar
                {
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
            }
            Err(error) => {
                tracing::error!(error = %error, "Error checking fired alarms");
            }
        }
    }

    EventOutcome::Handled
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::helpers::has_task;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::frontlight::{Frontlight, LightLevel};
    use crate::settings::IntermissionDisplay;

    #[test]
    fn handle_prepare_suspend_schedules_suspend_task() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        harness.context.settings.wifi = true;
        harness.context.online = true;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(has_task(&harness.tasks, DeviceTaskId::Suspend));
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
        if let Some(alarm_manager) = harness.context.alarm_manager.as_mut() {
            alarm_manager
                .schedule_alarm(AlarmType::AutoPowerOff, ChronoDuration::seconds(-10))
                .unwrap();
        }
        let outcome = harness.with_runtime_only(schedule_alarms_before_sleep);
        assert_eq!(outcome, Some(EventOutcome::Exit(ExitStatus::PowerOff)));
    }

    #[test]
    fn schedule_alarms_calendar_when_intermission_calendar() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
        let outcome = harness.with_runtime_only(schedule_alarms_before_sleep);
        assert!(outcome.is_none());
        assert!(
            harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .has_alarm(AlarmType::CalendarUpdate)
        );
    }

    #[test]
    fn handle_post_wake_auto_power_off_exit() {
        let mut harness = LifecycleHarness::new();
        let before = Local::now();
        if let Some(alarm_manager) = harness.context.alarm_manager.as_mut() {
            alarm_manager
                .schedule_alarm(AlarmType::AutoPowerOff, ChronoDuration::minutes(5))
                .unwrap();
            if let Ok(rtc) = harness.context.device.rtc() {
                rtc.simulate_alarm_fired();
            }
        }
        let after = before + ChronoDuration::minutes(5);
        let outcome = harness.with_parts(|hub, _bus, rq, context, runtime| {
            handle_post_wake(before, after, hub, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
    }

    #[test]
    fn perform_suspend_resume_reschedules_suspend() {
        let mut harness = LifecycleHarness::new();
        let (_before, _after) = harness.with_parts(|hub, _bus, _rq, context, runtime| {
            perform_suspend_resume(hub, context, runtime)
        });
        assert!(has_task(&harness.tasks, DeviceTaskId::Suspend));
        let power = harness.context.device.power_manager_for_test();
        assert!(power.was_suspend_called());
        assert!(power.was_resume_called());
        assert_eq!(power.suspend_call_count(), 1);
        assert_eq!(power.resume_call_count(), 1);
    }

    #[test]
    fn handle_might_suspend_below_threshold_noop() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 5.0;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::MightSuspend, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_might_suspend_above_threshold_begins_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 0.01;
        harness.inactive_since = Instant::now() - Duration::from_secs(120);
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::MightSuspend, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_might_suspend_blocked_when_shared() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 0.01;
        harness.context.shared = true;
        harness.inactive_since = Instant::now() - Duration::from_secs(120);
        let before = harness.inactive_since;
        std::thread::sleep(Duration::from_millis(5));
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::MightSuspend, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(harness.inactive_since > before);
    }
}

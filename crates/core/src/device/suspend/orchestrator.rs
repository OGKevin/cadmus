//! Explicit suspend cycle orchestration (Classic and DeepIdle).
//!
//! Phase diagram, function roles, and kind vs opportunistic soft nap are
//! documented on the parent [`crate::device::suspend`] module. This submodule
//! implements the handlers.
//!
//! # Auto Suspend via RTC
//!
//! Auto Suspend is scheduled as [`AlarmType::AutoSuspend`] on the device RTC
//! Activity calls [`crate::device::reschedule_auto_suspend_alarm`]; when the IRQ listener
//! claims the alarm it emits [`Event::RtcAlarmFired`], which calls
//! [`start_cycle`]. Monotonic idle (`Instant::elapsed`) is not
//! authoritative: soft sleep does not advance it, so the wall-clock RTC
//! deadline is the idle source of truth.

use super::cycle::{DeepIdleWaitState, SuspendCycle, SuspendKind, SuspendPhase};
use super::wake::{self, PollResult};
use super::{PREPARE_SUSPEND_WAIT_DELAY, SUSPEND_WAIT_DELAY};
use crate::AlarmType;
use crate::ClockInstant;
use crate::chrono::{Duration as ChronoDuration, Local, Timelike};
use crate::device::DeviceHardware as _;
use crate::device::inhibitor::{Kind, SoftSuspendName};
use crate::device::power::PowerManager;
use crate::device::rtc::{EnsureAlarmOutcome, PastDueAction};
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::device::{
    AppContext, DeviceRuntime, DeviceTask, DeviceTaskId, EventOutcome, ExitStatus, HistoryItem,
    reschedule_auto_suspend_alarm, schedule_device_task,
};
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::frontlight::Frontlight as _;
use crate::settings::IntermKind;
use crate::view::common::locate;
use crate::view::intermission::Intermission;
use crate::view::{Event, Hub, RenderData, RenderQueue, View, wait_for_all};
use std::sync::mpsc;
use std::time::Duration;

const DEEP_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Whether an explicit suspend cycle is already in flight (prepare task, Suspend
/// RTC, or stored [`SuspendCycle`]).
fn is_suspend_active(context: &AppContext, tasks: &[DeviceTask]) -> bool {
    tasks
        .iter()
        .any(|task| task.id == DeviceTaskId::PrepareSuspend)
        || is_suspend_rtc_pending(context)
        || context.suspend.is_some()
}

/// Acquires the deep-idle cycle lease when retrying after a poll timeout.
fn arm_deep_idle_lease(context: &mut AppContext) -> bool {
    if !context
        .suspend
        .as_ref()
        .is_some_and(|c| c.kind == SuspendKind::DeepIdle)
    {
        return false;
    }
    if context
        .suspend
        .as_ref()
        .is_some_and(|c| c.holds_cycle_lease())
    {
        return true;
    }
    let lease = context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::DeepIdle);
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.cycle_lease = Some(lease);
    }
    true
}

/// Restores autosleep mode and disarms kernel deep idle when leaving a DeepIdle leg.
fn leave_deep_idle_if_needed(context: &mut AppContext) {
    let Some(cycle) = context.suspend.as_mut() else {
        return;
    };
    cycle.cycle_lease = None;
    if let Some(restore) = cycle.deep_idle_restore.take() {
        context.inhibitor.set_mode(restore);
    }
    match context.device.power_manager() {
        Ok(power) => {
            if let Err(error) = power.disarm_deep_idle() {
                tracing::error!(error = %error, "Failed to disarm deep idle");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "power_manager() failed while leaving deep idle");
        }
    }
}

/// Converts auto-power-off days from settings to a chrono duration (minimum one second).
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

/// Cancels a pending Auto Suspend RTC before an explicit cycle starts.
fn cancel_auto_suspend_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = alarm_manager.cancel_alarm(AlarmType::AutoSuspend) {
        tracing::error!(error = %error, "failed to cancel AutoSuspend alarm");
    }
}

/// Schedules [`AlarmType::WakeDebounce`] after leave-sleep for the re-sleep window.
fn schedule_wake_debounce_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    let duration = ChronoDuration::from_std(SUSPEND_WAIT_DELAY)
        .unwrap_or_else(|_| ChronoDuration::seconds(15));
    if let Err(error) = alarm_manager.schedule_in(AlarmType::WakeDebounce, duration) {
        tracing::error!(error = %error, "failed to schedule WakeDebounce alarm");
    }
}

/// Clears a pending WakeDebounce RTC before re-entering sleep or finishing the cycle.
fn cancel_wake_debounce_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = alarm_manager.cancel_alarm(AlarmType::WakeDebounce) {
        tracing::error!(error = %error, "failed to cancel WakeDebounce alarm");
    }
}

/// Returns whether WakeDebounce is still armed on the RTC.
fn is_wake_debounce_scheduled(context: &AppContext) -> bool {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return false;
    };
    let alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    alarm_manager.has_alarm(AlarmType::WakeDebounce)
}

/// Schedules [`AlarmType::Suspend`] after prepare on the classic hard-suspend path.
fn schedule_suspend_alarm(context: &mut AppContext, delay: std::time::Duration) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    let duration = ChronoDuration::from_std(delay).unwrap_or_else(|_| ChronoDuration::seconds(15));
    if let Err(error) = alarm_manager.schedule_in(AlarmType::Suspend, duration) {
        tracing::error!(error = %error, "failed to schedule Suspend alarm");
    }
}

/// Clears a pending Suspend RTC (enter-sleep-now or cycle cancel).
fn cancel_suspend_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = alarm_manager.cancel_alarm(AlarmType::Suspend) {
        tracing::error!(error = %error, "failed to cancel Suspend alarm");
    }
}

/// Returns whether [`AlarmType::Suspend`] is still armed on the RTC.
fn is_suspend_alarm_scheduled(context: &AppContext) -> bool {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return false;
    };
    let alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    alarm_manager.has_alarm(AlarmType::Suspend)
}

/// True while Suspend or WakeDebounce RTC alarms are pending.
pub(crate) fn is_suspend_rtc_pending(context: &AppContext) -> bool {
    is_suspend_alarm_scheduled(context) || is_wake_debounce_scheduled(context)
}

/// Cancels Suspend and WakeDebounce RTC alarms together.
pub(in crate::device::suspend) fn cancel_suspend_rtcs(context: &mut AppContext) {
    cancel_suspend_alarm(context);
    cancel_wake_debounce_alarm(context);
}

/// Handles PrepareSuspend / Suspend / PollDeepIdleWait / suspend RTC alarms.
///
/// Event routing maps onto the phase machine documented at the module root:
/// `PrepareSuspend` → [`prepare_for_sleep`], `Suspend` / Suspend RTC →
/// [`enter_sleep`], `PollDeepIdleWait` → deep-idle poll.
pub(crate) fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::PrepareSuspend => prepare_for_sleep(hub, bus, rq, context, runtime),
        Event::Suspend => enter_sleep(hub, bus, rq, context, runtime),
        Event::PollDeepIdleWait => poll_deep_idle_wait(hub, bus, rq, context, runtime),
        Event::RtcAlarmFired(alarm_type) => {
            handle_rtc_alarm_fired(*alarm_type, hub, bus, rq, context, runtime)
        }
        _ => EventOutcome::Unhandled,
    }
}

/// Dispatches suspend-related RTC IRQ alarms onto phase handlers.
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
        AlarmType::Suspend => enter_sleep(hub, bus, rq, context, runtime),
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
            handle_calendar_update_continue_cycle(hub, bus, rq, context, runtime)
        }
    }
}

/// Auto Suspend idle deadline fired; starts a cycle unless USB-shared or already suspending.
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

    start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    EventOutcome::Handled
}

/// WakeDebounce window expired; re-enters sleep when not USB-shared.
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
    reenter_sleep(hub, bus, rq, context, runtime)
}

/// Rebuilds the calendar intermission widget when [`AlarmType::CalendarUpdate`] fires.
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

/// Seconds until the next five-minute wall-clock boundary (+ one second skew).
fn seconds_until_next_five_minute_boundary(now: &impl Timelike) -> i64 {
    let seconds_into_current_5min = (now.minute() as i64 % 5) * 60 + now.second() as i64;
    300 - seconds_into_current_5min + 1
}

/// Schedules [`AlarmType::CalendarUpdate`] for calendar intermission refresh during suspend.
///
/// Computes seconds to the next 5-minute boundary from system [`Local`] (calendar
/// UI clock), then arms a relative RTC alarm for that duration.
fn schedule_next_calendar_update(context: &mut AppContext) {
    if context.settings.intermissions[IntermKind::Suspend]
        != crate::settings::IntermissionDisplay::Calendar
    {
        return;
    }
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
    let duration = ChronoDuration::seconds(seconds_until_next_five_minute_boundary(&Local::now()));
    alarm_manager
        .ensure_scheduled(
            AlarmType::CalendarUpdate,
            duration,
            PastDueAction::Reschedule,
        )
        .map_err(|error| tracing::error!(error = %error, "Can't schedule calendar update alarm"))
        .ok();
}

/// CalendarUpdate fired during suspend; refresh UI and re-enter sleep.
fn handle_calendar_update_continue_cycle(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    refresh_calendar_intermission(rq, context, runtime);
    schedule_next_calendar_update(context);
    if context.shared {
        return EventOutcome::Handled;
    }
    reenter_sleep(hub, bus, rq, context, runtime)
}

/// Re-enter sleep after WakeDebounce or CalendarUpdate, preserving cycle kind.
///
/// Both Classic and DeepIdle set [`SuspendPhase::ArmingSleep`] and call
/// [`enter_sleep`] without a second [`prepare_for_sleep`] (frontlight/WiFi
/// teardown already ran). DeepIdle re-acquires the cycle lease when needed.
fn reenter_sleep(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    let kind = context.suspend.as_ref().map(|c| c.kind).unwrap_or_else(|| {
        if context.inhibitor.mode().is_armed() {
            SuspendKind::DeepIdle
        } else {
            SuspendKind::Classic
        }
    });
    cancel_suspend_rtcs(context);
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.phase = SuspendPhase::ArmingSleep;
    }
    if kind == SuspendKind::DeepIdle && !arm_deep_idle_lease(context) {
        tracing::error!("deep idle reentry failed: could not acquire cycle lease");
        finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
        return EventOutcome::Handled;
    }
    enter_sleep(hub, bus, rq, context, runtime)
}

/// Shared prepare teardown, then arm Classic Suspend RTC or enter DeepIdle sleep.
///
/// Advances phase to [`SuspendPhase::ArmingSleep`]. DeepIdle calls
/// [`enter_sleep`] immediately; Classic schedules [`AlarmType::Suspend`].
fn prepare_for_sleep(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
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

    let kind = context.suspend.as_ref().map(|c| c.kind);
    match kind {
        Some(SuspendKind::DeepIdle) => {
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::ArmingSleep;
            }
            enter_sleep(hub, bus, rq, context, runtime)
        }
        Some(SuspendKind::Classic) => {
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::ArmingSleep;
            }
            schedule_suspend_alarm(context, SUSPEND_WAIT_DELAY);
            EventOutcome::Handled
        }
        None => {
            schedule_suspend_alarm(context, SUSPEND_WAIT_DELAY);
            EventOutcome::Handled
        }
    }
}

/// Enter kernel sleep for the active cycle (or refuse a stray classic Suspend).
///
/// This is **not** how callers start suspend — use [`start_cycle`] first.
/// Handles [`Event::Suspend`] and [`AlarmType::Suspend`]:
/// - DeepIdle → start non-blocking wait ([`SuspendPhase::InSleep`])
/// - Classic → blocking `power.suspend` / `resume`, then post-wake debounce
/// - Already `InSleep` / `PostWakeDebounce` → ignore
/// - No cycle + soft armed → refuse classic (do not invent a cycle)
fn enter_sleep(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    cancel_suspend_alarm(context);

    if let Some(cycle) = context.suspend.as_ref()
        && cycle.is_in_sleep_or_debounce()
    {
        tracing::debug!(
            kind = ?cycle.kind,
            in_sleep_or_debounce = cycle.is_in_sleep_or_debounce(),
            "ignoring Suspend during in-sleep or post-wake debounce"
        );
        return EventOutcome::Handled;
    }

    if let Some(SuspendKind::DeepIdle) = context.suspend.as_ref().map(|c| c.kind) {
        if let Some(outcome) = schedule_alarms_before_sleep(context, runtime) {
            return outcome;
        }
        start_deep_idle_wait(hub, context, runtime.tasks);
        return EventOutcome::Handled;
    }

    if context.suspend.is_none() && context.inhibitor.mode().is_armed() {
        tracing::error!(
            "refusing classic suspend while soft-suspend is armed without an explicit cycle"
        );
        log_soft_suspend_holders(context, "classic suspend refused while soft-suspend armed");
        reschedule_auto_suspend_alarm(context);
        return EventOutcome::Handled;
    }

    if locate::<Intermission>(runtime.view.as_ref()).is_none() {
        return EventOutcome::Handled;
    }

    if let Some(outcome) = schedule_alarms_before_sleep(context, runtime) {
        return outcome;
    }

    let (before, after) = perform_suspend_resume(hub, context, runtime);
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.phase = SuspendPhase::PostWakeDebounce;
    }
    handle_post_wake(before, after, hub, bus, rq, context, runtime)
}

/// Arms AutoPowerOff and CalendarUpdate RTC alarms before kernel sleep; may exit for power-off.
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
        drop(alarm_manager);
        schedule_next_calendar_update(context);
    }

    None
}

/// Arms mem autosleep, kernel deep idle, and periodic [`Event::PollDeepIdleWait`] polling.
fn start_deep_idle_wait(hub: &Hub, context: &mut AppContext, tasks: &mut Vec<DeviceTask>) {
    let before = Local::now();
    tracing::info!(
        "{}",
        before.format("Entered deep idle on %B %-d, %Y at %H:%M:%S.")
    );

    let restore_mode = context.inhibitor.mode();
    if let Some(cycle) = context.suspend.as_mut()
        && cycle.deep_idle_restore.is_none()
    {
        cycle.deep_idle_restore = Some(restore_mode);
    }
    context.inhibitor.set_mode(AutosleepMode::Mem);

    match context.device.power_manager() {
        Ok(power) => {
            if let Err(error) = power.arm_deep_idle() {
                tracing::error!(error = %error, "Failed to arm deep idle");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "power_manager() failed arming deep idle");
        }
    }

    nix::unistd::sync();
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.cycle_lease = None;
        cycle.phase = SuspendPhase::InSleep {
            wait: DeepIdleWaitState::capture(before),
        };
    }
    log_soft_suspend_holders(context, "deep idle enter wait");
    tasks.retain(|task| task.id != DeviceTaskId::PrepareSuspend);
    schedule_device_task(
        DeviceTaskId::PollDeepIdleWait,
        Event::PollDeepIdleWait,
        DEEP_IDLE_POLL_INTERVAL,
        hub,
        tasks,
    );
}

/// Polls boottime/monotonic wake detect; retries deep idle or finishes on timeout.
fn poll_deep_idle_wait(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    runtime
        .tasks
        .retain(|task| task.id != DeviceTaskId::PollDeepIdleWait);

    let Some(state) = context
        .suspend
        .as_ref()
        .and_then(|c| c.deep_idle_wait().cloned())
    else {
        return EventOutcome::Handled;
    };

    match wake::resolve_wait(context, &state) {
        PollResult::StillWaiting => {
            schedule_device_task(
                DeviceTaskId::PollDeepIdleWait,
                Event::PollDeepIdleWait,
                DEEP_IDLE_POLL_INTERVAL,
                hub,
                runtime.tasks,
            );
            EventOutcome::Handled
        }
        PollResult::Woke => {
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::PostWakeDebounce;
            }
            let after = Local::now();
            tracing::info!(
                "{}",
                after.format("Left deep idle on %B %-d, %Y at %H:%M:%S.")
            );
            leave_deep_idle_if_needed(context);
            schedule_wake_debounce_alarm(context);
            handle_post_wake(
                state.sleep_started_at(),
                after,
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        }
        PollResult::TimedOut => {
            let after = Local::now();
            tracing::warn!("deep idle wait timed out without detecting suspend");
            log_soft_suspend_holders(context, "deep idle wait timeout");
            tracing::warn!("deep idle timed out; retrying deep idle");
            tracing::info!(
                "{}",
                after.format("Left deep idle on %B %-d, %Y at %H:%M:%S.")
            );
            leave_deep_idle_if_needed(context);
            if context.suspend.is_none() {
                return EventOutcome::Handled;
            }
            if !context.inhibitor.mode().is_armed() {
                tracing::error!("deep idle retry failed: soft-suspend not armed; finishing cycle");
                finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
                return EventOutcome::Handled;
            }
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::ArmingSleep;
            }
            if !arm_deep_idle_lease(context) {
                tracing::error!("deep idle retry failed: could not acquire cycle lease");
                finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
                return EventOutcome::Handled;
            }
            start_deep_idle_wait(hub, context, runtime.tasks);
            EventOutcome::Handled
        }
    }
}

/// Debug snapshot of soft-suspend lease holders at suspend milestones.
fn log_soft_suspend_holders(context: &AppContext, at: &str) {
    let holders = context.inhibitor.holders();
    let holder_names: Vec<&str> = holders.iter().map(|h| h.as_str()).collect();
    tracing::debug!(
        at,
        holders = holders.len(),
        holder_names = ?holder_names,
        mode = %context.inhibitor.mode(),
        grace_secs = context.inhibitor.autosleep_grace().as_secs_f32(),
        cycle_lease_held = context
            .suspend
            .as_ref()
            .is_some_and(|c| c.holds_cycle_lease()),
        "soft-suspend lease snapshot"
    );
}

/// Classic blocking `power.suspend` / `resume` and WakeDebounce arming.
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
                log_soft_suspend_holders(context, "classic suspend failed");
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

/// Checks RTC alarms that fired during classic sleep; may chain debounce, calendar, or power-off.
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
            match alarm_manager.check_fired_alarms(
                ClockInstant::Civil(before.to_utc()),
                ClockInstant::Civil(after.to_utc()),
            ) {
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
            return handle_calendar_update_continue_cycle(hub, bus, rq, context, runtime);
        }
    }

    EventOutcome::Handled
}

/// Start an explicit suspend cycle: fix kind, show intermission, schedule prepare.
///
/// Does **not** put the SoC to sleep. Sleep happens later via
/// [`prepare_for_sleep`] → [`enter_sleep`]. Kind is taken from an existing cycle
/// (re-entry) or from soft-suspend armedness (`DeepIdle` vs `Classic`).
pub(crate) fn start_cycle(
    context: &mut AppContext,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut crate::view::RenderQueue,
    tasks: &mut Vec<DeviceTask>,
) {
    cancel_auto_suspend_alarm(context);
    cancel_suspend_rtcs(context);

    let kind = if let Some(existing) = context.suspend.as_ref() {
        existing.kind
    } else if context.inhibitor.mode().is_armed() {
        SuspendKind::DeepIdle
    } else {
        SuspendKind::Classic
    };
    let preserved_restore = context.suspend.as_ref().and_then(|c| c.deep_idle_restore);

    let mut cycle = SuspendCycle::new(kind);
    cycle.deep_idle_restore = preserved_restore;
    if kind == SuspendKind::DeepIdle {
        let lease = context
            .inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::DeepIdle);
        cycle.cycle_lease = Some(lease);
    }
    context.suspend = Some(cycle);

    view.handle_event(&Event::Suspend, hub, bus, rq, context);
    if let Some(index) = locate::<Intermission>(view) {
        let child = view.child(index);
        rq.add(RenderData::new(child.id(), *child.rect(), UpdateMode::Full));
    } else {
        let interm = Intermission::new(
            context.device.framebuffer().rect(),
            crate::settings::IntermKind::Suspend,
            context,
        );
        rq.add(RenderData::new(
            interm.id(),
            *interm.rect(),
            UpdateMode::Full,
        ));
        view.children_mut().push(Box::new(interm));
    }
    let prepare_delay = if kind == SuspendKind::DeepIdle {
        Duration::ZERO
    } else {
        PREPARE_SUSPEND_WAIT_DELAY
    };
    schedule_device_task(
        DeviceTaskId::PrepareSuspend,
        Event::PrepareSuspend,
        prepare_delay,
        hub,
        tasks,
    );
}

/// End the cycle and return to interactive use (frontlight, wifi-at-rest, AutoSuspend).
pub(in crate::device::suspend) fn finish_cycle(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut crate::view::RenderQueue,
) {
    tasks.retain(|task| {
        task.id != DeviceTaskId::PrepareSuspend && task.id != DeviceTaskId::PollDeepIdleWait
    });
    leave_deep_idle_if_needed(context);
    context.suspend = None;
    context.set_frontlight(context.settings.frontlight);
    if context.settings.wifi.wants_radio_at_rest() {
        let session = context.wifi_session.clone();
        let hub = hub.clone();
        std::thread::spawn(move || match session.enable_radio() {
            Ok(true) => {
                hub.send((Event::Device(crate::input::DeviceEvent::NetUp)).into())
                    .ok();
            }
            Ok(false) => {}
            Err(error) => {
                tracing::error!(error = %error, "Failed to enable WiFi on resume");
            }
        });
    }
    if let Some(alarm_manager) = context.alarm_manager.as_ref() {
        let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
        for alarm in AlarmType::alarms_to_cancel_after_resume() {
            if let Err(error) = alarm_manager.cancel_alarm(alarm) {
                tracing::error!(error = ?error, alarm = ?alarm, "failed to cancel alarm after resume");
            }
        }
    }
    reschedule_auto_suspend_alarm(context);
    if let Some(index) = locate::<Intermission>(view) {
        let rect = *view.child(index).rect();
        view.children_mut().remove(index);
        rq.add(RenderData::expose(rect, UpdateMode::Full));
    } else {
        tracing::warn!("resume called but no intermission view found to remove");
    }
    hub.send((Event::ClockTick).into()).ok();
    hub.send((Event::BatteryTick).into()).ok();
}

/// Abort during [`SuspendPhase::Preparing`] only (drop PrepareSuspend task + intermission).
///
/// Full-cycle cancels after sleep arming use [`finish_cycle`] via
/// [`super::cancel_suspend_if_pending`].
pub(in crate::device::suspend) fn cancel_prepare(
    context: &mut AppContext,
    id: DeviceTaskId,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut crate::view::RenderQueue,
) {
    if id != DeviceTaskId::PrepareSuspend {
        return;
    }

    tasks.retain(|task| {
        task.id != DeviceTaskId::PrepareSuspend && task.id != DeviceTaskId::PollDeepIdleWait
    });
    leave_deep_idle_if_needed(context);
    context.suspend = None;
    if let Some(index) = locate::<Intermission>(view) {
        let rect = *view.child(index).rect();
        view.children_mut().remove(index);
        rq.add(RenderData::expose(rect, UpdateMode::Full));
    } else {
        tracing::warn!("resume called but no intermission view found to remove");
    }
    hub.send((Event::ClockTick).into()).ok();
    hub.send((Event::BatteryTick).into()).ok();
    reschedule_auto_suspend_alarm(context);
}

/// Tears down the view stack and renders the power-off intermission.
pub(crate) fn show_power_off_intermission(
    context: &mut AppContext,
    view: &mut dyn View,
    history: &mut Vec<HistoryItem>,
    updating: &mut Vec<crate::view::UpdateData>,
) {
    let (tx, _rx) = mpsc::channel();
    view.handle_event(
        &Event::Back,
        &tx,
        &mut crate::view::Bus::new(),
        &mut crate::view::RenderQueue::new(),
        context,
    );
    while let Some(mut item) = history.pop() {
        item.view.handle_event(
            &Event::Back,
            &tx,
            &mut crate::view::Bus::new(),
            &mut crate::view::RenderQueue::new(),
            context,
        );
    }
    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        crate::settings::IntermKind::PowerOff,
        context,
    );
    wait_for_all(updating, context);
    interm.render(context, *interm.rect());
    context
        .device
        .framebuffer_mut()
        .update(interm.rect(), UpdateMode::Full)
        .ok();
}

#[cfg(all(test, feature = "kobo"))]
#[path = "orchestrator_tests.rs"]
mod tests;

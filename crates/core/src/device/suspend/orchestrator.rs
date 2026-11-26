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
//! [`start_cycle_from_auto`]. Monotonic idle (`Instant::elapsed`) is not
//! authoritative: soft sleep does not advance it, so the wall-clock RTC
//! deadline is the idle source of truth.

use super::cycle::{DeepIdleWaitState, SuspendCycle, SuspendKind, SuspendPhase};
use super::helpers::is_suspend_active;
use super::wake::{self, PollResult};
use super::{PREPARE_SUSPEND_WAIT_DELAY, SUSPEND_WAIT_DELAY};
use crate::AlarmType;
use crate::ClockInstant;
use crate::chrono::{Duration as ChronoDuration, Local, Timelike};
use crate::device::DeviceHardware as _;
use crate::device::inhibitor::{
    Kind, SoftSuspendName, WakeLockPauseOutcome, WakeLockResumeOutcome,
};
#[cfg(target_os = "linux")]
use crate::device::linux::wake_telemetry::InterruptSnapshot;
use crate::device::power::PowerManager;
use crate::device::rtc::{
    AlarmManager, EnsureAlarmOutcome, HardwareWakeAlarm, PastDueAction, PreSleepAlarms, Rtc,
};
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
use crate::settings::WifiMode;
use crate::view::common::locate;
use crate::view::intermission::Intermission;
use crate::view::{Event, Hub, RenderData, RenderQueue, View, wait_for_all};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DEEP_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Upper bound for `/sys/power/wakeup_count` reads in refusal diagnostics.
///
/// The kernel can block that node while a wakeup event is in progress; a
/// stalled diagnostic read must not extend a failing suspend cycle.
const POWER_NODE_READ_TIMEOUT: Duration = Duration::from_millis(100);

/// Consecutive deep-idle wait timeouts before falling back to Classic suspend.
///
/// `/sys/power/autosleep` has no error channel, so a kernel refusal only shows
/// up as the 30 s wait timeout. Retrying forever keeps the device awake; after
/// this many refusals the cycle disarms autosleep and hands over to the
/// Classic path, whose errno reaches the log and whose WakeDebounce re-sleep
/// owns any further retries.
const DEEP_IDLE_TIMEOUT_RETRY_CAP: u32 = 3;

/// Origin of a deferred suspend so a release flush can pick the right path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeferredSuspend {
    Auto,
    Explicit,
}

fn queue_deferred_suspend(context: &mut AppContext, origin: DeferredSuspend) {
    if context.deferred_suspend == Some(DeferredSuspend::Explicit) {
        return;
    }
    context.deferred_suspend = Some(origin);
}

/// Acquires the deep-idle cycle lease when retrying after a poll timeout.
async fn arm_deep_idle_lease(context: &mut AppContext) -> bool {
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
    match context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::DeepIdle)
        .await
    {
        Ok(lease) => {
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.cycle_lease = Some(lease);
            }
            true
        }
        Err(error) => {
            tracing::error!(
                error = %error,
                soft_suspend_lease = %SoftSuspendName::DeepIdle,
                "failed to acquire deep-idle cycle lease"
            );
            false
        }
    }
}

/// Restores the shared `cadmus` wake lock when the current sleep leg still
/// owns suppression, re-acquiring it while any SoftSuspend holder remains.
///
/// Classic legs restore their wake lock inside `run_classic_commit`.
/// A failed restore is surfaced, never treated as a successful one.
async fn restore_wake_lock(context: &mut AppContext) {
    let owned = context
        .suspend
        .as_mut()
        .is_some_and(|cycle| cycle.wake_lock_epoch.take().is_some());
    if !owned {
        return;
    }
    match context.inhibitor.resume_soft_suspend_wake_lock().await {
        WakeLockResumeOutcome::Restored | WakeLockResumeOutcome::Unheld => {}
        WakeLockResumeOutcome::RestoreFailed | WakeLockResumeOutcome::WorkerFailed => {
            tracing::error!("soft-suspend wake-lock restore failed");
        }
    }
}

/// Drops the cycle lease, restores its saved autosleep mode, disarms kernel
/// deep idle, and releases this leg's wake-lock suppression.
async fn leave_deep_idle(context: &mut AppContext) {
    if context.suspend.is_none() {
        return;
    }
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.cycle_lease = None;
    }
    if let Some(restore) = context
        .suspend
        .as_mut()
        .and_then(|cycle| cycle.deep_idle_restore.take())
        && let Err(error) = context.inhibitor.set_mode(restore).await
    {
        tracing::error!(error = %error, "failed to restore autosleep mode after deep idle");
    }
    match context.device.power_manager() {
        Ok(power) => {
            if let Err(error) = power.disarm_deep_idle().await {
                tracing::error!(error = %error, "Failed to disarm deep idle");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "power_manager() failed while leaving deep idle");
        }
    }
    restore_wake_lock(context).await;
}

/// Cancels one batch of logical alarms in a single offloaded RTC transaction.
async fn cancel_alarms<const N: usize>(context: &AppContext, alarms: [AlarmType; N]) {
    let Some(alarm_manager) = context.alarm_manager.as_ref().cloned() else {
        return;
    };
    crate::device::rtc::alarm_transaction(&alarm_manager, async move |mut manager| {
        for alarm in alarms {
            if let Err(error) = manager.cancel_alarm(alarm).await {
                tracing::error!(error = %error, ?alarm, "failed to cancel alarm");
            }
        }
    })
    .await;
}

/// Schedules one relative alarm within a blocking-pool RTC transaction.
async fn schedule_alarm_in(context: &AppContext, alarm: AlarmType, duration: ChronoDuration) {
    let Some(alarm_manager) = context.alarm_manager.as_ref().cloned() else {
        return;
    };
    crate::device::rtc::alarm_transaction(&alarm_manager, async move |mut manager| {
        if let Err(error) = manager.schedule_in(alarm, duration).await {
            tracing::error!(error = %error, ?alarm, "failed to schedule alarm");
        }
    })
    .await;
}

async fn cancel_auto_suspend_alarm(context: &AppContext) {
    cancel_alarms(context, [AlarmType::AutoSuspend]).await;
}

async fn schedule_wake_debounce_alarm(context: &AppContext) {
    schedule_alarm_in(
        context,
        AlarmType::WakeDebounce,
        ChronoDuration::from_std(SUSPEND_WAIT_DELAY)
            .unwrap_or_else(|_| ChronoDuration::seconds(15)),
    )
    .await;
}

async fn cancel_wake_debounce_alarm(context: &AppContext) {
    cancel_alarms(context, [AlarmType::WakeDebounce]).await;
}

async fn schedule_suspend_alarm(context: &AppContext, delay: std::time::Duration) {
    schedule_alarm_in(
        context,
        AlarmType::Suspend,
        ChronoDuration::from_std(delay).unwrap_or_else(|_| ChronoDuration::seconds(15)),
    )
    .await;
}

async fn cancel_suspend_alarm(context: &AppContext) {
    cancel_alarms(context, [AlarmType::Suspend]).await;
}

/// True while Suspend or WakeDebounce RTC alarms are pending.
pub(crate) async fn is_suspend_rtc_pending(context: &AppContext) -> bool {
    is_suspend_rtc_pending_in(context.alarm_manager.as_ref().cloned()).await
}

/// The shared alarm manager a context of device `D` holds.
type AlarmArcs<R> = std::sync::Arc<tokio::sync::Mutex<AlarmManager<R>>>;

/// True while Suspend or WakeDebounce RTC alarms are pending in `alarm_manager`.
///
/// The shared manager means the query shares hardware worker contention with
/// schedules and cancels, so it uses the same offloaded transaction boundary.
pub(crate) async fn is_suspend_rtc_pending_in<R: Rtc + Send + Sync + 'static>(
    alarm_manager: Option<AlarmArcs<R>>,
) -> bool {
    let Some(alarm_manager) = alarm_manager else {
        return false;
    };
    crate::device::rtc::alarm_transaction(&alarm_manager, async move |manager| {
        manager.has_alarm(AlarmType::Suspend) || manager.has_alarm(AlarmType::WakeDebounce)
    })
    .await
    .unwrap_or(false)
}

/// Cancels Suspend and WakeDebounce RTC alarms together.
pub(in crate::device::suspend) async fn cancel_suspend_rtcs(context: &mut AppContext) {
    cancel_alarms(context, [AlarmType::Suspend, AlarmType::WakeDebounce]).await;
}

/// Handles PrepareSuspend / Suspend / PollDeepIdleWait / suspend RTC alarms.
///
/// Event routing maps onto the phase machine documented at the module root:
/// `PrepareSuspend` → [`prepare_for_sleep`], `Suspend` / Suspend RTC →
/// [`enter_sleep`], `PollDeepIdleWait` → deep-idle poll.
pub(crate) async fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::PrepareSuspend => prepare_for_sleep(hub, bus, rq, context, runtime).await,
        Event::Suspend => enter_sleep(hub, bus, rq, context, runtime).await,
        Event::PollDeepIdleWait => poll_deep_idle_wait(hub, bus, rq, context, runtime).await,
        Event::RtcAlarmFired(alarm_type) => {
            handle_rtc_alarm_fired(*alarm_type, hub, bus, rq, context, runtime).await
        }
        Event::InhibitCleared(kind) => {
            handle_inhibit_cleared(
                *kind,
                context,
                runtime.view.as_mut(),
                hub,
                bus,
                rq,
                runtime.tasks,
            )
            .await
        }
        Event::ClearDeferredSuspend => handle_clear_deferred_suspend(context),
        _ => EventOutcome::Unhandled,
    }
}

/// Clears a queued deferred suspend intent without starting a cycle.
///
/// Used when the deferred sleep must not run: OTA reboot
/// ([`Event::ClearDeferredSuspend`])
/// and user activity ([`crate::device::reschedule_auto_suspend_alarm`]).
/// Returns whether a flag was actually cleared.
pub(crate) fn clear_deferred_suspend(context: &mut AppContext, reason: &'static str) -> bool {
    if context.deferred_suspend.take().is_none() {
        return false;
    }
    tracing::debug!(reason, "cleared deferred suspend intent");
    true
}

/// Drops deferred suspend without flushing (OTA success before reboot).
///
/// Does not re-arm [`AlarmType::AutoSuspend`]; that needs a later
/// [`crate::device::reschedule_auto_suspend_alarm`].
fn handle_clear_deferred_suspend(context: &mut AppContext) -> EventOutcome {
    clear_deferred_suspend(context, "ota_reboot");
    EventOutcome::Handled
}

/// Flushes a deferred suspend intent when the last guard of `kind` is released.
///
/// Posted through [`Event::InhibitCleared`] by the last-release notifier of
/// that kind (see `Inhibitor::set_full_release_notifier` and
/// `Inhibitor::set_sleep_release_notifier`).
///
/// [`Kind::Full`] flushes either origin: an Explicit intent runs
/// [`start_cycle`] even while Sleep is held, an Auto intent goes through
/// [`start_cycle_from_auto`], which re-defers while another gate remains.
/// [`Kind::Sleep`] flushes only a Deferred Auto Suspend; a deferred Explicit
/// suspend stays queued for the Full release. The intent is taken before the
/// start call: a start ignored because a cycle is already in flight drops it,
/// and that cycle's end ([`finish_cycle`] / [`cancel_prepare`]) re-arms the
/// timer; a start deferred by a held Full or Sleep lease queues it again.
async fn handle_inhibit_cleared(
    kind: Kind,
    context: &mut AppContext,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    tasks: &mut Vec<DeviceTask>,
) -> EventOutcome {
    let reason = match kind {
        Kind::Full => "full_inhibit_cleared",
        Kind::Sleep => "sleep_inhibit_cleared",
        Kind::SoftSuspend => "soft_suspend_inhibit_cleared",
    };
    let origin = match (kind, context.deferred_suspend) {
        (Kind::Full, _) => context.deferred_suspend.take(),
        (Kind::Sleep, Some(DeferredSuspend::Auto)) => context.deferred_suspend.take(),
        (Kind::Sleep | Kind::SoftSuspend, _) => None,
    };
    if let Some(origin) = origin {
        tracing::debug!(reason, ?origin, "flushing deferred suspend");
        start_cycle_with_origin(origin, context, view, hub, bus, rq, tasks).await;
    }
    EventOutcome::Handled
}

/// Dispatches suspend-related RTC IRQ alarms onto phase handlers.
async fn handle_rtc_alarm_fired(
    alarm_type: AlarmType,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match alarm_type {
        AlarmType::AutoSuspend => handle_auto_suspend_fired(hub, bus, rq, context, runtime).await,
        AlarmType::Suspend => enter_sleep(hub, bus, rq, context, runtime).await,
        AlarmType::WakeDebounce => handle_wake_debounce_fired(hub, bus, rq, context, runtime).await,
        AlarmType::AutoPowerOff => {
            if context.inhibitor.full_active() {
                return EventOutcome::Handled;
            }
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            )
            .await;
            EventOutcome::Exit(ExitStatus::PowerOff)
        }
        AlarmType::CalendarUpdate => {
            handle_calendar_update_continue_cycle(hub, bus, rq, context, runtime).await
        }
    }
}

/// Auto Suspend idle deadline fired; starts a cycle unless USB-shared or already suspending.
async fn handle_auto_suspend_fired(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.settings.auto_suspend <= 0.0 {
        return EventOutcome::Handled;
    }

    if context.shared || is_suspend_active(context, runtime.tasks).await {
        reschedule_auto_suspend_alarm(context).await;
        return EventOutcome::Handled;
    }

    start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks).await;
    EventOutcome::Handled
}

/// WakeDebounce window expired; re-enters sleep when not USB-shared.
async fn handle_wake_debounce_fired(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    cancel_wake_debounce_alarm(context).await;
    if context.shared {
        return EventOutcome::Handled;
    }
    reenter_sleep(hub, bus, rq, context, runtime).await
}

/// Rebuilds the calendar intermission widget when [`AlarmType::CalendarUpdate`] fires.
async fn refresh_calendar_intermission(
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
    )
    .await;
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
async fn schedule_next_calendar_update(context: &AppContext) {
    if context.settings.intermissions[IntermKind::Suspend]
        != crate::settings::IntermissionDisplay::Calendar
    {
        return;
    }
    let Some(alarm_manager) = context.alarm_manager.as_ref().cloned() else {
        return;
    };
    let duration = ChronoDuration::seconds(seconds_until_next_five_minute_boundary(&Local::now()));
    crate::device::rtc::alarm_transaction(&alarm_manager, async move |mut alarm_manager| {
        alarm_manager
            .ensure_scheduled(
                AlarmType::CalendarUpdate,
                duration,
                PastDueAction::Reschedule,
            )
            .await
            .map_err(
                |error| tracing::error!(error = %error, "Can't schedule calendar update alarm"),
            )
            .ok();
    })
    .await;
}

/// CalendarUpdate fired during suspend; refresh UI and re-enter sleep.
async fn handle_calendar_update_continue_cycle(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    refresh_calendar_intermission(rq, context, runtime).await;
    schedule_next_calendar_update(context).await;
    if context.shared {
        return EventOutcome::Handled;
    }
    reenter_sleep(hub, bus, rq, context, runtime).await
}

/// Re-enter sleep after WakeDebounce or CalendarUpdate, preserving cycle kind.
///
/// Both Classic and DeepIdle set [`SuspendPhase::ArmingSleep`] and call
/// [`enter_sleep`] without a second [`prepare_for_sleep`] (frontlight/WiFi
/// teardown already ran). DeepIdle re-acquires the cycle lease when needed.
async fn reenter_sleep(
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
    cancel_suspend_rtcs(context).await;
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.phase = SuspendPhase::ArmingSleep;
    }
    if kind == SuspendKind::DeepIdle && !arm_deep_idle_lease(context).await {
        tracing::error!("deep idle reentry failed: could not acquire cycle lease");
        finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
        return EventOutcome::Handled;
    }
    Box::pin(enter_sleep(hub, bus, rq, context, runtime)).await
}

/// Shared prepare teardown, then arm Classic Suspend RTC or enter DeepIdle sleep.
///
/// Advances phase to [`SuspendPhase::ArmingSleep`]. DeepIdle calls
/// [`enter_sleep`] immediately; Classic schedules [`AlarmType::Suspend`].
async fn prepare_for_sleep(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    runtime
        .tasks
        .retain(|task| task.id != DeviceTaskId::PrepareSuspend);
    wait_for_all(runtime.updating, context).await;
    if let Some(settings_manager) = runtime.settings_manager {
        settings_manager
            .save(&context.settings)
            .await
            .map_err(|error| tracing::error!(error = %error, "Can't save settings"))
            .ok();
    }

    if context.settings.frontlight {
        context.settings.frontlight_levels = context.device.frontlight().levels();
        if let Err(error) = context.device.frontlight_mut().turn_off().await {
            tracing::error!(error = %error, "failed to turn off frontlight for suspend");
        }
    }
    context.wifi_session.set_desired_radio_on(false);
    context.wifi_session.next_reconcile_generation();
    runtime
        .tasks
        .retain(|task| task.id != DeviceTaskId::WifiReconcile);
    if context.settings.wifi != crate::settings::WifiMode::Off {
        if let Err(error) = context.wifi_session.disable_radio().await {
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
            enter_sleep(hub, bus, rq, context, runtime).await
        }
        Some(SuspendKind::Classic) => {
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::ArmingSleep;
            }
            schedule_suspend_alarm(context, SUSPEND_WAIT_DELAY).await;
            EventOutcome::Handled
        }
        None => {
            schedule_suspend_alarm(context, SUSPEND_WAIT_DELAY).await;
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
async fn enter_sleep(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    cancel_suspend_alarm(context).await;

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

    if context.inhibitor.full_active() {
        if context.suspend.is_some() {
            tracing::warn!("Full inhibit acquired during suspend cycle; aborting kernel sleep");
            finish_blocked_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, bus, rq).await;
        } else {
            tracing::warn!("refusing suspend entry while Full inhibit active");
        }
        return EventOutcome::Handled;
    }

    if let Some(SuspendKind::DeepIdle) = context.suspend.as_ref().map(|c| c.kind) {
        if let Some(outcome) = schedule_alarms_before_sleep(context, runtime).await {
            return outcome;
        }
        start_deep_idle_wait(hub, bus, rq, context, runtime.view.as_mut(), runtime.tasks).await;
        return EventOutcome::Handled;
    }

    if context.suspend.is_none() && context.inhibitor.mode().is_armed() {
        tracing::error!(
            "refusing classic suspend while soft-suspend is armed without an explicit cycle"
        );
        log_soft_suspend_holders(context, "classic suspend refused while soft-suspend armed");
        reschedule_auto_suspend_alarm(context).await;
        return EventOutcome::Handled;
    }

    if locate::<Intermission>(runtime.view.as_ref()).is_none() {
        return EventOutcome::Handled;
    }

    if let Some(outcome) = schedule_alarms_before_sleep(context, runtime).await {
        return outcome;
    }

    match perform_suspend_resume(hub, context, runtime).await {
        ClassicSuspendOutcome::NotSlept => {
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::PostWakeDebounce;
            }
            schedule_wake_debounce_alarm(context).await;
            EventOutcome::Handled
        }
        ClassicSuspendOutcome::BlockedByFull => {
            finish_blocked_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, bus, rq).await;
            EventOutcome::Handled
        }
        ClassicSuspendOutcome::Slept { before, after } => {
            if abort_cycle_for_full_inhibit(
                context,
                runtime.tasks,
                runtime.view.as_mut(),
                hub,
                bus,
                rq,
            )
            .await
            {
                return EventOutcome::Handled;
            }
            schedule_wake_debounce_alarm(context).await;
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::PostWakeDebounce;
            }
            handle_post_wake(before, after, hub, bus, rq, context, runtime).await
        }
    }
}

/// Arms AutoPowerOff and CalendarUpdate RTC alarms before kernel sleep; may exit for power-off.
///
/// AutoPowerOff scheduling and its wake-time read/reprogram run as one RTC
/// transaction on the blocking pool, awaiting it before entering sleep.
async fn schedule_alarms_before_sleep(
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Option<EventOutcome> {
    let alarm_manager = context.alarm_manager.as_ref().cloned()?;

    enum AlarmStep {
        Proceed,
        PowerOff,
    }

    let power_off_days = context.settings.auto_power_off;
    let step = crate::device::rtc::alarm_transaction(&alarm_manager, async move |mut alarms| {
        if power_off_days <= 0.0 {
            AlarmStep::Proceed
        } else {
            let duration =
                crate::device::auto_suspend::secs_to_chrono_duration(power_off_days * 86_400.0);
            match alarms
                .ensure_scheduled(AlarmType::AutoPowerOff, duration, PastDueAction::Cancel)
                .await
            {
                Ok(EnsureAlarmOutcome::PastDue) => AlarmStep::PowerOff,
                Ok(_) => AlarmStep::Proceed,
                Err(error) => {
                    tracing::error!(error = %error, "Can't schedule auto power off alarm");
                    AlarmStep::Proceed
                }
            }
        }
    })
    .await
    .unwrap_or(AlarmStep::Proceed);

    if matches!(step, AlarmStep::PowerOff) {
        tracing::info!("AutoPowerOff alarm is past due, powering off");
        show_power_off_intermission(
            context,
            runtime.view.as_mut(),
            runtime.history,
            runtime.updating,
        )
        .await;
        return Some(EventOutcome::Exit(ExitStatus::PowerOff));
    }

    schedule_next_calendar_update(context).await;

    None
}

/// Reads the hardware wake-alarm slot through the device's own RTC handle.
///
/// The device hands out the same handle the manager wraps, so this never takes
/// the alarm-manager lock and cannot contend with the IRQ listener.
async fn read_wake_alarm(context: &AppContext) -> HardwareWakeAlarm {
    match context.device.rtc() {
        Ok(rtc) => rtc.wake_alarm().await,
        Err(error) => {
            tracing::debug!(error = %error, "wake alarm read skipped: no RTC");
            HardwareWakeAlarm::default()
        }
    }
}

/// Captures the hardware and Cadmus alarm state that deep-idle entry leaves in place.
///
/// The manager lock covers only the schedule read; the hardware slot is read
/// through [`read_wake_alarm`].
async fn capture_pre_sleep_alarms(context: &AppContext) -> PreSleepAlarms {
    let cadmus = match context.alarm_manager.as_ref().cloned() {
        Some(alarms) => crate::device::rtc::alarm_transaction(&alarms, async move |manager| {
            manager.next_scheduled_alarm().await
        })
        .await
        .flatten(),
        None => None,
    };
    PreSleepAlarms {
        hardware: read_wake_alarm(context).await,
        cadmus,
    }
}

#[cfg(target_os = "linux")]
async fn read_interrupts(context: &mut AppContext) -> Option<String> {
    #[cfg(test)]
    {
        context.deep_idle_interrupts_inject.pop_front().flatten()
    }
    #[cfg(not(test))]
    {
        let _ = context;
        crate::device::linux::wake_telemetry::read_interrupts().await
    }
}

#[cfg(target_os = "linux")]
async fn read_interrupt_snapshot(context: &mut AppContext) -> Option<Arc<InterruptSnapshot>> {
    let raw = read_interrupts(context).await?;
    InterruptSnapshot::parse(&raw).map(Arc::new)
}

/// Emits the kernel wake source, alarm comparison, and IRQ counter delta for
/// one primary wake.
///
/// Samples `/proc/interrupts` once, before the other post-wake node reads, so
/// the delta excludes those telemetry reads. That sample supplies both the
/// existing `wake_irq_name` and the comparison against the baseline stored on
/// [`DeepIdleWaitState`].
///
/// A PMIC parent IRQ does not identify its RTC child, and the kernel reports no
/// writer for the hardware alarm, so neither field alone names a cause. Only
/// Linux exposes these nodes, so other platforms emit nothing.
async fn log_wake_source(context: &mut AppContext, state: &DeepIdleWaitState) {
    #[cfg(target_os = "linux")]
    {
        use crate::device::linux::wake_telemetry;

        let interrupts = read_interrupts(context).await;
        let pre_sleep_alarms = state.pre_sleep_alarms();
        let post_hardware_alarm = read_wake_alarm(context).await;
        let wake_irq = wake_telemetry::read_wakeup_irq().await;
        let wake_irq_name = wake_irq.and_then(|irq| {
            interrupts
                .as_deref()
                .and_then(|raw| wake_telemetry::parse_irq_name(raw, irq))
        });
        let (irq_delta_status, irq_deltas_json, irq_delta_reasons_json) =
            interrupt_delta_json(state.interrupt_baseline(), interrupts.as_deref());
        tracing::info!(
            pre_hardware_alarm = %pre_sleep_alarms.hardware,
            pre_cadmus_alarm_type = ?pre_sleep_alarms.cadmus.map(|(alarm_type, _)| alarm_type),
            pre_cadmus_alarm = ?pre_sleep_alarms.cadmus.map(|(_, wake_time)| wake_time),
            post_hardware_alarm = %post_hardware_alarm,
            wake_irq = ?wake_irq,
            wake_irq_name = ?wake_irq_name,
            unmanaged_alarm_candidate = pre_sleep_alarms.unmanaged_alarm_candidate(),
            irq_delta_status,
            irq_deltas_json,
            irq_delta_reasons_json,
            "Primary deep-idle wake"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (context, state);
    }
}

/// Arms mem autosleep, kernel deep idle, and periodic [`Event::PollDeepIdleWait`] polling.
async fn start_deep_idle_wait(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    view: &mut dyn View,
    tasks: &mut Vec<DeviceTask>,
) {
    let entered = Local::now();
    tracing::info!(
        "{}",
        entered.format("Entered deep idle on %B %-d, %Y at %H:%M:%S.")
    );
    let pre_sleep_alarms = capture_pre_sleep_alarms(context).await;

    let restore_mode = context.inhibitor.mode();
    if let Some(cycle) = context.suspend.as_mut()
        && cycle.deep_idle_restore.is_none()
    {
        cycle.deep_idle_restore = Some(restore_mode);
    }
    if let Err(error) = context.inhibitor.set_mode(AutosleepMode::Mem).await {
        tracing::error!(error = %error, "Failed to arm mem autosleep");
    }

    let armed = match context.device.power_manager() {
        Ok(power) => match power.arm_deep_idle().await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(error = %error, "Failed to arm deep idle");
                false
            }
        },
        Err(error) => {
            tracing::error!(error = %error, "power_manager() failed arming deep idle");
            false
        }
    };
    if !armed {
        finish_cycle(context, tasks, view, hub, rq).await;
        return;
    }

    if let Err(error) = crate::runtime::spawn_blocking(nix::unistd::sync).await {
        tracing::error!(error = %error, "Failed to flush filesystem buffers for deep idle");
        finish_cycle(context, tasks, view, hub, rq).await;
        return;
    }
    #[cfg(target_os = "linux")]
    let interrupt_baseline = read_interrupt_snapshot(context).await;
    let before = Local::now();
    let wait = DeepIdleWaitState::capture(
        before,
        pre_sleep_alarms,
        #[cfg(target_os = "linux")]
        interrupt_baseline,
    );
    let epoch = match context.inhibitor.pause_soft_suspend_wake_lock().await {
        WakeLockPauseOutcome::BlockedByFull => {
            finish_blocked_cycle(context, tasks, view, hub, bus, rq).await;
            return;
        }
        WakeLockPauseOutcome::UnlockFailed => {
            tracing::error!("deep idle aborted: wake-lock unlock failed");
            finish_cycle(context, tasks, view, hub, rq).await;
            return;
        }
        WakeLockPauseOutcome::Paused(epoch) => epoch,
    };
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.wake_lock_epoch = Some(epoch);
        cycle.cycle_lease = None;
        cycle.phase = SuspendPhase::InSleep { wait };
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
async fn poll_deep_idle_wait(
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

    if abort_cycle_for_full_inhibit(context, runtime.tasks, runtime.view.as_mut(), hub, bus, rq)
        .await
    {
        return EventOutcome::Handled;
    }

    let epoch_current = context
        .suspend
        .as_ref()
        .and_then(|cycle| cycle.wake_lock_epoch)
        .is_some_and(|epoch| context.inhibitor.wake_lock_epoch_is_current(epoch));
    if !epoch_current {
        tracing::warn!(
            "deep-idle suppression epoch changed before poll; treating as Full interruption"
        );
        finish_blocked_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, bus, rq).await;
        return EventOutcome::Handled;
    }

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
                cycle.consecutive_deep_idle_timeouts = 0;
            }
            let after = Local::now();
            tracing::info!(
                "{}",
                after.format("Left deep idle on %B %-d, %Y at %H:%M:%S.")
            );
            log_wake_source(context, &state).await;
            leave_deep_idle(context).await;
            schedule_wake_debounce_alarm(context).await;
            handle_post_wake(
                state.sleep_started_at(),
                after,
                hub,
                bus,
                rq,
                context,
                runtime,
            )
            .await
        }
        PollResult::TimedOut => {
            let after = Local::now();
            tracing::warn!("deep idle wait timed out without detecting suspend");
            let Some(cycle) = context.suspend.as_mut() else {
                return EventOutcome::Handled;
            };
            cycle.consecutive_deep_idle_timeouts =
                cycle.consecutive_deep_idle_timeouts.saturating_add(1);
            let timeouts = cycle.consecutive_deep_idle_timeouts;
            if timeouts == 1 {
                log_deep_idle_refusal(context, &state).await;
            }
            let fallback = timeouts >= DEEP_IDLE_TIMEOUT_RETRY_CAP;
            if fallback {
                tracing::warn!(
                    timeouts,
                    "deep idle retry cap reached; falling back to classic suspend"
                );
            } else {
                tracing::warn!(timeouts, "deep idle timed out; retrying deep idle");
            }
            tracing::info!(
                "{}",
                after.format("Left deep idle on %B %-d, %Y at %H:%M:%S.")
            );
            leave_deep_idle(context).await;
            if context.suspend.is_none() {
                return EventOutcome::Handled;
            }
            if !context.inhibitor.mode().is_armed() {
                tracing::error!("deep idle retry failed: soft-suspend not armed; finishing cycle");
                finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
                return EventOutcome::Handled;
            }
            if fallback {
                return fallback_to_classic(hub, bus, rq, context, runtime).await;
            }
            if let Some(cycle) = context.suspend.as_mut() {
                cycle.phase = SuspendPhase::ArmingSleep;
            }
            if !arm_deep_idle_lease(context).await {
                tracing::error!("deep idle retry failed: could not acquire cycle lease");
                finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
                return EventOutcome::Handled;
            }
            start_deep_idle_wait(hub, bus, rq, context, runtime.view.as_mut(), runtime.tasks).await;
            EventOutcome::Handled
        }
    }
}

/// Falls back to the Classic suspend path after capped deep-idle timeouts.
///
/// The kernel's autosleep refusal has no errno, so the blocking Classic path is
/// the only way to surface the reason. Autosleep is disarmed for the rest of
/// the cycle because `/sys/power/state` rejects `mem` while it is armed, which
/// would turn every attempt into the same guard error; the armed mode is
/// stashed on the cycle and [`finish_cycle`] restores it. Re-entering through
/// [`enter_sleep`] keeps the cycle on the classic Suspend RTC / WakeDebounce
/// retry path instead of polling autosleep every 30 s.
async fn fallback_to_classic(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    let restored_mode = context.inhibitor.mode();
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.kind = SuspendKind::Classic;
        cycle.phase = SuspendPhase::ArmingSleep;
        cycle.deep_idle_restore = Some(restored_mode);
    }
    if let Err(error) = context.inhibitor.set_mode(AutosleepMode::Off).await {
        tracing::error!(error = %error, "Failed to disarm autosleep for classic fallback");
    }
    enter_sleep(hub, bus, rq, context, runtime).await
}

/// Emits one blocker snapshot per failing deep-idle cycle.
///
/// Autosleep refusals carry no errno, so this names the app-side lease holders
/// and the kernel-side wake state at the first wait timeout. Node reads are
/// best-effort: a missing or unreadable node only yields `None` fields.
async fn log_deep_idle_refusal(context: &mut AppContext, state: &DeepIdleWaitState) {
    let holders = context.inhibitor.holders();
    let holder_names: Vec<&str> = holders.iter().map(|h| h.as_str()).collect();
    #[cfg(target_os = "linux")]
    {
        let kernel_wake_locks = read_power_node("/sys/power/wake_lock").await;
        let wakeup_count = read_wakeup_count().await;
        let interrupts = read_interrupts(context).await;
        let (irq_delta_status, irq_deltas_json, irq_delta_reasons_json) =
            interrupt_delta_json(state.interrupt_baseline(), interrupts.as_deref());
        tracing::warn!(
            holders = holders.len(),
            holder_names = ?holder_names,
            grace_secs = context.inhibitor.autosleep_grace().as_secs_f32(),
            kernel_wake_locks = ?kernel_wake_locks,
            wakeup_count = ?wakeup_count,
            plugged = context.plugged,
            shared = context.shared,
            irq_delta_status,
            irq_deltas_json,
            irq_delta_reasons_json,
            "deep idle wait timed out; suspend blockers"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = state;
        tracing::warn!(
            holders = holders.len(),
            holder_names = ?holder_names,
            grace_secs = context.inhibitor.autosleep_grace().as_secs_f32(),
            plugged = context.plugged,
            shared = context.shared,
            "deep idle wait timed out; suspend blockers"
        );
    }
}

/// Reads a `/sys/power` node trimmed; read failures are optional context.
#[cfg(target_os = "linux")]
async fn read_power_node(path: &str) -> Option<String> {
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .map(|value| value.trim().to_owned())
}

/// One `/sys/power/wakeup_count` read in flight at a time.
///
/// Dropping a timed-out `tokio::fs` future does not stop the blocking-pool
/// read it started, and the kernel can block that node for the duration of a
/// wakeup event. While one read is outstanding, callers get `None` instead of
/// piling up another blocked read.
#[cfg(target_os = "linux")]
static WAKEUP_COUNT_READ_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Reads `/sys/power/wakeup_count` under a timeout, at most one read in
/// flight; a blocked, timed-out, or failed read yields `None`.
///
/// The read runs detached on the process runtime because the caller's timeout
/// only drops the await, not the blocking-pool read it started: the task has to
/// outlive the timeout to clear the in-flight gate when the kernel returns. It
/// is a one-shot with no owner to cancel, so it uses [`crate::runtime::spawn`]
/// rather than a [`Job`](crate::runtime::Job), whose drop cancels.
#[cfg(target_os = "linux")]
async fn read_wakeup_count() -> Option<String> {
    if WAKEUP_COUNT_READ_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    let read = crate::runtime::spawn(async {
        let value = tokio::fs::read_to_string("/sys/power/wakeup_count")
            .await
            .ok()
            .map(|value| value.trim().to_owned());
        WAKEUP_COUNT_READ_IN_FLIGHT.store(false, Ordering::Release);
        value
    });
    tokio::time::timeout(POWER_NODE_READ_TIMEOUT, read)
        .await
        .ok()?
        .ok()?
}

/// Interrupt delta fields comparing a deep-idle baseline with a fresh sample.
///
/// Missing or unparsable samples collapse to the `unavailable` status the wake
/// telemetry already reports.
#[cfg(target_os = "linux")]
fn interrupt_delta_json(
    baseline: Option<Arc<InterruptSnapshot>>,
    interrupts: Option<&str>,
) -> (&'static str, String, String) {
    let comparison = baseline.map(|baseline| {
        let after = interrupts.and_then(InterruptSnapshot::parse);
        baseline.compare(after.as_ref())
    });
    match comparison {
        Some(comparison) => comparison.json(),
        None => crate::device::linux::wake_telemetry::InterruptDeltas::unavailable().json(),
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

enum ClassicSuspendOutcome {
    Slept {
        before: chrono::DateTime<Local>,
        after: chrono::DateTime<Local>,
    },
    BlockedByFull,
    /// No kernel sleep happened; retry without running wake classification.
    NotSlept,
}

/// Shared completion for one owned Classic commit.
///
/// The `DeviceTask` owns cancellation and the Tokio task handle; this shared
/// cell keeps the result available even when the caller stops waiting. A
/// cancelled waiter never cancels the committed write and peripheral cleanup.
pub(in crate::device::suspend) struct ClassicCommit {
    result: tokio::sync::Mutex<Option<ClassicSuspendOutcome>>,
    completed: std::sync::atomic::AtomicBool,
    done: tokio::sync::Notify,
}

impl ClassicCommit {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            result: tokio::sync::Mutex::new(None),
            completed: std::sync::atomic::AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        })
    }

    async fn finish(&self, result: ClassicSuspendOutcome) {
        *self.result.lock().await = Some(result);
        self.completed
            .store(true, std::sync::atomic::Ordering::Release);
        self.done.notify_waiters();
    }

    async fn wait_finished(&self) {
        let notified = self.done.notified();
        tokio::pin!(notified);
        loop {
            notified.as_mut().enable();
            if self.completed.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            notified.as_mut().await;
            notified.set(self.done.notified());
        }
    }

    async fn take_result(&self) -> Option<ClassicSuspendOutcome> {
        self.wait_finished().await;
        self.result.lock().await.take()
    }
}

async fn disarm_classic_hardware<P: PowerManager>(power: &P) {
    if let Err(error) = power.disarm_deep_idle().await {
        tracing::error!(error = %error, "Failed to disarm prepared Classic hardware");
    }
}

/// Owns one Classic commit through preparation, kernel write, and cleanup.
///
/// Cancellation is cooperative only before the gate token is acquired. Once
/// committed, dropping the `DeviceTask` or its waiter cannot select against
/// the write, wake-lock restoration, or peripheral cleanup.
async fn run_classic_commit<P: PowerManager + 'static>(
    cancel: tokio_util::sync::CancellationToken,
    commit: std::sync::Arc<ClassicCommit>,
    power: std::sync::Arc<P>,
    inhibitor: std::sync::Arc<crate::device::inhibitor::Inhibitor>,
    plugged: bool,
    shared: bool,
) {
    let before = Local::now();
    let outcome = async {
        if let Err(error) = power.prepare_suspend().await {
            tracing::error!(error = %error, "Failed to prepare classic suspend");
            disarm_classic_hardware(power.as_ref()).await;
            return ClassicSuspendOutcome::NotSlept;
        }
        if cancel.is_cancelled() {
            disarm_classic_hardware(power.as_ref()).await;
            return ClassicSuspendOutcome::NotSlept;
        }

        let gate = inhibitor.lock_full_suspend_gate().await;
        if cancel.is_cancelled() || inhibitor.full_active() {
            drop(gate);
            disarm_classic_hardware(power.as_ref()).await;
            if inhibitor.full_active() {
                return ClassicSuspendOutcome::BlockedByFull;
            }
            return ClassicSuspendOutcome::NotSlept;
        }

        match inhibitor.pause_soft_suspend_wake_lock().await {
            WakeLockPauseOutcome::BlockedByFull => {
                drop(gate);
                disarm_classic_hardware(power.as_ref()).await;
                ClassicSuspendOutcome::BlockedByFull
            }
            WakeLockPauseOutcome::UnlockFailed => {
                drop(gate);
                disarm_classic_hardware(power.as_ref()).await;
                ClassicSuspendOutcome::NotSlept
            }
            WakeLockPauseOutcome::Paused(_) => {
                let suspend_result = power.suspend().await;
                let suspend_failed = suspend_result.is_err();
                if let Err(error) = suspend_result {
                    #[cfg(target_os = "linux")]
                    {
                        let kernel_wake_locks = read_power_node("/sys/power/wake_lock").await;
                        let wakeup_count = read_wakeup_count().await;
                        tracing::error!(
                            error = %error,
                            kernel_wake_locks = ?kernel_wake_locks,
                            wakeup_count = ?wakeup_count,
                            plugged,
                            shared,
                            "Failed to suspend device"
                        );
                    }
                    #[cfg(not(target_os = "linux"))]
                    tracing::error!(error = %error, plugged, shared, "Failed to suspend device");
                }
                let after = Local::now();
                tracing::info!("{}", after.format("Woke up on %B %-d, %Y at %H:%M:%S."));
                if let Err(error) = power.resume().await {
                    tracing::error!(error = %error, "Failed to resume device");
                }
                if matches!(
                    inhibitor.resume_soft_suspend_wake_lock().await,
                    WakeLockResumeOutcome::RestoreFailed | WakeLockResumeOutcome::WorkerFailed
                ) {
                    tracing::error!("Classic wake-lock restoration failed after sleep");
                }
                drop(gate);
                if suspend_failed {
                    ClassicSuspendOutcome::NotSlept
                } else {
                    ClassicSuspendOutcome::Slept { before, after }
                }
            }
        }
    }
    .await;
    commit.finish(outcome).await;
}

/// Stores one Classic commit in the device task owner and awaits its result.
/// The cycle retains that commit after its finished task is pruned, allowing
/// later cleanup to await completion without taking the consumed result.
async fn perform_suspend_resume(
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> ClassicSuspendOutcome {
    let before = Local::now();
    tracing::info!(
        "{}",
        before.format("Went to sleep on %B %-d, %Y at %H:%M:%S.")
    );
    tracing::debug!(
        rotation = context.display.rotation,
        dims = ?context.display.dims,
        framebuffer_rotation = context.device.framebuffer().rotation(),
        framebuffer_dims = ?context.device.framebuffer().dims(),
        "display state before suspend"
    );
    let power = match context.device.power_manager() {
        Ok(power) => power,
        Err(error) => {
            tracing::error!(error = %error, "power_manager() initialization failed for suspend");
            return ClassicSuspendOutcome::NotSlept;
        }
    };
    let commit = ClassicCommit::new();
    if let Some(cycle) = context.suspend.as_mut() {
        cycle.classic_commit = Some(Arc::clone(&commit));
    }
    let task_commit = Arc::clone(&commit);
    let inhibitor = Arc::clone(&context.inhibitor);
    let plugged = context.plugged;
    let shared = context.shared;
    runtime.tasks.push(DeviceTask {
        id: DeviceTaskId::ClassicSuspend,
        job: crate::runtime::Job::spawn(move |cancel| {
            run_classic_commit(cancel, task_commit, power, inhibitor, plugged, shared)
        }),
    });
    let result = commit.take_result().await;
    runtime
        .tasks
        .retain(|task| task.id != DeviceTaskId::ClassicSuspend);
    if result.is_none() {
        tracing::warn!("Classic commit result was already consumed");
    }
    let _ = hub;
    result.unwrap_or(ClassicSuspendOutcome::NotSlept)
}

/// Drains a stored Classic commit before any further hardware cleanup.
///
/// Removing its `DeviceTask` requests cancellation, which is honoured only
/// before the commit point; an in-flight kernel write always runs through
/// peripheral cleanup first. The result is consumed only by the original waiter;
/// cleanup waits separately for completion even after that consumption.
pub async fn drain_committed_suspend(context: &AppContext, tasks: &mut Vec<DeviceTask>) {
    let commit = context
        .suspend
        .as_ref()
        .and_then(|cycle| cycle.classic_commit.clone());
    tasks.retain(|task| task.id != DeviceTaskId::ClassicSuspend);
    if let Some(commit) = commit {
        commit.wait_finished().await;
    }
}

/// Checks RTC alarms that fired during classic sleep; may chain debounce, calendar, or power-off.
async fn handle_post_wake(
    before: chrono::DateTime<Local>,
    after: chrono::DateTime<Local>,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if let Some(alarm_manager) = context.alarm_manager.as_ref().cloned() {
        let fired_alarms =
            crate::device::rtc::alarm_transaction(&alarm_manager, async move |mut manager| {
                match manager
                    .check_fired_alarms(
                        ClockInstant::Civil(before.to_utc()),
                        ClockInstant::Civil(after.to_utc()),
                    )
                    .await
                {
                    Ok(fired) => {
                        tracing::info!(alarms = ?fired, "Checked fired alarms after wake");
                        fired
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "Error checking fired alarms");
                        Vec::new()
                    }
                }
            })
            .await
            .unwrap_or_default();
        if fired_alarms.contains(&AlarmType::AutoPowerOff) {
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            )
            .await;
            return EventOutcome::Exit(ExitStatus::PowerOff);
        }
        if fired_alarms.contains(&AlarmType::WakeDebounce) {
            return handle_wake_debounce_fired(hub, bus, rq, context, runtime).await;
        }
        if fired_alarms.contains(&AlarmType::CalendarUpdate) {
            return handle_calendar_update_continue_cycle(hub, bus, rq, context, runtime).await;
        }
    }

    EventOutcome::Handled
}

/// Auto Suspend idle path: defers while Full **or** Sleep inhibit is held.
///
/// Explicit callers ([`start_cycle`]) still run while Sleep is held. Cover,
/// power button, and menu Suspend use that path. Queues
/// [`DeferredSuspend::Auto`] so [`handle_inhibit_cleared`] rechecks the
/// remaining gates instead of flushing through `start_cycle`. The last Sleep
/// lease release also flushes through this function, so a Full lease or
/// another Sleep holder still re-defers the intent; the next gesture's
/// [`crate::device::reschedule_auto_suspend_alarm`] clears it.
pub(crate) async fn start_cycle_from_auto(
    context: &mut AppContext,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut crate::view::RenderQueue,
    tasks: &mut Vec<DeviceTask>,
) {
    start_cycle_with_origin(DeferredSuspend::Auto, context, view, hub, bus, rq, tasks).await;
}

/// Start an explicit suspend cycle: fix kind, show intermission, schedule prepare.
///
/// Does **not** put the SoC to sleep. Sleep happens later via
/// [`prepare_for_sleep`] → [`enter_sleep`]. Kind is fixed from soft-suspend
/// armedness (`DeepIdle` vs `Classic`).
///
/// A call while a suspend cycle is already in flight is ignored: it creates no
/// second cycle and leaves the running cycle's tasks and RTC alarms untouched.
///
/// When [`Inhibitor::full_active`](crate::device::inhibitor::Inhibitor::full_active)
/// is true, sets
/// [`Context::deferred_suspend`](crate::context::Context::deferred_suspend)
/// and returns without a cycle. SoftSuspend-only and Sleep holders do not defer
/// this explicit path. Their shared kernel `cadmus` wake lock is suppressed
/// later, at the actual kernel-sleep boundary, and restored on wake while
/// holders remain. Auto Suspend uses [`start_cycle_from_auto`], which also
/// defers on Sleep.
async fn start_cycle_with_origin(
    origin: DeferredSuspend,
    context: &mut AppContext,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut crate::view::RenderQueue,
    tasks: &mut Vec<DeviceTask>,
) {
    if is_suspend_active(context, tasks).await {
        tracing::debug!("ignoring suspend start while a suspend cycle is active");
        return;
    }

    let full = context.inhibitor.full_active();
    let sleep = context.inhibitor.sleep_active();
    if origin == DeferredSuspend::Auto && (full || sleep) {
        queue_deferred_suspend(context, DeferredSuspend::Auto);
        tracing::debug!(full, sleep, "deferring Auto suspend while inhibit active");
        return;
    }
    if full {
        queue_deferred_suspend(context, DeferredSuspend::Explicit);
        tracing::debug!("deferring suspend while Full inhibit active");
        return;
    }

    cancel_auto_suspend_alarm(context).await;
    cancel_suspend_rtcs(context).await;

    let kind = if context.inhibitor.mode().is_armed() {
        SuspendKind::DeepIdle
    } else {
        SuspendKind::Classic
    };
    let origin = if origin == DeferredSuspend::Explicit
        || context
            .suspend
            .as_ref()
            .is_some_and(|existing| existing.origin == DeferredSuspend::Explicit)
    {
        DeferredSuspend::Explicit
    } else {
        origin
    };

    let mut cycle = SuspendCycle::new(kind, origin);
    if kind == SuspendKind::DeepIdle {
        match context
            .inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::DeepIdle)
            .await
        {
            Ok(lease) => cycle.cycle_lease = Some(lease),
            Err(error) => {
                tracing::error!(
                    error = %error,
                    soft_suspend_lease = %SoftSuspendName::DeepIdle,
                    "failed to acquire deep-idle cycle lease; falling back to Classic suspend"
                );
                cycle.kind = SuspendKind::Classic;
            }
        }
    }
    context.suspend = Some(cycle);

    view.handle_event(&Event::Suspend, hub, bus, rq, context)
        .await;
    if let Some(index) = locate::<Intermission>(view) {
        let child = view.child(index);
        rq.add(RenderData::new(child.id(), *child.rect(), UpdateMode::Full));
    } else {
        let interm = Intermission::new(
            context.device.framebuffer().rect(),
            crate::settings::IntermKind::Suspend,
            context,
        )
        .await;
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

/// Starts an explicit suspend cycle; see [`start_cycle_with_origin`].
pub(crate) async fn start_cycle(
    context: &mut AppContext,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut crate::view::RenderQueue,
    tasks: &mut Vec<DeviceTask>,
) {
    start_cycle_with_origin(
        DeferredSuspend::Explicit,
        context,
        view,
        hub,
        bus,
        rq,
        tasks,
    )
    .await;
}

/// Aborts an active cycle when `Kind::Full` was acquired after it started.
///
/// Returns whether a cycle was aborted and preserves its intent through cleanup.
/// If Full was already cleared, the intent is flushed immediately; otherwise it
/// waits for `FullInhibitCleared`. An Explicit origin wins over Auto. A worker
/// can acquire Full after a cycle starts, so sleep must not cross a live holder.
async fn abort_cycle_for_full_inhibit(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
) -> bool {
    if !context.inhibitor.full_active() || context.suspend.is_none() {
        return false;
    }
    tracing::warn!("Full inhibit acquired during suspend cycle; aborting kernel sleep");
    finish_blocked_cycle(context, tasks, view, hub, bus, rq).await;
    true
}

/// Ends a cycle whose sleep window was blocked by Full inhibit.
///
/// Origin and any newer queued request are preserved through cleanup. If Full
/// is still held, the intent waits for `FullInhibitCleared`; if it is already
/// gone, the intent is flushed here because that notification may already
/// have been handled. Explicit origin wins over Auto.
async fn finish_blocked_cycle(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
) {
    let cycle_origin = context.suspend.as_ref().map(|cycle| cycle.origin);
    let queued_origin = context.deferred_suspend;
    finish_cycle(context, tasks, view, hub, rq).await;
    let origin = match (cycle_origin, queued_origin) {
        (Some(DeferredSuspend::Explicit), _) | (_, Some(DeferredSuspend::Explicit)) => {
            DeferredSuspend::Explicit
        }
        (Some(DeferredSuspend::Auto), _) | (_, Some(DeferredSuspend::Auto)) => {
            DeferredSuspend::Auto
        }
        (None, None) => DeferredSuspend::Explicit,
    };
    queue_deferred_suspend(context, origin);
    if !context.inhibitor.full_active() {
        handle_full_inhibit_cleared(context, view, hub, bus, rq, tasks).await;
    }
}

/// End the cycle and return to interactive use (frontlight, wifi-at-rest, AutoSuspend).
///
/// Single teardown path for wake, cancel, and abort. Peripheral and
/// post-resume alarm cleanup runs whenever a cycle exists or a Suspend /
/// WakeDebounce RTC is still armed, including cancels that land while only a
/// PrepareSuspend task is pending: a repeat Suspend can replace the cycle
/// after teardown already turned frontlight and WiFi off. Task-only cleanup
/// still removes the intermission, reschedules Auto Suspend, and emits ticks.
pub(in crate::device::suspend) async fn finish_cycle(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut crate::view::RenderQueue,
) {
    let full_cleanup = context.suspend.is_some() || is_suspend_rtc_pending(context).await;
    drain_committed_suspend(context, tasks).await;
    tasks.retain(|task| {
        task.id != DeviceTaskId::PrepareSuspend && task.id != DeviceTaskId::PollDeepIdleWait
    });
    cancel_suspend_rtcs(context).await;
    leave_deep_idle(context).await;
    context.suspend = None;
    if full_cleanup {
        context.set_frontlight(context.settings.frontlight).await;
        crate::device::spawn_wifi_reconcile(
            hub,
            &context.wifi_session,
            match context.settings.wifi {
                WifiMode::AlwaysOn => true,
                WifiMode::Auto => context.wifi_session.has_holders(),
                WifiMode::Off => false,
            },
            tasks,
        );
        cancel_alarms(context, AlarmType::alarms_to_cancel_after_resume()).await;
    }
    reschedule_auto_suspend_alarm(context).await;
    tracing::debug!(
        rotation = context.display.rotation,
        dims = ?context.display.dims,
        framebuffer_rotation = context.device.framebuffer().rotation(),
        "display state when finishing suspend cycle"
    );
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

/// Tears down the view stack and renders the power-off intermission.
pub(crate) async fn show_power_off_intermission(
    context: &mut AppContext,
    view: &mut dyn View,
    history: &mut Vec<HistoryItem>,
    updating: &mut Vec<crate::view::UpdateData>,
) {
    let (tx, _rx) = crate::view::hub_channel();
    view.handle_event(
        &Event::Back,
        &tx,
        &mut crate::view::Bus::new(),
        &mut crate::view::RenderQueue::new(),
        context,
    )
    .await;
    while let Some(mut item) = history.pop() {
        item.view
            .handle_event(
                &Event::Back,
                &tx,
                &mut crate::view::Bus::new(),
                &mut crate::view::RenderQueue::new(),
                context,
            )
            .await;
    }
    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        crate::settings::IntermKind::PowerOff,
        context,
    )
    .await;
    wait_for_all(updating, context).await;
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

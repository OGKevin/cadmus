//! Device input event handling for suspend, power, cover, and USB plug events.

use super::super::input::BATTERY_REFRESH_INTERVAL;
use super::helpers::{cancel_suspend_if_pending, has_task, is_suspend_active};
use super::usb_share::disable_usb_share;
use super::{begin_suspend, schedule_device_task};
use crate::device::DeviceRotation as _;
use crate::device::{AppContext, DeviceRuntime, DeviceTaskId, EventOutcome, Orientation};
use crate::fl;
use crate::framebuffer::UpdateMode;
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent, PowerSource};
use crate::view::dialog::Dialog;
use crate::view::{EntryId, Event, Hub, NotificationEvent, RenderData, RenderQueue, View, ViewId};
use std::process::Command;
use std::time::Instant;

/// Dispatches a lifecycle [`Event`] to the appropriate device-input handler.
pub(super) fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    let Event::Device(device_event) = event else {
        return EventOutcome::Unhandled;
    };
    match device_event {
        DeviceEvent::Button {
            code: ButtonCode::Power,
            status: ButtonStatus::Released,
            ..
        } => handle_power_button_released(hub, bus, rq, context, runtime),
        DeviceEvent::Button {
            code: ButtonCode::Light,
            status: ButtonStatus::Pressed,
            ..
        } => handle_light_button_pressed(hub),
        DeviceEvent::RotateScreen(n) => handle_rotate_screen(*n, hub, context, runtime),
        DeviceEvent::NetUp => handle_net_up(hub, context, runtime),
        DeviceEvent::CoverOn => handle_cover_on(hub, bus, rq, context, runtime),
        DeviceEvent::CoverOff => handle_cover_off(hub, rq, context, runtime),
        DeviceEvent::UserActivity => handle_user_activity(context, runtime),
        DeviceEvent::Plug(source) => handle_plug(*source, hub, rq, context, runtime),
        DeviceEvent::Unplug(..) => handle_unplug(hub, rq, context, runtime),
        _ => EventOutcome::Unhandled,
    }
}

/// Handles a power-button release to begin or cancel suspend.
///
/// Ignored when USB sharing is active or the cover is closed. Toggles between
/// starting suspend via [`begin_suspend`] and cancelling a pending suspend task.
fn handle_power_button_released(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.shared || context.covered {
        return EventOutcome::Handled;
    }

    if has_task(runtime.tasks, DeviceTaskId::PrepareSuspend)
        || has_task(runtime.tasks, DeviceTaskId::Suspend)
    {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    } else {
        begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    }

    EventOutcome::Handled
}

/// Forwards a light-button press as [`Event::ToggleFrontlight`].
fn handle_light_button_pressed(hub: &Hub) -> EventOutcome {
    hub.send(Event::ToggleFrontlight).ok();
    EventOutcome::Handled
}

/// Handles a screen-rotation request from the input pipeline.
///
/// Blocked during USB share, active suspend, or when rotation lock forbids the
/// target orientation. Forwards [`Event::Select(EntryId::Rotate)`] on success.
fn handle_rotate_screen(
    n: i8,
    hub: &Hub,
    context: &AppContext,
    runtime: &DeviceRuntime<'_>,
) -> EventOutcome {
    tracing::debug!(rotation = n, "Gyro rotation");

    if context.shared || is_suspend_active(runtime.tasks) {
        return EventOutcome::Handled;
    }

    if let Some(rotation_lock) = context.settings.rotation_lock {
        let orientation = context.device.orientation(n);
        if rotation_lock == crate::settings::RotationLock::Current
            || (rotation_lock == crate::settings::RotationLock::Portrait
                && orientation == Orientation::Landscape)
            || (rotation_lock == crate::settings::RotationLock::Landscape
                && orientation == Orientation::Portrait)
        {
            return EventOutcome::Handled;
        }
    }

    hub.send(Event::Select(EntryId::Rotate(n))).ok();
    EventOutcome::Handled
}

/// Marks the device online when the network interface comes up.
///
/// Shows a connectivity notification and sets `context.online`. Returns
/// [`EventOutcome::Continue`] so the main loop can dispatch the event to views
/// (including background [`Home`] fetchers when another view is active).
fn handle_net_up(
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if is_suspend_active(runtime.tasks) || context.online {
        return EventOutcome::Handled;
    }

    let ip = Command::new("scripts/ip.sh")
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string()
        })
        .unwrap_or_default();
    let essid = Command::new("scripts/essid.sh")
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string()
        })
        .unwrap_or_default();
    let msg = fl!("notification-network-up", ip = ip, essid = essid);
    hub.send(Event::Notification(NotificationEvent::Show(msg)))
        .ok();

    context.online = true;
    EventOutcome::Continue
}

/// Handles the sleep cover closing.
///
/// Sets `context.covered` and begins suspend when sleep-cover is enabled and
/// no suspend or USB-share session is active.
fn handle_cover_on(
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.covered {
        return EventOutcome::Handled;
    }

    context.covered = true;
    if !context.settings.sleep_cover || context.shared || is_suspend_active(runtime.tasks) {
        return EventOutcome::Handled;
    }

    begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);

    EventOutcome::Handled
}

/// Handles the sleep cover opening.
///
/// Clears `context.covered` and cancels a pending suspend when sleep-cover
/// suspend was active.
fn handle_cover_off(
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if !context.covered {
        return EventOutcome::Handled;
    }

    context.covered = false;
    if context.shared || !context.settings.sleep_cover {
        return EventOutcome::Handled;
    }

    cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);

    EventOutcome::Handled
}

/// Resets inactivity tracking when auto-suspend is enabled.
fn handle_user_activity(context: &AppContext, runtime: &mut DeviceRuntime<'_>) -> EventOutcome {
    if context.settings.auto_suspend > 0.0 {
        *runtime.inactive_since = Instant::now();
    }
    EventOutcome::Handled
}

/// Handles a charger or USB-host plug event.
fn handle_plug(
    power_source: PowerSource,
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.plugged {
        return EventOutcome::Handled;
    }

    context.plugged = true;
    runtime
        .tasks
        .retain(|task| task.id != DeviceTaskId::CheckBattery);

    if context.covered {
        return EventOutcome::Handled;
    }

    match power_source {
        PowerSource::Wall => {
            if has_task(runtime.tasks, DeviceTaskId::Suspend) {
                return EventOutcome::Handled;
            }
        }
        PowerSource::Host => handle_plug_host(hub, rq, context, runtime),
    }

    hub.send(Event::BatteryTick).ok();

    EventOutcome::Handled
}

/// Handles USB-host plug: cancels suspend, prompts or auto-starts USB share.
fn handle_plug_host(
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) {
    cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);

    if context.settings.auto_share {
        hub.send(Event::PrepareShare).ok();
    } else {
        let dialog = Dialog::builder(ViewId::ShareDialog, "Share storage via USB?".to_string())
            .add_button("Cancel", Event::Close(ViewId::ShareDialog))
            .add_button("Share", Event::PrepareShare)
            .build(context);
        rq.add(RenderData::new(
            dialog.id(),
            *dialog.rect(),
            UpdateMode::Gui,
        ));
        runtime.view.children_mut().push(Box::new(dialog));
    }

    *runtime.inactive_since = Instant::now();
}

/// Handles charger or USB-host unplug.
///
/// Disables USB share when active, otherwise reschedules battery checks and
/// may cancel suspend on wall-power removal.
fn handle_unplug(
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if !context.plugged {
        return EventOutcome::Handled;
    }

    if context.shared {
        disable_usb_share(
            context,
            runtime.startup_cwd.and_then(|cwd| cwd.as_deref()),
            &context.settings.logging,
            hub,
        );
    } else {
        context.plugged = false;
        schedule_device_task(
            DeviceTaskId::CheckBattery,
            Event::CheckBattery,
            BATTERY_REFRESH_INTERVAL,
            hub,
            runtime.tasks,
        );
        if has_task(runtime.tasks, DeviceTaskId::Suspend) {
            if !context.covered {
                super::helpers::cancel_suspend_if_pending(
                    context,
                    runtime.tasks,
                    runtime.view.as_mut(),
                    hub,
                    rq,
                );
            }
        } else {
            hub.send(Event::BatteryTick).ok();
        }
    }

    EventOutcome::Handled
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::input::PowerSource;
    use crate::view::EntryId;

    #[test]
    fn handle_power_button_ignored_when_shared() {
        let mut harness = LifecycleHarness::new();
        harness.context.shared = true;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_power_button_released(hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.tasks.is_empty());
    }

    #[test]
    fn handle_power_button_begins_suspend() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_power_button_released(hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_power_button_cancels_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_power_button_released(hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_light_button_forwards_toggle() {
        let harness = LifecycleHarness::new();
        let outcome = handle_light_button_pressed(&harness.hub_tx);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(
            harness
                .drain_hub()
                .iter()
                .any(|e| matches!(e, Event::ToggleFrontlight))
        );
    }

    #[test]
    fn handle_rotate_screen_blocked_during_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::Suspend);
        let hub = harness.hub_tx.clone();
        let outcome = harness
            .with_runtime_only(|context, runtime| handle_rotate_screen(1, &hub, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.drain_hub().is_empty());
    }

    #[test]
    fn handle_rotate_screen_forwards_select() {
        let mut harness = LifecycleHarness::new();
        let hub = harness.hub_tx.clone();
        let outcome = harness
            .with_runtime_only(|context, runtime| handle_rotate_screen(2, &hub, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(
            harness
                .drain_hub()
                .iter()
                .any(|e| matches!(e, Event::Select(EntryId::Rotate(2))))
        );
    }

    #[test]
    fn handle_net_up_sets_online_and_shows_notification() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness
            .with_parts(|hub, _bus, _rq, context, runtime| handle_net_up(hub, context, runtime));
        assert_eq!(outcome, EventOutcome::Continue);
        assert!(harness.context.online);
        assert!(
            harness
                .drain_hub()
                .iter()
                .any(|event| matches!(event, Event::Notification(NotificationEvent::Show(_))))
        );
    }

    #[test]
    fn handle_net_up_noop_when_online() {
        let mut harness = LifecycleHarness::new();
        harness.context.online = true;
        let outcome = harness
            .with_parts(|hub, _bus, _rq, context, runtime| handle_net_up(hub, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.drain_hub().is_empty());
    }

    #[test]
    fn handle_cover_on_sets_covered_and_begins_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.sleep_cover = true;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_cover_on(hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.context.covered);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_cover_off_cancels_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.context.covered = true;
        harness.context.settings.sleep_cover = true;
        harness.push_task(DeviceTaskId::PrepareSuspend);
        let outcome = harness.with_parts(|hub, _bus, rq, context, runtime| {
            handle_cover_off(hub, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!harness.context.covered);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn handle_user_activity_resets_inactive_since() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 300.0;
        let before = harness.inactive_since;
        std::thread::sleep(std::time::Duration::from_millis(5));
        let outcome =
            harness.with_runtime_only(|context, runtime| handle_user_activity(context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.inactive_since > before);
    }

    #[test]
    fn handle_plug_host_auto_share() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_share = true;
        let outcome = harness.with_parts(|hub, _bus, rq, context, runtime| {
            handle_plug(PowerSource::Host, hub, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.context.plugged);
        assert!(
            harness
                .drain_hub()
                .iter()
                .any(|e| matches!(e, Event::PrepareShare))
        );
    }

    #[test]
    fn handle_unplug_reschedules_battery_check() {
        let mut harness = LifecycleHarness::new();
        harness.context.plugged = true;
        let outcome = harness
            .with_parts(|hub, _bus, rq, context, runtime| handle_unplug(hub, rq, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!harness.context.plugged);
        assert!(has_task(&harness.tasks, DeviceTaskId::CheckBattery));
    }

    #[test]
    fn handle_unplug_when_shared_disables_usb() {
        let mut harness = LifecycleHarness::new();
        harness.context.plugged = true;
        harness.context.shared = true;
        let outcome = harness
            .with_parts(|hub, _bus, rq, context, runtime| handle_unplug(hub, rq, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        let usb = harness.context.device.usb_manager_for_test();
        assert_eq!(usb.disable_call_count(), 1);
        assert_eq!(usb.enabled(), Some(false));
        assert!(
            harness
                .drain_hub()
                .iter()
                .any(|e| matches!(e, Event::Select(EntryId::Restart)))
        );
    }
}

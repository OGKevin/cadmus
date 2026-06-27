//! Periodic battery capacity checks for low-battery warnings and auto power-off.

use super::helpers::is_suspend_active;
use super::{
    super::input::BATTERY_REFRESH_INTERVAL, schedule_device_task, show_power_off_intermission,
};
use crate::battery::Battery as _;
use crate::device::DeviceHardware as _;
use crate::device::{AppContext, DeviceRuntime, DeviceTaskId, EventOutcome, ExitStatus};
use crate::fl;
use crate::framebuffer::UpdateMode;
use crate::settings::BatterySettings;
use crate::view::notification::Notification;
use crate::view::{Event, Hub, RenderData, RenderQueue, View};

/// Result of comparing a capacity reading against [`BatterySettings`] thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatteryLevelOutcome {
    Ok,
    Warn,
    PowerOff,
}

/// Classifies `capacity` against configured warn and power-off thresholds.
fn battery_level_outcome(capacity: f32, settings: &BatterySettings) -> BatteryLevelOutcome {
    if capacity < settings.power_off {
        BatteryLevelOutcome::PowerOff
    } else if capacity < settings.warn {
        BatteryLevelOutcome::Warn
    } else {
        BatteryLevelOutcome::Ok
    }
}

/// Handles [`Event::CheckBattery`]: reschedules the check and reacts to low capacity.
///
/// Always schedules the next check. Skips capacity evaluation while suspend is
/// active. Below the power-off threshold triggers [`ExitStatus::PowerOff`]; below
/// the warn threshold shows a transient notification.
pub(super) fn handle_event(
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    schedule_device_task(
        DeviceTaskId::CheckBattery,
        Event::CheckBattery,
        BATTERY_REFRESH_INTERVAL,
        hub,
        runtime.tasks,
    );

    if is_suspend_active(runtime.tasks) {
        return EventOutcome::Handled;
    }

    let Some(capacity) = context
        .device
        .battery_mut()
        .capacity()
        .ok()
        .map(|values| values[0])
    else {
        return EventOutcome::Handled;
    };

    match battery_level_outcome(capacity, &context.settings.battery) {
        BatteryLevelOutcome::PowerOff => {
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            );
            EventOutcome::Exit(ExitStatus::PowerOff)
        }
        BatteryLevelOutcome::Warn => {
            let notif = Notification::new(
                None,
                fl!("notification-battery-low"),
                false,
                hub,
                rq,
                context,
            );
            rq.add(RenderData::new(notif.id(), *notif.rect(), UpdateMode::Gui));
            runtime.view.children_mut().push(Box::new(notif));
            EventOutcome::Handled
        }
        BatteryLevelOutcome::Ok => EventOutcome::Handled,
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::helpers::has_task;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::view::notification::Notification;

    fn settings() -> BatterySettings {
        BatterySettings {
            warn: 10.0,
            power_off: 3.0,
        }
    }

    #[test]
    fn battery_level_outcome_ok_above_warn() {
        assert_eq!(
            battery_level_outcome(50.0, &settings()),
            BatteryLevelOutcome::Ok
        );
    }

    #[test]
    fn battery_level_outcome_warn_between_thresholds() {
        assert_eq!(
            battery_level_outcome(5.0, &settings()),
            BatteryLevelOutcome::Warn
        );
    }

    #[test]
    fn battery_level_outcome_power_off_below_threshold() {
        assert_eq!(
            battery_level_outcome(2.0, &settings()),
            BatteryLevelOutcome::PowerOff
        );
    }

    #[test]
    fn battery_level_outcome_at_warn_boundary() {
        assert_eq!(
            battery_level_outcome(10.0, &settings()),
            BatteryLevelOutcome::Ok
        );
    }

    #[test]
    fn battery_level_outcome_at_power_off_boundary() {
        assert_eq!(
            battery_level_outcome(3.0, &settings()),
            BatteryLevelOutcome::Warn
        );
    }

    #[test]
    fn handle_event_reschedules_task() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness
            .with_parts(|hub, _bus, rq, context, runtime| handle_event(hub, rq, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(has_task(&harness.tasks, DeviceTaskId::CheckBattery));
    }

    #[test]
    fn handle_event_skips_during_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        harness.context.device.battery_mut().set_capacity(1.0);
        let outcome = harness
            .with_parts(|hub, _bus, rq, context, runtime| handle_event(hub, rq, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(has_task(&harness.tasks, DeviceTaskId::CheckBattery));
    }

    #[test]
    fn handle_event_warn_pushes_notification() {
        let mut harness = LifecycleHarness::new();
        harness.context.device.battery_mut().set_capacity(5.0);
        let outcome = harness
            .with_parts(|hub, _bus, rq, context, runtime| handle_event(hub, rq, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(
            harness
                .view
                .children()
                .iter()
                .any(|child| child.is::<Notification>())
        );
    }

    #[test]
    fn handle_event_power_off_exits() {
        let mut harness = LifecycleHarness::new();
        harness.context.device.battery_mut().set_capacity(2.0);
        let outcome = harness
            .with_parts(|hub, _bus, rq, context, runtime| handle_event(hub, rq, context, runtime));
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
    }
}

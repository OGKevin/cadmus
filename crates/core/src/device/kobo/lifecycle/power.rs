//! Power-off and application exit event handling.

use super::{begin_suspend, show_power_off_intermission};
use crate::device::{AppContext, DeviceRuntime, EventOutcome, ExitStatus};
use crate::gesture::GestureEvent;
use crate::input::ButtonCode;
use crate::view::{EntryId, Event, Hub, RenderQueue};

/// Dispatches power-off and exit lifecycle events.
pub(super) fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut crate::view::Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power)) => {
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            );
            EventOutcome::Exit(ExitStatus::PowerOff)
        }
        Event::Select(EntryId::PowerOff) => {
            show_power_off_intermission(
                context,
                runtime.view.as_mut(),
                runtime.history,
                runtime.updating,
            );
            EventOutcome::Exit(ExitStatus::PowerOff)
        }
        Event::Select(EntryId::Restart) => EventOutcome::Exit(ExitStatus::Restart),
        Event::Select(EntryId::Reboot) => EventOutcome::Exit(ExitStatus::Reboot),
        Event::Select(EntryId::Quit) => EventOutcome::Exit(ExitStatus::Quit),
        Event::Select(EntryId::Suspend) => {
            begin_suspend(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
            EventOutcome::Handled
        }
        _ => EventOutcome::Unhandled,
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::DeviceTaskId;
    use crate::device::kobo::lifecycle::helpers::has_task;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;

    #[test]
    fn handle_event_power_off_exits() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Select(EntryId::PowerOff),
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
    fn handle_event_suspend_begins_suspend() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Select(EntryId::Suspend),
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
}

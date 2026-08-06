//! Power-off and application exit event handling.

use super::helpers::{cancel_suspend_if_pending, is_suspend_active};
use super::{begin_suspend, show_power_off_intermission};
use crate::device::DevicePaths as _;
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
            if is_suspend_active(context, runtime.tasks) {
                cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
                return EventOutcome::Handled;
            }
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
        Event::Select(EntryId::SwitchInstall) => {
            match context.device.peer_installs().into_iter().next() {
                Some(peer) => EventOutcome::Exit(ExitStatus::RunCommand(peer.launcher)),
                None => {
                    tracing::error!("SwitchInstall selected but no peer install found");
                    EventOutcome::Handled
                }
            }
        }
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

    struct RemovePeerLauncherOnDrop(std::path::PathBuf);

    impl Drop for RemovePeerLauncherOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn handle_event_switch_install_exits_run_command() {
        let mut harness = LifecycleHarness::new();
        let peer_dir = std::env::temp_dir()
            .join("test-kobo-installation")
            .join(".adds/cadmus");
        let launcher = peer_dir.join("cadmus.sh");
        std::fs::create_dir_all(&peer_dir).unwrap();
        std::fs::write(&launcher, "#!/bin/sh\n").unwrap();
        let _cleanup = RemovePeerLauncherOnDrop(launcher.clone());

        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Select(EntryId::SwitchInstall),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(
            outcome,
            EventOutcome::Exit(ExitStatus::RunCommand(launcher.clone()))
        );
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

    #[test]
    fn hold_button_long_power_cancels_pending_suspend_rtc() {
        use crate::AlarmType;
        use crate::chrono::Duration as ChronoDuration;

        let mut harness = LifecycleHarness::new();
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_alarm(AlarmType::Suspend, ChronoDuration::seconds(15))
                .unwrap();
        }
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power)),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(!alarms.has_alarm(AlarmType::Suspend));
    }

    #[test]
    fn hold_button_long_power_cancels_wake_debounce() {
        use crate::AlarmType;
        use crate::chrono::Duration as ChronoDuration;

        let mut harness = LifecycleHarness::new();
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_alarm(AlarmType::WakeDebounce, ChronoDuration::seconds(15))
                .unwrap();
        }
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power)),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
    }

    #[test]
    fn hold_button_long_power_cancels_pending_prepare_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power)),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn hold_button_long_power_exits_when_no_suspend_pending() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power)),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
    }
}

//! Power-off and application exit event handling.

use crate::device::DevicePaths as _;
use crate::device::suspend::{
    cancel_suspend_if_pending, is_suspend_active, show_power_off_intermission, start_cycle,
};
use crate::device::{AppContext, DeviceRuntime, EventOutcome, ExitStatus};
use crate::gesture::GestureEvent;
use crate::input::ButtonCode;
use crate::view::{EntryId, Event, Hub, RenderQueue};

/// Whether Full inhibit must ignore user power-off / restart / quit / switch-install.
fn user_exit_blocked(context: &AppContext) -> bool {
    context.inhibitor.full_active()
}

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
            if user_exit_blocked(context) {
                return EventOutcome::Handled;
            }
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
            if user_exit_blocked(context) {
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
        Event::Select(EntryId::Restart) => {
            if user_exit_blocked(context) {
                return EventOutcome::Handled;
            }
            EventOutcome::Exit(ExitStatus::Restart)
        }
        Event::Select(EntryId::Reboot) => {
            if user_exit_blocked(context) {
                return EventOutcome::Handled;
            }
            EventOutcome::Exit(ExitStatus::Reboot)
        }
        Event::Select(EntryId::Quit) => {
            if user_exit_blocked(context) {
                return EventOutcome::Handled;
            }
            EventOutcome::Exit(ExitStatus::Quit)
        }
        Event::Select(EntryId::SwitchInstall) => {
            if user_exit_blocked(context) {
                return EventOutcome::Handled;
            }
            match context.device.peer_installs().into_iter().next() {
                Some(peer) => EventOutcome::Exit(ExitStatus::RunCommand(peer.launcher)),
                None => {
                    tracing::error!("SwitchInstall selected but no peer install found");
                    EventOutcome::Handled
                }
            }
        }
        Event::Select(EntryId::Suspend) => {
            start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
            EventOutcome::Handled
        }
        _ => EventOutcome::Unhandled,
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::DeviceTaskId;
    use crate::device::inhibitor::Kind;
    use crate::device::suspend::has_task;
    use crate::device::test_harness::DeviceRuntimeHarness;

    #[test]
    fn handle_event_power_off_exits() {
        let mut harness = DeviceRuntimeHarness::new();
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
    fn full_inhibit_ignores_user_power_off() {
        let mut harness = DeviceRuntimeHarness::new();
        let _guard = harness
            .context
            .inhibitor
            .acquire(Kind::Full, "ota")
            .unwrap();
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
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn full_inhibit_ignores_user_reboot() {
        let mut harness = DeviceRuntimeHarness::new();
        let _guard = harness
            .context
            .inhibitor
            .acquire(Kind::Full, "ota")
            .unwrap();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Select(EntryId::Reboot),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn full_inhibit_allows_exit_after_release() {
        let mut harness = DeviceRuntimeHarness::new();
        let guard = harness
            .context
            .inhibitor
            .acquire(Kind::Full, "ota")
            .unwrap();
        drop(guard);
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(
                &Event::Select(EntryId::Reboot),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::Reboot));
    }

    struct RemovePeerLauncherOnDrop(std::path::PathBuf);

    impl Drop for RemovePeerLauncherOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn handle_event_switch_install_exits_run_command() {
        let mut harness = DeviceRuntimeHarness::new();
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
        let mut harness = DeviceRuntimeHarness::new();
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

        let mut harness = DeviceRuntimeHarness::new();
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
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

        let mut harness = DeviceRuntimeHarness::new();
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::WakeDebounce, ChronoDuration::seconds(15))
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
        let mut harness = DeviceRuntimeHarness::new();
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
        let mut harness = DeviceRuntimeHarness::new();
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

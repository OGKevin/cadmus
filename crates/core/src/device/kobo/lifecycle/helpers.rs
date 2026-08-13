//! Shared suspend-task predicates for lifecycle handlers.

use super::cancel_suspend;
use super::finish_suspend_cycle;
use super::suspend::{cancel_suspend_rtcs, is_suspend_rtc_pending};
use crate::device::AppContext;
use crate::device::{DeviceTask, DeviceTaskId};
use crate::view::{RenderQueue, View};
use std::sync::mpsc::Sender;

/// Returns whether `tasks` contains a task with the given `id`.
pub(super) fn has_task(tasks: &[DeviceTask], id: DeviceTaskId) -> bool {
    tasks.iter().any(|task| task.id == id)
}

/// Returns whether a suspend flow is in progress.
///
/// True when PrepareSuspend is pending, a Suspend / WakeDebounce RTC is armed in
/// the map (including past-due), or [`AppContext::suspend_cycle_active`] is set.
pub(super) fn is_suspend_active(context: &AppContext, tasks: &[DeviceTask]) -> bool {
    has_task(tasks, DeviceTaskId::PrepareSuspend)
        || is_suspend_rtc_pending(context)
        || context.suspend_cycle_active
}

/// Cancels an in-progress suspend when PrepareSuspend or a suspend RTC is pending.
///
/// Prefer [`super::begin_suspend`] to start suspend and [`cancel_suspend`] when the
/// caller already knows PrepareSuspend is pending.
pub(super) fn cancel_suspend_if_pending(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Sender<crate::view::Event>,
    rq: &mut RenderQueue,
) {
    let should_finish = is_suspend_rtc_pending(context) || context.suspend_cycle_active;
    cancel_suspend_rtcs(context);
    if has_task(tasks, DeviceTaskId::PrepareSuspend) {
        cancel_suspend(context, DeviceTaskId::PrepareSuspend, tasks, view, hub, rq);
    } else if should_finish {
        finish_suspend_cycle(context, tasks, view, hub, rq);
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::AlarmType;
    use crate::chrono::Duration as ChronoDuration;
    use crate::device::kobo::lifecycle::suspend::is_suspend_alarm_scheduled;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;

    #[test]
    fn has_task_empty() {
        let harness = LifecycleHarness::new();
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn has_task_present() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn is_suspend_active_prepare() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn is_suspend_active_suspend_rtc() {
        let harness = LifecycleHarness::new();
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
        assert!(is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn is_suspend_active_false() {
        let harness = LifecycleHarness::new();
        assert!(!is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn is_suspend_active_past_due_suspend_rtc() {
        let harness = LifecycleHarness::new();
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(-1))
                .unwrap();
            assert!(alarms.has_alarm(AlarmType::Suspend));
            assert!(!alarms.is_alarm_scheduled(AlarmType::Suspend));
        }
        assert!(is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn is_suspend_active_cycle_flag_without_rtc() {
        let mut harness = LifecycleHarness::new();
        harness.context.suspend_cycle_active = true;
        assert!(is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn cancel_suspend_if_pending_prepare() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn cancel_suspend_if_pending_suspend_rtc() {
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
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
                .unwrap();
        }
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(!is_suspend_alarm_scheduled(&harness.context));
    }

    #[test]
    fn cancel_suspend_if_pending_wake_debounce() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
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
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn cancel_suspend_if_pending_noop() {
        let mut harness = LifecycleHarness::new();
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(harness.tasks.is_empty());
    }

    #[test]
    fn cancel_suspend_if_pending_past_due_suspend_rtc() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.context.suspend_cycle_active = true;
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(-1))
                .unwrap();
        }
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(!is_suspend_alarm_scheduled(&harness.context));
        assert!(!harness.context.suspend_cycle_active);
        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend));
    }

    #[test]
    fn cancel_suspend_if_pending_after_claim_uses_cycle_flag() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.context.suspend_cycle_active = true;
        assert!(!is_suspend_rtc_pending(&harness.context));
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(!harness.context.suspend_cycle_active);
        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend));
    }
}

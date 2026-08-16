//! Suspend-task predicates shared by device lifecycle handlers.
//!
//! Not Kobo-specific — any platform that drives [`super`] can use these.

use super::orchestrator::{
    cancel_prepare, cancel_suspend_rtcs, finish_cycle, is_suspend_rtc_pending,
};
use crate::device::AppContext;
use crate::device::{DeviceTask, DeviceTaskId};
use crate::view::{Hub, RenderQueue, View};

/// Returns whether `tasks` contains a task with the given `id`.
pub(crate) fn has_task(tasks: &[DeviceTask], id: DeviceTaskId) -> bool {
    tasks.iter().any(|task| task.id == id)
}

/// Returns whether a suspend flow is in progress.
///
/// True when PrepareSuspend is pending, a Suspend / WakeDebounce RTC is armed in
/// the map (including past-due), or an explicit [`AppContext::suspend`] cycle is set.
pub(crate) fn is_suspend_active(context: &AppContext, tasks: &[DeviceTask]) -> bool {
    has_task(tasks, DeviceTaskId::PrepareSuspend)
        || is_suspend_rtc_pending(context)
        || context.suspend.is_some()
}

/// Cancels an in-progress suspend when PrepareSuspend or a suspend RTC/cycle is pending.
///
/// Prefer [`finish_cycle`] whenever a [`AppContext::suspend`] cycle exists — including
/// DeepIdle reentry while `PrepareSuspend` is pending after teardown already turned
/// frontlight/WiFi off. [`cancel_prepare`] only covers a PrepareSuspend task with no cycle.
pub(crate) fn cancel_suspend_if_pending(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut RenderQueue,
) {
    let had_cycle = context.suspend.is_some();
    let had_suspend_rtc = is_suspend_rtc_pending(context);
    cancel_suspend_rtcs(context);
    if had_cycle {
        finish_cycle(context, tasks, view, hub, rq);
    } else if has_task(tasks, DeviceTaskId::PrepareSuspend) {
        cancel_prepare(context, DeviceTaskId::PrepareSuspend, tasks, view, hub, rq);
    } else if had_suspend_rtc {
        finish_cycle(context, tasks, view, hub, rq);
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::super::cycle::{SuspendCycle, SuspendKind};
    use super::*;
    use crate::AlarmType;
    use crate::chrono::Duration as ChronoDuration;
    use crate::device::test_harness::DeviceRuntimeHarness;

    #[test]
    fn has_task_empty() {
        let harness = DeviceRuntimeHarness::new();
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn has_task_present() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[test]
    fn is_suspend_active_prepare() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn is_suspend_active_suspend_rtc() {
        let harness = DeviceRuntimeHarness::new();
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
        let harness = DeviceRuntimeHarness::new();
        assert!(!is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn is_suspend_active_past_due_suspend_rtc() {
        let harness = DeviceRuntimeHarness::new();
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
    fn is_suspend_active_cycle_without_rtc() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.suspend = Some(SuspendCycle::new(SuspendKind::Classic));
        assert!(is_suspend_active(&harness.context, &harness.tasks));
    }

    #[test]
    fn cancel_suspend_if_pending_prepare() {
        let mut harness = DeviceRuntimeHarness::new();
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
        assert!(!alarms.has_alarm(AlarmType::Suspend));
    }

    #[test]
    fn cancel_suspend_if_pending_wake_debounce() {
        let mut harness = DeviceRuntimeHarness::new();
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
        let mut harness = DeviceRuntimeHarness::new();
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
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.context.suspend = Some(SuspendCycle::new(SuspendKind::Classic));
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
        assert!(!is_suspend_rtc_pending(&harness.context));
        assert!(harness.context.suspend.is_none());
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
    fn cancel_suspend_if_pending_after_claim_uses_cycle() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.auto_suspend = 30.0;
        harness.context.suspend = Some(SuspendCycle::new(SuspendKind::Classic));
        assert!(!is_suspend_rtc_pending(&harness.context));
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(harness.context.suspend.is_none());
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

//! Suspend-task predicates shared by device lifecycle handlers.
//!
//! Not Kobo-specific — any platform that drives [`super`] can use these.

#[cfg(test)]
use super::orchestrator::DeferredSuspend;
use super::orchestrator::{finish_cycle, is_suspend_rtc_pending};
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
pub(crate) async fn is_suspend_active(context: &AppContext, tasks: &[DeviceTask]) -> bool {
    has_task(tasks, DeviceTaskId::PrepareSuspend)
        || is_suspend_rtc_pending(context).await
        || context.suspend.is_some()
}

/// Cancels an in-progress suspend when PrepareSuspend, a suspend RTC, or a cycle is pending.
///
/// [`finish_cycle`] owns the whole teardown for every shape, including a
/// PrepareSuspend-only cancel after a replacement cycle turned frontlight and
/// WiFi off.
pub(crate) async fn cancel_suspend_if_pending(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut RenderQueue,
) {
    if !is_suspend_active(context, tasks).await {
        return;
    }
    finish_cycle(context, tasks, view, hub, rq).await;
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::super::cycle::{SuspendCycle, SuspendKind};
    use super::*;
    use crate::AlarmType;
    use crate::chrono::Duration as ChronoDuration;
    use crate::device::test_harness::DeviceRuntimeHarness;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn has_task_empty() {
        let harness = DeviceRuntimeHarness::new().await;
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn has_task_present() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_suspend_active_prepare() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(is_suspend_active(&harness.context, &harness.tasks).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_suspend_active_suspend_rtc() {
        let harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
                .await
                .unwrap();
        }
        assert!(is_suspend_active(&harness.context, &harness.tasks).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_suspend_active_false() {
        let harness = DeviceRuntimeHarness::new().await;
        assert!(!is_suspend_active(&harness.context, &harness.tasks).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_suspend_active_past_due_suspend_rtc() {
        let harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(-1))
                .await
                .unwrap();
            assert!(alarms.has_alarm(AlarmType::Suspend));
            assert!(!alarms.is_alarm_scheduled(AlarmType::Suspend).await);
        }
        assert!(is_suspend_active(&harness.context, &harness.tasks).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_suspend_active_cycle_without_rtc() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.suspend = Some(SuspendCycle::new(
            SuspendKind::Classic,
            DeferredSuspend::Explicit,
        ));
        assert!(is_suspend_active(&harness.context, &harness.tasks).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_suspend_if_pending_prepare() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.push_task(DeviceTaskId::PrepareSuspend);
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
        assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_suspend_if_pending_suspend_rtc() {
        let mut harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
                .await
                .unwrap();
        }
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(!alarms.has_alarm(AlarmType::Suspend));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_suspend_if_pending_wake_debounce() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.auto_suspend = 30.0;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::WakeDebounce, ChronoDuration::seconds(15))
                .await
                .unwrap();
        }
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_suspend_if_pending_noop() {
        let mut harness = DeviceRuntimeHarness::new().await;
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
        assert!(harness.tasks.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_suspend_if_pending_past_due_suspend_rtc() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.auto_suspend = 30.0;
        harness.context.suspend = Some(SuspendCycle::new(
            SuspendKind::Classic,
            DeferredSuspend::Explicit,
        ));
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(-1))
                .await
                .unwrap();
        }
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
        assert!(!is_suspend_rtc_pending(&harness.context).await);
        assert!(harness.context.suspend.is_none());
        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_suspend_if_pending_after_claim_uses_cycle() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.auto_suspend = 30.0;
        harness.context.suspend = Some(SuspendCycle::new(
            SuspendKind::Classic,
            DeferredSuspend::Explicit,
        ));
        assert!(!is_suspend_rtc_pending(&harness.context).await);
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
        assert!(harness.context.suspend.is_none());
        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend).await);
    }
}

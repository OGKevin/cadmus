//! Shared suspend-task predicates for lifecycle handlers.

use super::cancel_suspend;
use crate::device::AppContext;
use crate::device::{DeviceTask, DeviceTaskId};
use crate::view::{RenderQueue, View};
use std::sync::mpsc::Sender;

/// Returns whether `tasks` contains a task with the given `id`.
pub(super) fn has_task(tasks: &[DeviceTask], id: DeviceTaskId) -> bool {
    tasks.iter().any(|task| task.id == id)
}

/// Returns whether a suspend flow is in progress (`PrepareSuspend` or `Suspend`).
pub(super) fn is_suspend_active(tasks: &[DeviceTask]) -> bool {
    has_task(tasks, DeviceTaskId::PrepareSuspend) || has_task(tasks, DeviceTaskId::Suspend)
}

/// Cancels an in-progress suspend when `PrepareSuspend` or `Suspend` is pending.
///
/// Prefer [`super::begin_suspend`] to start suspend and [`cancel_suspend`] when the
/// caller already knows which task id to cancel.
pub(super) fn cancel_suspend_if_pending(
    context: &mut AppContext,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Sender<crate::view::Event>,
    rq: &mut RenderQueue,
) {
    if has_task(tasks, DeviceTaskId::PrepareSuspend) {
        cancel_suspend(context, DeviceTaskId::PrepareSuspend, tasks, view, hub, rq);
    } else if has_task(tasks, DeviceTaskId::Suspend) {
        cancel_suspend(context, DeviceTaskId::Suspend, tasks, view, hub, rq);
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;

    #[test]
    fn has_task_empty() {
        let harness = LifecycleHarness::new();
        assert!(!has_task(&harness.tasks, DeviceTaskId::Suspend));
    }

    #[test]
    fn has_task_present() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
        assert!(!has_task(&harness.tasks, DeviceTaskId::Suspend));
    }

    #[test]
    fn is_suspend_active_prepare() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::PrepareSuspend);
        assert!(is_suspend_active(&harness.tasks));
    }

    #[test]
    fn is_suspend_active_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::Suspend);
        assert!(is_suspend_active(&harness.tasks));
    }

    #[test]
    fn is_suspend_active_false() {
        let harness = LifecycleHarness::new();
        assert!(!is_suspend_active(&harness.tasks));
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
    fn cancel_suspend_if_pending_suspend() {
        let mut harness = LifecycleHarness::new();
        harness.push_task(DeviceTaskId::Suspend);
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        );
        assert!(!has_task(&harness.tasks, DeviceTaskId::Suspend));
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
}

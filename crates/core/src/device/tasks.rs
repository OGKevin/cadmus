//! Delayed device tasks posted back to the main-loop hub.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::device::{DeviceTask, DeviceTaskId};
use crate::view::{Event, Hub};

/// Schedules a delayed [`Event`] and tracks it in `tasks`.
///
/// Replaces any existing task with the same [`DeviceTaskId`], cancelling the
/// superseded one instead of leaving it alive until its own delay expires. The
/// wait runs on the shared runtime and ends early when the [`DeviceTask`] is
/// dropped, for example when the task is cleared.
pub(crate) fn schedule_device_task(
    id: DeviceTaskId,
    event: Event,
    delay: Duration,
    hub: &Hub,
    tasks: &mut Vec<DeviceTask>,
) {
    let hub = hub.clone();
    tasks.retain(|task| task.id != id);
    let job = crate::runtime::Job::spawn(move |cancel| post_after(delay, event, hub, cancel));
    tasks.push(DeviceTask { id, job });
}

/// Waits `delay`, then posts `event` unless the task was cleared meanwhile.
///
/// The cancel check is repeated after the sleep because a task cleared in the
/// same tick still owns a live token until its [`DeviceTask`] is dropped, so
/// the token alone does not prove the task is still wanted.
async fn post_after(delay: Duration, event: Event, hub: Hub, cancel: CancellationToken) {
    tokio::select! {
        biased;
        () = cancel.cancelled() => return,
        () = tokio::time::sleep(delay) => {}
    }
    if !cancel.is_cancelled() {
        hub.send(event.into()).ok();
    }
}

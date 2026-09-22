//! Delayed device tasks posted back to the main-loop hub.

use crate::device::{DeviceTask, DeviceTaskId};
use crate::view::{Event, Hub};
use std::sync::mpsc;
use std::time::Duration;

/// Schedules a delayed [`Event`] and tracks it in `tasks`.
///
/// Replaces any existing task with the same [`DeviceTaskId`]. The delayed
/// event is dropped when the receiver side is closed, for example when the
/// task is superseded or cleared. The wait runs on the shared runtime.
pub(crate) fn schedule_device_task(
    id: DeviceTaskId,
    event: Event,
    delay: Duration,
    hub: &Hub,
    tasks: &mut Vec<DeviceTask>,
) {
    let (ty, ry) = mpsc::channel();
    let hub2 = hub.clone();
    tasks.retain(|task| task.id != id);
    tasks.push(DeviceTask { id, _chan: ry });
    crate::runtime::current_handle().spawn(async move {
        tokio::time::sleep(delay).await;
        if ty.send(()).is_ok() {
            hub2.send(event.into()).ok();
        }
    });
}

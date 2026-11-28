//! Delayed device tasks posted back to the main-loop hub.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::device::wifi::WifiSession;
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

/// Replaces any owned radio reconciliation with one carrying the latest intent.
///
/// Intent is published before the owned job starts, and the job calls
/// `apply_radio`: a late start therefore reconciles the *current* radio intent
/// rather than overwriting it with the state at scheduling time. Completion is
/// tagged with the new generation so the event loop can reject stale results
/// even when they were queued before the job was invalidated.
pub(crate) fn spawn_wifi_reconcile(
    hub: &Hub,
    session: &Arc<WifiSession>,
    desired_on: bool,
    tasks: &mut Vec<DeviceTask>,
) {
    session.set_desired_radio_on(desired_on);
    let generation = session.next_reconcile_generation();
    tasks.retain(|task| task.id != DeviceTaskId::WifiReconcile);
    let session = Arc::clone(session);
    let hub = hub.clone();
    let job = crate::runtime::Job::spawn(move |cancel| async move {
        if cancel.is_cancelled() {
            return;
        }
        match session.apply_radio().await {
            Ok(connected) => {
                let enabled = session.wifi_manager().is_enabled().await;
                tracing::info!(desired_on, enabled, connected, "wifi reconcile");
                if connected {
                    hub.send(Event::WifiReconciled(generation).into()).ok();
                }
            }
            Err(error) => {
                tracing::error!(error = %error, desired_on, "Failed to reconcile WiFi radio");
            }
        }
    });
    tasks.push(DeviceTask {
        id: DeviceTaskId::WifiReconcile,
        job,
    });
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

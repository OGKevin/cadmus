use std::time::Duration;

use crate::task::{BackgroundTask, TaskFuture, TaskId, sleep_unless_cancelled};
use crate::view::Event;
use tokio_util::sync::CancellationToken;

const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Background task that periodically asks the UI loop to recompute and apply
/// automatic frontlight levels.
#[derive(Default)]
pub struct AutoFrontlightTask;

impl BackgroundTask for AutoFrontlightTask {
    fn id(&self) -> TaskId {
        TaskId::AutoFrontlight
    }

    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            while !cancel.is_cancelled() {
                if let Err(e) = hub.send((Event::UpdateAutoFrontlight).into()) {
                    tracing::error!(error = %e, "failed to send auto-frontlight update event");
                    break;
                }

                if sleep_unless_cancelled(cancel, CHECK_INTERVAL).await {
                    break;
                }
            }
        })
    }
}

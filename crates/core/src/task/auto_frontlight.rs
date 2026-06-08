use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::view::Event;

const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Background task that periodically asks the UI loop to recompute and apply
/// automatic frontlight levels.
#[derive(Default)]
pub struct AutoFrontlightTask;

impl BackgroundTask for AutoFrontlightTask {
    fn id(&self) -> TaskId {
        TaskId::AutoFrontlight
    }

    fn run(&mut self, hub: &Sender<Event>, shutdown: &ShutdownSignal) {
        while !shutdown.should_stop() {
            if let Err(e) = hub.send(Event::UpdateAutoFrontlight) {
                tracing::error!(error = %e, "failed to send auto-frontlight update event");
                break;
            }

            if shutdown.wait(CHECK_INTERVAL) {
                break;
            }
        }
    }
}

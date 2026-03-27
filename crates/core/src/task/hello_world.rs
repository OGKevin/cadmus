//! Example background task for testing the task infrastructure.
//!
//! This task prints "Hello world!" every minute. It is only compiled
//! when the `test` feature is enabled.

use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::view::Event;

const PRINT_INTERVAL: Duration = Duration::from_secs(60);

/// Example task that prints a message periodically.
///
/// This serves as a reference implementation for the [`BackgroundTask`] trait
/// and validates that the task infrastructure works correctly.
pub struct HelloWorldTask;

impl BackgroundTask for HelloWorldTask {
    fn id(&self) -> TaskId {
        TaskId::HelloWorld
    }

    fn run(&mut self, _hub: &Sender<Event>, shutdown: &ShutdownSignal) {
        tracing::info!("hello_world task started");

        loop {
            {
                #[cfg(feature = "otel")]
                let _span = tracing::info_span!("hello_world_tick").entered();
                tracing::info!("Hello world!");
            }

            if shutdown.wait(PRINT_INTERVAL) {
                break;
            }
        }

        tracing::info!("hello_world task stopped");
    }
}

//! Example background task for testing the task infrastructure.
//!
//! This task prints "Hello world!" every minute. It is only compiled
//! when the `test` feature is enabled.

use std::time::Duration;

use crate::task::{BackgroundTask, TaskFuture, TaskId, sleep_unless_cancelled};
use tokio_util::sync::CancellationToken;

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

    fn run<'a>(
        &'a mut self,
        _hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            tracing::info!("hello_world task started");

            loop {
                {
                    #[cfg(feature = "tracing")]
                    let _span = tracing::info_span!("hello_world_tick").entered();
                    tracing::info!("Hello world!");
                }

                if sleep_unless_cancelled(cancel, PRINT_INTERVAL).await {
                    break;
                }
            }

            tracing::info!("hello_world task stopped");
        })
    }
}

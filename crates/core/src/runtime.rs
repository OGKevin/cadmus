//! Process-wide Tokio runtime for sync→async bridges.
//!
//! Cadmus’s main event loop is synchronous. Async work (SQLx, zbus, …) is
//! driven via [`RUNTIME::block_on`](RUNTIME) from call sites that must stay
//! sync. Long-lived background tasks that own an event loop keep their own
//! runtime so they do not monopolize this one with forever-running `block_on`.

use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

/// Global lazy-initialized Tokio runtime for process-wide sync→async bridges.
pub static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tracing::info!("initializing process-wide Tokio runtime");
    Runtime::new().expect("failed to create process-wide Tokio runtime")
});

//! Shared shutdown utilities for telemetry subsystems.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Executes a shutdown closure with a timeout to prevent indefinite blocking.
///
/// Spawns a dedicated thread to run the provided closure and waits up to
/// `timeout` for completion. If the closure does not finish within the
/// timeout, the function returns and the spawned thread is left detached.
///
/// # Resource Leak Warning
///
/// This function **must only be called during application shutdown**. The
/// spawned thread is not joined on timeout — if called outside of shutdown,
/// the detached thread constitutes a resource leak.
///
/// # Arguments
///
/// * `shutdown` - The shutdown operation to execute (e.g., flushing buffers,
///   closing connections).
/// * `timeout` - Maximum time to wait for the operation to complete.
pub fn shutdown_with_timeout(shutdown: impl FnOnce() + Send + 'static, timeout: Duration) {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        shutdown();
        let _ = tx.send(());
    });

    let _ = rx.recv_timeout(timeout);
}

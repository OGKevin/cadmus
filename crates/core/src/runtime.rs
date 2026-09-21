//! Process-wide Tokio runtime and the sync-to-async blocking facade.
//!
//! Process entry builds one multi-thread runtime. The reactor is capped.
//! The blocking pool uses Tokio's ceiling and creates a thread only when
//! work is waiting. Synchronous callers use [`block_on`], which parks a
//! runtime worker with `block_in_place` instead of nesting a bare `block_on`.
//!
//! [`enter`] (and the first [`current_handle`] on a live runtime) publish one
//! process [`Handle`] so leftover `std::thread` tasks can still drive work on
//! that runtime. Calling [`block_on`] or [`current_handle`] before any process
//! runtime exists is a bug.

use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::{Builder, Handle};
use tokio::task::JoinSet;

/// Reactor threads. Not the pool that runs blocking work.
pub const WORKER_THREADS: usize = 2;

/// Ceiling for the blocking pool.
///
/// This is Tokio's own default. A thread is created when a blocking job is
/// waiting and every existing one is busy, and idle threads exit on their
/// own. Blocking jobs share this pool.
pub const MAX_BLOCKING_THREADS: usize = 512;

/// How long shutdown waits for in-flight async work before aborting it.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

/// Multi-thread builder with both thread pools capped.
pub fn builder() -> Builder {
    let mut builder = Builder::new_multi_thread();
    builder
        .worker_threads(WORKER_THREADS)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .enable_all();
    builder
}

/// Drives `future` on the process runtime.
///
/// `#[tokio::main]` cannot set [`MAX_BLOCKING_THREADS`], so process entry uses
/// this builder instead of the attribute macro.
///
/// After `future` finishes, blocking tasks have [`SHUTDOWN_DEADLINE`] to
/// return. A task blocked in a read would otherwise hold process exit open.
pub fn enter<F, T>(future: F) -> T
where
    F: Future<Output = T>,
{
    let runtime = builder().build().expect("failed to build process runtime");
    publish_process_handle(runtime.handle().clone());
    let result = runtime.block_on(future);
    runtime.shutdown_timeout(SHUTDOWN_DEADLINE);
    clear_process_handle();
    result
}

/// Handle for the process runtime.
///
/// Prefers the caller's current runtime, then the handle published by
/// [`enter`] or an earlier on-runtime [`current_handle`] call.
///
/// # Panics
///
/// Panics when no process runtime exists yet. Use [`enter`] at process entry,
/// or `#[tokio::test]` in tests.
#[track_caller]
pub fn current_handle() -> Handle {
    if let Ok(handle) = Handle::try_current() {
        publish_process_handle(handle.clone());
        return handle;
    }
    process_handle().unwrap_or_else(|| {
        panic!(
            "cadmus runtime handle required; call runtime::enter at process entry or use #[tokio::test] (at {})",
            std::panic::Location::caller()
        )
    })
}

/// Runs `future` to completion on the process runtime.
///
/// Parks the worker via `block_in_place` when called from a runtime thread.
/// Off-worker callers (plain OS threads) use [`Handle::block_on`] directly.
///
/// # Panics
///
/// Panics when no process runtime exists yet. Use [`enter`] at process entry,
/// or `#[tokio::test]` in tests.
#[track_caller]
pub fn block_on<F: Future>(future: F) -> F::Output {
    let handle = current_handle();
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        handle.block_on(future)
    }
}

/// Bare `Handle::block_on` from a runtime worker.
///
/// Debug builds panic and name the caller. Release builds perform the nested
/// block, which Tokio rejects on a worker that is not parked.
#[cfg(test)]
#[track_caller]
fn block_on_bare<F: Future>(future: F) -> F::Output {
    let caller = std::panic::Location::caller();
    let handle = current_handle();
    if cfg!(debug_assertions) {
        panic!("bare nested block_on from a runtime worker at {caller}");
    }
    handle.block_on(future)
}

/// Waits for registered async work, then aborts whatever is still running.
pub async fn shutdown() {
    let mut in_flight = std::mem::take(&mut *in_flight().lock().await);
    finish_within_deadline(&mut in_flight, SHUTDOWN_DEADLINE).await;
}

/// Spawns `future` onto the shutdown set. Must run on the process runtime.
pub async fn spawn_in_flight<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    in_flight().lock().await.spawn(future);
}

/// Waits up to `deadline` for `in_flight` to finish, then aborts the rest.
pub async fn finish_within_deadline(in_flight: &mut JoinSet<()>, deadline: Duration) {
    let deadline_at = tokio::time::Instant::now() + deadline;
    loop {
        if in_flight.is_empty() {
            return;
        }
        let remaining = deadline_at.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            report_shutdown_deadline(deadline);
            in_flight.abort_all();
            return;
        }
        tokio::select! {
            joined = in_flight.join_next() => {
                if joined.is_none() {
                    return;
                }
            }
            () = tokio::time::sleep(remaining) => {
                report_shutdown_deadline(deadline);
                in_flight.abort_all();
                return;
            }
        }
    }
}

fn report_shutdown_deadline(deadline: Duration) {
    tracing::error!(
        deadline_ms = deadline.as_millis() as u64,
        "async shutdown deadline exceeded"
    );
}

fn in_flight() -> &'static tokio::sync::Mutex<JoinSet<()>> {
    static IN_FLIGHT: OnceLock<tokio::sync::Mutex<JoinSet<()>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| tokio::sync::Mutex::new(JoinSet::new()))
}

fn process_handle_slot() -> &'static Mutex<Option<Handle>> {
    static PROCESS_HANDLE: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
    PROCESS_HANDLE.get_or_init(|| Mutex::new(None))
}

fn publish_process_handle(handle: Handle) {
    *process_handle_slot().lock().expect("process handle lock") = Some(handle);
}

fn clear_process_handle() {
    *process_handle_slot().lock().expect("process handle lock") = None;
}

fn process_handle() -> Option<Handle> {
    process_handle_slot()
        .lock()
        .expect("process handle lock")
        .clone()
}

/// Publishes a process [`Handle`] for unit tests that are not on `#[tokio::test]`.
///
/// Prefer `#[tokio::test]` for new tests. Helpers such as
/// [`crate::context::test_helpers::create_test_context`] call this so
/// plain `#[test]` cases still reach [`block_on`] under nextest isolation.
#[cfg(test)]
pub(crate) fn ensure_published_for_test() {
    if process_handle().is_some() {
        return;
    }
    if let Ok(handle) = Handle::try_current() {
        publish_process_handle(handle);
        return;
    }
    static TEST_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = TEST_RUNTIME.get_or_init(|| {
        builder()
            .build()
            .expect("failed to build test process runtime")
    });
    publish_process_handle(runtime.handle().clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[should_panic(expected = "bare nested block_on from a runtime worker")]
    async fn bare_nested_block_on_panics_on_a_runtime_worker() {
        block_on_bare(async {});
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_returns_within_deadline_with_work_in_flight() {
        let mut in_flight = JoinSet::new();
        in_flight.spawn(std::future::pending());
        let deadline = Duration::from_millis(50);
        let started = Instant::now();
        finish_within_deadline(&mut in_flight, deadline).await;
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hub_accepts_sync_send_from_a_non_runtime_thread() {
        let (tx, mut rx) = crate::view::hub_channel();
        std::thread::spawn(move || {
            tx.send(crate::view::Event::ClockTick.into()).unwrap();
        })
        .join()
        .unwrap();
        let message = rx.recv().await.expect("hub message");
        assert!(matches!(message.event, crate::view::Event::ClockTick));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_on_works_on_the_test_runtime() {
        assert_eq!(block_on(async { 7 }), 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_on_works_from_an_os_thread_after_handle_published() {
        let _ = current_handle();
        let value = std::thread::spawn(|| block_on(async { 11 }))
            .join()
            .expect("os thread");
        assert_eq!(value, 11);
    }

    #[test]
    fn enter_drives_block_on_from_the_process_runtime() {
        enter(async {
            assert_eq!(block_on(async { 7 }), 7);
        });
    }
}

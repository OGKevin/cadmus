//! Process Tokio runtime and the sync-to-async blocking facade.
//!
//! Process entry builds one multi-thread runtime. The reactor is capped.
//! The blocking pool uses Tokio's ceiling and creates a thread only when
//! work is waiting. Synchronous callers use [`block_on`], which parks a
//! runtime worker with `block_in_place`.
//!
//! There is no process-global [`Handle`]. Callers must already be on the
//! runtime ([`enter`] or `#[tokio::test]`). Off-runtime work belongs on
//! `spawn_blocking` / `tokio::spawn`, or takes an explicit handle at spawn.

use std::future::Future;
use std::io::Write;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::{Builder, Handle};
use tokio::task::JoinSet;

/// Reactor threads. Not the pool that runs blocking work.
///
/// With only two workers, CPU-heavy or `std` I/O that runs inline in an
/// `async fn` (for example hashing a whole library file with BLAKE3, or
/// `Command::output` for Wi-Fi scripts) can stall timers, input, and other
/// tasks. That work belongs on [`spawn_blocking`] or `tokio::process`, not
/// on these threads.
pub const WORKER_THREADS: usize = 2;

/// Ceiling for the blocking pool.
///
/// This is Tokio's own default. A thread is created when a blocking job is
/// waiting and every existing one is busy, and idle threads exit on their
/// own. Blocking jobs share this pool.
pub const MAX_BLOCKING_THREADS: usize = 512;

/// How long shutdown waits for in-flight async work before aborting it.
///
/// Tokio's blocking pool cannot cancel a stuck `recv`/`sleep` in a
/// `spawn_blocking` closure. This ceiling keeps process exit finite after
/// those loops are asked to stop (closed pipes, stop flags). Shrink only after
/// every long-lived blocking task exits promptly on shutdown.
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
/// Process entry uses this builder instead of `#[tokio::main]` so exit can
/// apply [`SHUTDOWN_DEADLINE`] after the future returns. The blocking-pool
/// ceiling matches Tokio's default ([`MAX_BLOCKING_THREADS`]).
///
/// After `future` finishes, blocking tasks have [`SHUTDOWN_DEADLINE`] to
/// return. A task blocked in a read would otherwise hold process exit open.
pub fn enter<F, T>(future: F) -> T
where
    F: Future<Output = T>,
{
    let runtime = builder().build().expect("failed to build process runtime");
    let result = runtime.block_on(future);
    runtime.shutdown_timeout(SHUTDOWN_DEADLINE);
    result
}

/// Runs `f` on the blocking pool of the current runtime.
pub fn spawn_blocking<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    current_handle().spawn_blocking(f)
}

/// Handle for the caller's current runtime.
///
/// # Panics
///
/// Panics when not running on a Tokio runtime. Use [`enter`] at process entry,
/// or `#[tokio::test]` in tests.
#[track_caller]
pub fn current_handle() -> Handle {
    Handle::try_current().unwrap_or_else(|_| {
        panic!(
            "cadmus runtime handle required; call runtime::enter at process entry or use #[tokio::test] (at {})",
            std::panic::Location::caller()
        )
    })
}

/// Runs `future` to completion on the current runtime.
///
/// Parks the worker via `block_in_place`. Must be called from a **multi-thread**
/// runtime worker (`#[tokio::test(flavor = "multi_thread")]` or [`enter`]).
/// The default `#[tokio::test]` current-thread flavor panics on
/// `block_in_place`.
///
/// # Panics
///
/// Panics when not running on a Tokio runtime, or when called from a
/// current-thread runtime that cannot park.
#[track_caller]
pub fn block_on<F: Future>(future: F) -> F::Output {
    let handle = current_handle();
    tokio::task::block_in_place(|| handle.block_on(future))
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
            report_shutdown_deadline(deadline, in_flight.len());
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
                report_shutdown_deadline(deadline, in_flight.len());
                in_flight.abort_all();
                return;
            }
        }
    }
}

fn report_shutdown_deadline(deadline: Duration, in_flight: usize) {
    let deadline_ms = deadline.as_millis() as u64;
    tracing::error!(deadline_ms, in_flight, "async shutdown deadline exceeded");
    let _ = writeln!(
        std::io::stderr(),
        "async shutdown deadline exceeded deadline_ms={deadline_ms} in_flight={in_flight}"
    );
}

fn in_flight() -> &'static tokio::sync::Mutex<JoinSet<()>> {
    static IN_FLIGHT: OnceLock<tokio::sync::Mutex<JoinSet<()>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| tokio::sync::Mutex::new(JoinSet::new()))
}

/// Runs `on_unwind` if this value is dropped while still armed.
///
/// A panic in a spawned task does not stop the process. Arm a guard for the
/// lifetime of a task that owns a view, and send the event that closes the
/// view from `on_unwind`. Call [`Self::disarm`] when the task returns
/// normally. The callback must not panic.
#[must_use = "disarm the guard after a normal return, or the callback runs on drop"]
pub(crate) struct UnwindGuard<F: FnOnce()> {
    on_unwind: Option<F>,
}

impl<F: FnOnce()> UnwindGuard<F> {
    /// Arms `on_unwind` until [`Self::disarm`] or drop.
    pub(crate) fn arm(on_unwind: F) -> Self {
        Self {
            on_unwind: Some(on_unwind),
        }
    }

    /// Drops the callback so a later drop does nothing.
    pub(crate) fn disarm(&mut self) {
        self.on_unwind.take();
    }
}

impl<F: FnOnce()> Drop for UnwindGuard<F> {
    fn drop(&mut self) {
        if let Some(on_unwind) = self.on_unwind.take() {
            on_unwind();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn unwind_guard_runs_callback_on_unwind() {
        let hit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&hit);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = UnwindGuard::arm(move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            panic!("task");
        }));
        assert!(result.is_err());
        assert!(hit.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn unwind_guard_skips_callback_after_disarm() {
        let hit = std::sync::atomic::AtomicBool::new(false);
        let mut guard = UnwindGuard::arm(|| hit.store(true, std::sync::atomic::Ordering::SeqCst));
        guard.disarm();
        drop(guard);
        assert!(!hit.load(std::sync::atomic::Ordering::SeqCst));
    }

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

    #[test]
    fn enter_bounds_shutdown_when_a_blocking_task_is_stuck() {
        let started = Instant::now();
        enter(async {
            let _detached = spawn_blocking(|| std::thread::sleep(Duration::from_secs(30)));
        });
        let elapsed = started.elapsed();
        assert!(
            elapsed < SHUTDOWN_DEADLINE + Duration::from_secs(2),
            "exit took {elapsed:?}"
        );
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

    #[test]
    fn enter_drives_block_on_from_the_process_runtime() {
        enter(async {
            assert_eq!(block_on(async { 7 }), 7);
        });
    }
}

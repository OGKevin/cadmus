//! Process Tokio runtime and the sync-to-async blocking facade.
//!
//! Process entry builds one multi-thread runtime. The reactor is capped.
//! The blocking pool uses Tokio's ceiling and creates a thread only when
//! work is waiting. Production callers `.await` so no worker parks (the
//! process runtime has just two).
//!
//! There is no process-global [`Handle`] and no process-global job registry.
//! Callers must already be on the runtime ([`enter`] or `#[tokio::test]`), so
//! work that cannot be async belongs on [`spawn_blocking`].
//!
//! Work that outlives a call is started with [`Job`] and owned by whoever asked
//! for it. Lifetime is the owner's `Drop`, so there is nothing for the process
//! to sweep at exit: owners stop their own work during teardown, and
//! [`enter`] bounds the blocking pool, which Tokio cannot cancel.
//!
//! # Examples
//!
//! Process entry wraps the whole app on one runtime with a bounded shutdown:
//!
//! ```no_run
//! use cadmus_core::runtime;
//!
//! fn main() {
//!     runtime::enter(async {
//!         // application async entry
//!     });
//! }
//! ```
//!
//! Background work that must outlive a call is owned as a [`Job`]:
//!
//! ```no_run
//! use cadmus_core::runtime::Job;
//! use tokio_util::sync::CancellationToken;
//!
//! async fn example() {
//!     let job = Job::spawn(|cancel: CancellationToken| async move {
//!         tokio::select! {
//!             () = cancel.cancelled() => {}
//!             () = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
//!         }
//!     });
//!     job.cancel();
//!     let _ = job.join(std::time::Duration::from_secs(1)).await;
//! }
//! ```

use std::future::Future;
use std::time::Duration;

use tokio::runtime::{Builder, Handle};
use tokio_util::sync::CancellationToken;

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

/// OS name of every thread the process runtime spawns.
///
/// Tokio applies one name to reactor workers and blocking-pool threads.
/// Linux `comm` keeps 15 bytes; this stays under that so process lists show
/// the full name, distinct from the main thread.
const THREAD_NAME: &str = "cadmus-rt";

/// How long shutdown waits for in-flight async work before aborting it.
///
/// Tokio's blocking pool cannot cancel a stuck `recv`/`sleep` in a
/// `spawn_blocking` closure. This ceiling keeps process exit finite after
/// those loops are asked to stop (closed pipes, stop flags). Shrink only after
/// every long-lived blocking task exits promptly on shutdown.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

/// Multi-thread builder with both thread pools capped.
///
/// Reactor workers and blocking-pool threads are named `cadmus-rt`.
pub fn builder() -> Builder {
    let mut builder = Builder::new_multi_thread();
    builder
        .worker_threads(WORKER_THREADS)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_name(THREAD_NAME)
        .enable_all();
    builder
}

/// Drives `future` on the process runtime.
///
/// Process entry uses this builder instead of `#[tokio::main]` so exit can
/// apply [`SHUTDOWN_DEADLINE`] after the future returns. The blocking-pool
/// ceiling matches Tokio's default ([`MAX_BLOCKING_THREADS`]).
///
/// After `future` finishes — or panics — blocking tasks have
/// [`SHUTDOWN_DEADLINE`] to return. A task blocked in a read would otherwise
/// hold process exit open. The deadline applies on the panic path too, so a
/// crash cannot leave the process hanging on an unkillable reader.
pub fn enter<F, T>(future: F) -> T
where
    F: Future<Output = T>,
{
    let runtime = builder().build().expect("failed to build process runtime");
    let guard = ShutdownGuard {
        runtime: Some(runtime),
    };
    let result = guard.runtime.as_ref().unwrap().block_on(future);
    drop(guard);
    result
}

/// Applies [`SHUTDOWN_DEADLINE`] on drop, including while unwinding.
///
/// [`enter`] wraps the app future in this guard instead of catching unwind on
/// the app output, which would require `T: UnwindSafe`.
struct ShutdownGuard {
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(SHUTDOWN_DEADLINE);
        }
    }
}

/// Runs `f` on the blocking pool of the current runtime.
#[track_caller]
pub fn spawn_blocking<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    current_handle().spawn_blocking(f)
}

/// A cancellation observed by [`race_cancel`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct Cancelled;

/// Runs `work` unless `cancel` trips first, dropping `work` when it does.
///
/// For I/O that must stay cancellable but has no cooperative checkpoint of its
/// own — an HTTP request, a subprocess round trip. Gives up the borrow of `work`
/// rather than waiting for it, so the abandoned request stops occupying a
/// connection. `biased` makes cancellation win when both are ready in the same
/// poll, so a cancel that lands as the result arrives is not lost.
pub(crate) async fn race_cancel<T, E>(
    cancel: &tokio_util::sync::CancellationToken,
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, E>
where
    E: From<Cancelled>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(E::from(Cancelled)),
        result = work => result,
    }
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

/// A background job with a known owner.
///
/// This is the only sanctioned way to start work that outlives the current
/// call: the owner stores the [`Job`], so the work cannot outlive the thing that
/// wants it. Lifetime is the owner's `Drop`.
///
/// Dropping a [`Job`] **cancels** and detaches; it never blocks, because an
/// owner can be dropped while unwinding. Use [`Job::join`] to await completion
/// within a deadline, which is only safe where awaiting is fine.
pub struct Job<T = ()> {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<T>,
}

/// How a [`Job::join`] ended.
#[derive(Debug)]
pub enum JobOutcome<T> {
    /// The work returned before the deadline.
    Finished(T),
    /// The deadline elapsed, so the job was aborted.
    Aborted,
    /// The task panicked.
    Panicked,
}

impl<T: Send + 'static> Job<T> {
    /// Spawns `work` on the process runtime, handing it the token that observes
    /// its own cancellation.
    ///
    /// `work` is expected to check the token at the checkpoints it already has.
    #[track_caller]
    pub fn spawn<F, Fut>(work: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        Self::with_token(CancellationToken::new(), work)
    }

    /// Spawns `work` on an existing cancellation token (for example
    /// [`crate::http::CancelFlag::cancellation_token`]).
    #[track_caller]
    pub fn with_token<F, Fut>(cancel: CancellationToken, work: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let token = cancel.clone();
        let handle = current_handle().spawn(async move { work(token).await });
        Self { cancel, handle }
    }

    /// Asks the job to stop, without waiting.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Returns whether the job has stopped on its own.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Waits up to `deadline` for the job to finish, then aborts it.
    pub async fn join(mut self, deadline: Duration) -> JobOutcome<T> {
        match tokio::time::timeout(deadline, &mut self.handle).await {
            Ok(Ok(value)) => JobOutcome::Finished(value),
            Ok(Err(_)) => {
                tracing::error!("background job panicked");
                JobOutcome::Panicked
            }
            Err(_) => {
                tracing::warn!(
                    deadline_ms = deadline.as_millis(),
                    "background job did not stop in time; aborting"
                );
                self.cancel.cancel();
                self.handle.abort();
                let _ = (&mut self.handle).await;
                JobOutcome::Aborted
            }
        }
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Cancels and joins every job, sharing one deadline across all of them.
///
/// Every job is cancelled *before* the first join, so no producer keeps running
/// while an earlier job consumes the shared budget. Jobs that outlive the
/// deadline are aborted.
///
/// Returns `true` when not every job finished within the budget — a timed-out,
/// aborted, or panicked job.
pub async fn finish_within_deadline(jobs: Vec<Job>, deadline: Duration) -> bool {
    for job in &jobs {
        job.cancel();
    }
    let deadline_at = tokio::time::Instant::now() + deadline;
    let mut exceeded = false;
    for job in jobs {
        let remaining = deadline_at.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            exceeded = true;
            let _ = job.join(Duration::from_millis(1)).await;
        } else {
            exceeded |= !matches!(job.join(remaining).await, JobOutcome::Finished(()));
        }
    }
    if exceeded {
        tracing::error!(
            deadline_ms = deadline.as_millis() as u64,
            "async shutdown did not complete cleanly"
        );
    }
    exceeded
}

/// Runs `on_unwind` if this value is dropped while still armed **and** the
/// thread is unwinding from a panic.
///
/// A panic in a spawned task does not stop the process. Arm a guard for the
/// lifetime of a task that owns a view, and send the event that closes the
/// view from `on_unwind`. Call [`Self::disarm`] when the task returns
/// normally or when failure paths already sent the same cleanup explicitly.
/// Task abort or cancel must not rely on this guard. The callback must not
/// panic.
#[must_use = "disarm the guard after a normal return, or the callback runs on panic unwind"]
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
        if std::thread::panicking()
            && let Some(on_unwind) = self.on_unwind.take()
        {
            on_unwind();
        }
    }
}

/// Runs `on_drop` when dropped unless [`Self::disarm`]ed.
///
/// [`UnwindGuard`] runs only while panicking. This also runs on a normal drop.
/// The callback must not panic.
#[must_use = "disarm the guard to skip the callback, or it runs on drop"]
pub(crate) struct DropGuard<F: FnOnce()> {
    on_drop: Option<F>,
}

impl<F: FnOnce()> DropGuard<F> {
    /// Arms `on_drop` until [`Self::disarm`] or drop.
    pub(crate) fn arm(on_drop: F) -> Self {
        Self {
            on_drop: Some(on_drop),
        }
    }

    /// Drops the callback so a later drop does nothing.
    pub(crate) fn disarm(&mut self) {
        self.on_drop.take();
    }
}

impl<F: FnOnce()> Drop for DropGuard<F> {
    fn drop(&mut self) {
        if let Some(on_drop) = self.on_drop.take() {
            on_drop();
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

    #[test]
    fn unwind_guard_skips_callback_on_normal_drop() {
        let hit = std::sync::atomic::AtomicBool::new(false);
        drop(UnwindGuard::arm(|| {
            hit.store(true, std::sync::atomic::Ordering::SeqCst)
        }));
        assert!(!hit.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn drop_guard_runs_callback_on_normal_drop() {
        let hit = std::sync::atomic::AtomicBool::new(false);
        drop(DropGuard::arm(|| {
            hit.store(true, std::sync::atomic::Ordering::SeqCst)
        }));
        assert!(hit.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn drop_guard_skips_callback_after_disarm() {
        let hit = std::sync::atomic::AtomicBool::new(false);
        let mut guard = DropGuard::arm(|| hit.store(true, std::sync::atomic::Ordering::SeqCst));
        guard.disarm();
        drop(guard);
        assert!(!hit.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finish_within_deadline_aborts_work_that_ignores_cancellation() {
        let job = Job::spawn(|_| std::future::pending::<()>());
        let started = Instant::now();
        let exceeded = finish_within_deadline(vec![job], Duration::from_millis(50)).await;
        assert!(exceeded);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    /// Every job must be cancelled before the first join. A job earlier in the
    /// list that ignores its token must not delay cancellation of the rest.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finish_within_deadline_cancels_every_job_before_joining_any() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let saw_cancel = Arc::new(AtomicBool::new(false));
        let stubborn = Job::spawn(|_| std::future::pending::<()>());
        let flag = Arc::clone(&saw_cancel);
        let cooperative = Job::spawn(move |token| async move {
            token.cancelled().await;
            flag.store(true, Ordering::SeqCst);
        });

        let _ =
            finish_within_deadline(vec![stubborn, cooperative], Duration::from_millis(200)).await;

        assert!(
            saw_cancel.load(Ordering::SeqCst),
            "a job behind a stubborn one must already be cancelled"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_a_job_cancels_its_work() {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = std::sync::Arc::clone(&cancel);
        let job = Job::spawn(move |token| async move {
            token.cancelled().await;
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        drop(job);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(cancel.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn joining_a_finished_job_returns_its_output() {
        let job = Job::spawn(|_| async { 7_u8 });
        let started = Instant::now();
        assert!(matches!(
            job.join(Duration::from_secs(5)).await,
            JobOutcome::Finished(7)
        ));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn process_runtime_names_worker_and_blocking_threads() {
        let runtime = builder().build().expect("process runtime");
        let worker = runtime.spawn(async { std::thread::current().name().map(str::to_owned) });
        let blocking = runtime.spawn_blocking(|| std::thread::current().name().map(str::to_owned));
        runtime.block_on(async {
            assert_eq!(
                worker.await.expect("worker task").as_deref(),
                Some(THREAD_NAME)
            );
            assert_eq!(
                blocking.await.expect("blocking task").as_deref(),
                Some(THREAD_NAME)
            );
        });
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

    #[test]
    #[should_panic(expected = "cadmus runtime handle required")]
    fn current_handle_panics_without_a_runtime() {
        let _ = current_handle();
    }
}

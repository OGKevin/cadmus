mod app;

use cadmus_core::anyhow::Error;

#[cfg(feature = "profiling")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Enable jemalloc heap profiling when built with the profiling feature.
// prof_active:true starts profiling active so Pyroscope can collect samples immediately.
#[cfg(feature = "profiling")]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

/// Starts Cadmus on one multi-thread Tokio runtime.
///
/// Blocking jobs share Tokio's default pool ceiling
/// ([`cadmus_core::runtime::MAX_BLOCKING_THREADS`]); idle threads exit on
/// their own. [`cadmus_core::runtime::WORKER_THREADS`] caps only the async
/// reactor. Entry goes through [`cadmus_core::runtime::enter`] so process
/// exit can apply [`cadmus_core::runtime::SHUTDOWN_DEADLINE`].
fn main() -> Result<(), Error> {
    cadmus_core::runtime::enter(async_main())
}

/// Runs the UI until exit, then flushes logging.
///
/// Background jobs are stopped by their owners during `app::run`, each within
/// its own deadline, and [`cadmus_core::runtime::enter`] bounds the blocking
/// pool with [`cadmus_core::runtime::SHUTDOWN_DEADLINE`] once this returns.
/// [`cadmus_core::logging::shutdown_logging`] runs last so logs emitted during
/// shutdown are not cut off. Its tracing guard flush runs on a std thread and
/// does not require Tokio; kernel log capture is a cancellable task.
async fn async_main() -> Result<(), Error> {
    let result = app::run().await;
    cadmus_core::logging::shutdown_logging().await;
    result
}

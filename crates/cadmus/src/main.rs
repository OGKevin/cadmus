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
/// Blocking jobs share one pool. It grows on demand up to
/// [`cadmus_core::runtime::MAX_BLOCKING_THREADS`] and idle threads exit on
/// their own. Past that ceiling, further blocking jobs wait.
/// [`cadmus_core::runtime::WORKER_THREADS`] caps only the async reactor.
///
/// `#[tokio::main]` can set the reactor size and cannot set this pool, so
/// entry goes through [`cadmus_core::runtime::enter`].
fn main() -> Result<(), Error> {
    cadmus_core::runtime::enter(async_main())
}

async fn async_main() -> Result<(), Error> {
    let result = app::run().await;
    cadmus_core::runtime::shutdown().await;
    result
}

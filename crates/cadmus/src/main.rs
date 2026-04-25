mod app;

use crate::app::run;
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

fn main() -> Result<(), Error> {
    run()
}

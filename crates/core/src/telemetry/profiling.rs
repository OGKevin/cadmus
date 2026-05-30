//! Pyroscope continuous profiling integration.
//!
//! This module initializes two Pyroscope profiling agents:
//!
//! - **Heap agent** — samples heap allocations via jemalloc, sending profiles
//!   tagged `cadmus.heap`.
//! - **CPU agent** — samples CPU call stacks via pprof-rs, sending profiles
//!   tagged `cadmus.cpu`.
//!
//! Both are only compiled when the `profiling` feature is enabled.
//!
//! # Prerequisites
//!
//! The binary crate must:
//! 1. Set `tikv_jemallocator::Jemalloc` as the global allocator.
//! 2. Export `malloc_conf` to enable jemalloc profiling at startup.
//!
//! # Configuration
//!
//! The Pyroscope server URL is resolved in this order:
//! 1. `PYROSCOPE_SERVER_URL` environment variable
//! 2. `settings.logging.pyroscope_endpoint` from `Settings.toml`
//!
//! If neither is set, profiling is disabled silently.
//!
//! # Example
//!
//! ```no_run
//! // Initialize profiling early in main(), before any significant allocation.
//! cadmus_core::telemetry::profiling::init_profiling(None)?;
//!
//! // ... application runs ...
//!
//! cadmus_core::telemetry::profiling::shutdown_profiling();
//! # Ok::<(), anyhow::Error>(())
//! ```

use anyhow::{Context, Error};
use pyroscope::backend::backend::BackendConfig;
use pyroscope::backend::jemalloc::jemalloc_backend;
use pyroscope::backend::pprof::{PprofConfig, pprof_backend};
use pyroscope::pyroscope::{PyroscopeAgent, PyroscopeAgentBuilder, PyroscopeAgentRunning};
use std::sync::Mutex;
use std::time::Duration;

struct ProfilingAgents {
    heap: PyroscopeAgent<PyroscopeAgentRunning>,
    cpu: PyroscopeAgent<PyroscopeAgentRunning>,
}

static AGENTS: Mutex<Option<ProfilingAgents>> = Mutex::new(None);

fn resolve_endpoint(settings_endpoint: Option<&str>) -> Option<String> {
    if let Ok(url) = std::env::var("PYROSCOPE_SERVER_URL") {
        return Some(url);
    }
    settings_endpoint.map(|s| s.to_string())
}

const SAMPLE_RATE: u32 = 100;

/// Initializes both the heap and CPU Pyroscope profiling agents.
///
/// The server URL is resolved from the `PYROSCOPE_SERVER_URL` environment
/// variable first, then from `settings_endpoint`. If neither is set,
/// profiling is silently skipped.
///
/// Must be called once, early in `main()`, before significant allocations
/// occur. The global jemalloc allocator must be active and `malloc_conf` must
/// export `prof:true,prof_active:true` for heap profiles to be collected.
///
/// # Errors
///
/// Returns an error if either Pyroscope agent cannot connect or start.
pub fn init_profiling(settings_endpoint: Option<&str>) -> Result<(), Error> {
    let url = match resolve_endpoint(settings_endpoint) {
        Some(url) => url,
        None => {
            tracing::debug!("No Pyroscope endpoint configured, profiling disabled");
            return Ok(());
        }
    };

    tracing::info!(url = %url, "Starting Pyroscope profiling agents (heap + CPU)");

    let version = crate::version::get_current_version().to_string();

    let heap_agent = PyroscopeAgentBuilder::new(
        url.clone(),
        "cadmus.heap",
        SAMPLE_RATE,
        "pyroscope-rs",
        &version,
        jemalloc_backend(),
    )
    .build()
    .context("failed to build Pyroscope heap agent")?;

    let cpu_agent = PyroscopeAgentBuilder::new(
        url.clone(),
        "cadmus.cpu",
        SAMPLE_RATE,
        "pyroscope-rs",
        &version,
        pprof_backend(
            PprofConfig {
                sample_rate: SAMPLE_RATE,
            },
            BackendConfig {
                report_thread_name: true,
                ..BackendConfig::default()
            },
        ),
    )
    .build()
    .context("failed to build Pyroscope CPU agent")?;

    let heap_running = heap_agent
        .start()
        .context("failed to start Pyroscope heap agent")?;
    let cpu_running = cpu_agent
        .start()
        .context("failed to start Pyroscope CPU agent")?;

    match AGENTS.lock() {
        Ok(mut guard) => {
            *guard = Some(ProfilingAgents {
                heap: heap_running,
                cpu: cpu_running,
            });
        }
        Err(_) => {
            if let Ok(stopped) = heap_running.stop() {
                stopped.shutdown();
            }
            if let Ok(stopped) = cpu_running.stop() {
                stopped.shutdown();
            }
            anyhow::bail!("profiling agents mutex is poisoned");
        }
    }

    tracing::info!(
        url = %url,
        sample_rate = SAMPLE_RATE,
        "Pyroscope profiling agents running"
    );

    Ok(())
}

/// Shuts down both Pyroscope profiling agents and flushes buffered profiles.
///
/// Safe to call even if `init_profiling` was never called or profiling was
/// disabled due to a missing endpoint.
pub fn shutdown_profiling() {
    let timeout = Duration::from_millis(1500);

    if let Ok(mut guard) = AGENTS.lock() {
        if let Some(agents) = guard.take() {
            if let Ok(stopped) = agents.heap.stop() {
                super::shutdown::shutdown_with_timeout(move || stopped.shutdown(), timeout);
            }
            if let Ok(stopped) = agents.cpu.stop() {
                super::shutdown::shutdown_with_timeout(move || stopped.shutdown(), timeout);
            }
        }
    }
}

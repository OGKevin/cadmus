//! Telemetry subsystem: OpenTelemetry tracing/logging and Pyroscope profiling.

pub(crate) mod shutdown;

#[cfg(feature = "profiling")]
pub mod profiling;

#[cfg(feature = "tracing")]
pub mod tracing;

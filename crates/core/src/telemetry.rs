//! OpenTelemetry integration for exporting logs and traces to OTLP endpoints.
//!
//! This module provides initialization and shutdown functions for OpenTelemetry telemetry
//! when the `otel` feature is enabled. It configures both trace and log exporters that
//! send data to an OTLP-compatible backend.
//!
//! # Architecture
//!
//! The telemetry system uses:
//! - **Tracer Provider**: Exports distributed traces via OTLP HTTP
//! - **Logger Provider**: Exports structured logs via OTLP HTTP  
//! - **Batch Processors**: Buffer and send data asynchronously to minimize overhead
//! - **Resource Attributes**: Attach service metadata to all telemetry data
//!
//! # Configuration
//!
//! The OTLP endpoint can be configured in two ways (environment variable takes precedence):
//! 1. `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable
//! 2. `otlp_endpoint` field in `LoggingSettings`
//!
//! # Example
//!
//! ```
//! use cadmus_core::settings::LoggingSettings;
//! use cadmus_core::telemetry;
//! use cadmus_core::logging::get_run_id;
//!
//! let mut settings = LoggingSettings::default();
//! settings.otlp_endpoint = Some("http://localhost:4318".to_string());
//!
//! // Initialize telemetry (returns layer for tracing subscriber)
//! let layer = telemetry::init_telemetry(&settings, get_run_id())?;
//!
//! // ... application runs ...
//!
//! // Flush and shutdown at exit
//! telemetry::shutdown_telemetry();
//! # Ok::<(), anyhow::Error>(())
//! ```

use crate::settings::LoggingSettings;
use anyhow::{Context, Error};
use gethostname::gethostname;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::logs::{BatchLogProcessor, LoggerProvider as SdkLoggerProvider};
use opentelemetry_sdk::trace::{
    BatchSpanProcessor, Config as TraceConfig, TracerProvider as SdkTracerProvider,
};
use opentelemetry_sdk::{runtime, Resource};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::Duration;

const GIT_VERSION: &str = env!("GIT_VERSION");
const SERVICE_NAME: &str = "cadmus";
static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();
static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

/// Initializes OpenTelemetry telemetry with trace and log exporters.
///
/// This function sets up:
/// - A tracer provider for distributed tracing
/// - A logger provider for structured log export
/// - Resource attributes identifying the service
///
/// If no OTLP endpoint is configured (via settings or environment variable),
/// this function returns `Ok(None)` and telemetry export is disabled.
///
/// # Arguments
///
/// * `settings` - Logging configuration containing optional OTLP endpoint
/// * `run_id` - Unique identifier for this application run
///
/// # Returns
///
/// Returns an optional `OpenTelemetryTracingBridge` layer that can be added to
/// the tracing subscriber. Returns `None` if OTLP export is not configured.
///
/// # Errors
///
/// Returns an error if:
/// - The OTLP exporter cannot be built
/// - The tracer or logger provider initialization fails
///
/// # Example
///
/// ```
/// use cadmus_core::settings::LoggingSettings;
/// use cadmus_core::telemetry::init_telemetry;
///
/// let settings = LoggingSettings {
///     enabled: true,
///     level: "info".to_string(),
///     max_files: 3,
///     directory: "logs".into(),
///     otlp_endpoint: Some("http://localhost:4318".to_string()),
/// };
///
/// let layer = init_telemetry(&settings, "run-123")?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn init_telemetry(
    settings: &LoggingSettings,
    run_id: &str,
) -> Result<
    Option<
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge<
            SdkLoggerProvider,
            opentelemetry_sdk::logs::Logger,
        >,
    >,
    Error,
> {
    let endpoint = match otel_endpoint(settings) {
        Some(endpoint) => endpoint,
        None => return Ok(None),
    };

    let hostname = gethostname().to_string_lossy().into_owned();

    let resource = Resource::new([
        KeyValue::new("service.name", SERVICE_NAME),
        KeyValue::new("service.version", GIT_VERSION),
        KeyValue::new("cadmus.run_id", run_id.to_string()),
        KeyValue::new("hostname", hostname),
    ]);

    let tracer_provider = build_tracer_provider(&endpoint, resource.clone())?;
    let logger_provider = build_logger_provider(&endpoint, resource)?;

    let tracer_provider = TRACER_PROVIDER.get_or_init(|| tracer_provider);
    let logger_provider = LOGGER_PROVIDER.get_or_init(|| logger_provider);

    global::set_tracer_provider(tracer_provider.clone());

    let layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(logger_provider);

    println!(
        "Initialized OpenTelemetry telemetry with endpoint {}",
        endpoint
    );

    Ok(Some(layer))
}

/// This ensures that when there are connection failures during shutdown, it doesn't block
/// forever.
fn shutdown_with_timeout(shutdown: impl FnOnce() + Send + 'static, timeout: Duration) {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        shutdown();
        let _ = tx.send(());
    });

    let _ = rx.recv_timeout(timeout);
}

/// Shuts down OpenTelemetry providers and flushes buffered telemetry.
///
/// This function should be called before application exit to ensure all
/// buffered traces and logs are exported to the OTLP endpoint. It:
/// - Shuts down the tracer provider (flushes pending traces)
/// - Shuts down the logger provider (flushes pending logs)  
/// - Cleans up the global tracer provider
///
/// After calling this function, no more telemetry will be exported.
/// This function is idempotent and safe to call multiple times.
///
/// # Example
///
/// ```
/// use cadmus_core::telemetry::shutdown_telemetry;
///
/// // At application exit
/// shutdown_telemetry();
/// ```
pub fn shutdown_telemetry() {
    let timeout = Duration::from_millis(1500);

    if let Some(provider) = TRACER_PROVIDER.get() {
        shutdown_with_timeout(
            {
                move || {
                    let _ = provider.shutdown();
                }
            },
            timeout,
        );
    }

    if let Some(provider) = LOGGER_PROVIDER.get() {
        shutdown_with_timeout(
            {
                move || {
                    let _ = provider.shutdown();
                }
            },
            timeout,
        );
    }

    global::shutdown_tracer_provider();
}

/// Determines the OTLP endpoint from settings or environment variables.
///
/// Environment variables take precedence over configuration file settings.
///
/// # Arguments
///
/// * `settings` - Logging configuration that may contain an OTLP endpoint
///
/// # Returns
///
/// Returns `Some(endpoint)` if an OTLP endpoint is configured, `None` otherwise.
fn otel_endpoint(settings: &LoggingSettings) -> Option<String> {
    if let Ok(value) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        return Some(value);
    }

    settings.otlp_endpoint.clone()
}

/// Builds a tracer provider with OTLP HTTP export.
///
/// # Arguments
///
/// * `endpoint` - Base OTLP endpoint URL
/// * `resource` - Resource attributes to attach to all traces
///
/// # Returns
///
/// Returns a configured `SdkTracerProvider` ready for use.
///
/// # Errors
///
/// Returns an error if the OTLP span exporter cannot be built.
fn build_tracer_provider(endpoint: &str, resource: Resource) -> Result<SdkTracerProvider, Error> {
    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(endpoint)
        .build_span_exporter()
        .context("can't build otlp span exporter")?;
    let processor = BatchSpanProcessor::builder(exporter, runtime::TokioCurrentThread).build();
    let config = TraceConfig::default().with_resource(resource);

    Ok(SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_config(config)
        .build())
}

/// Builds a logger provider with OTLP HTTP export.
///
/// The logger provider exports logs to `<endpoint>/v1/logs`.
///
/// # Arguments
///
/// * `endpoint` - Base OTLP endpoint URL  
/// * `resource` - Resource attributes to attach to all logs
///
/// # Returns
///
/// Returns a configured `SdkLoggerProvider` ready for use.
///
/// # Errors
///
/// Returns an error if the OTLP log exporter cannot be built.
fn build_logger_provider(endpoint: &str, resource: Resource) -> Result<SdkLoggerProvider, Error> {
    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(format!("{}/v1/logs", endpoint.trim_end_matches('/')))
        .build_log_exporter()
        .context("can't build otlp log exporter")?;
    let processor = BatchLogProcessor::builder(exporter, runtime::TokioCurrentThread).build();

    Ok(SdkLoggerProvider::builder()
        .with_log_processor(processor)
        .with_resource(resource)
        .build())
}

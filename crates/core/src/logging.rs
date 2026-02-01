//! Structured logging infrastructure with JSON output and OpenTelemetry integration.
//!
//! This module provides logging functionality for Cadmus, including:
//! - JSON-structured logs written to rotating files
//! - Configurable log levels and filtering  
//! - Automatic log file cleanup based on retention policies
//! - Optional OpenTelemetry export (when `otel` feature is enabled)
//! - Unique run ID for correlating logs across a session
//!
//! # Architecture
//!
//! The logging system is built on the `tracing` crate ecosystem:
//! - `tracing_subscriber` for composable logging layers
//! - `tracing_appender` for non-blocking file I/O
//! - JSON formatting for structured, machine-readable logs
//! - `EnvFilter` for flexible log level control
//!
//! Each application run generates a unique Run ID (UUID v7) that appears in:
//! - The log filename: `cadmus-<run_id>.json`
//! - OpenTelemetry resource attributes
//! - All log entries for correlation
//!
//! # Log File Management
//!
//! Log files are automatically managed:
//! - Files are named with the run ID: `cadmus-<run_id>.json`
//! - Older files are deleted when `max_files` limit is exceeded
//! - Cleanup happens at initialization, keeping only the most recent files
//!
//! # Configuration
//!
//! Logging is configured via `LoggingSettings`:
//!
//! ```toml
//! [logging]
//! enabled = true
//! level = "info"
//! max-files = 3
//! directory = "logs"
//! otlp-endpoint = "http://localhost:4318"  # Optional
//! ```
//!
//! The log level can be overridden with the `RUST_LOG` environment variable:
//!
//! ```bash
//! RUST_LOG=debug ./cadmus
//! RUST_LOG=cadmus::view=trace,info ./cadmus
//! ```
//!
//! # Example Usage
//!
//! ```rust
//! use cadmus_core::settings::LoggingSettings;
//! use cadmus_core::logging::{init_logging, shutdown_logging, get_run_id};
//!
//! let settings = LoggingSettings {
//!     enabled: true,
//!     level: "info".to_string(),
//!     max_files: 3,
//!     directory: "logs".into(),
//!     otlp_endpoint: None,
//! };
//!
//! // Initialize at application startup
//! init_logging(&settings)?;
//! eprintln!("Started with run ID: {}", get_run_id());
//!
//! // Use tracing macros throughout the application
//! tracing::info!("Application started");
//!
//! // Shutdown at application exit (flushes buffers)
//! shutdown_logging();
//! # Ok::<(), anyhow::Error>(())
//! ```

use crate::settings::LoggingSettings;
#[cfg(feature = "otel")]
use crate::telemetry;
use anyhow::{Context, Error};
use std::fs;
use std::fs::DirEntry;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const GIT_VERSION: &str = env!("GIT_VERSION");
const LOG_FILE_PREFIX: &str = "cadmus-";
const LOG_FILE_SUFFIX: &str = "json";

static LOG_GUARD: OnceLock<Mutex<Option<WorkerGuard>>> = OnceLock::new();
static RUN_ID: OnceLock<String> = OnceLock::new();

/// Returns the unique run ID for this application session.
///
/// The run ID is a UUID v7 generated at first access and remains constant
/// for the lifetime of the process. It is used to:
/// - Name the log file: `cadmus-<run_id>.json`
/// - Tag OpenTelemetry telemetry exports
/// - Correlate all operations within a single run
///
/// # Returns
///
/// A string slice containing the run ID, valid for the program's lifetime.
///
/// # Example
///
/// ```
/// use cadmus_core::logging::get_run_id;
///
/// let run_id = get_run_id();
/// eprintln!("Application run ID: {}", run_id);
/// assert_eq!(get_run_id(), run_id); // Consistent across calls
/// ```
pub fn get_run_id() -> &'static str {
    RUN_ID.get_or_init(|| Uuid::now_v7().to_string()).as_str()
}

/// Removes old log files to maintain the configured retention limit.
///
/// This function scans the log directory for files matching the pattern
/// `cadmus-*.json` and deletes the oldest files if the count exceeds `max_files`.
///
/// Note: this relies on the run ID being a UUID v7 (time-ordered). Filenames are
/// `cadmus-<run_id>.json` where `<run_id>` is generated with `Uuid::now_v7()`,
/// so lexicographic sorting of the filenames corresponds to chronological order.
/// Sorting by file name therefore yields oldest-first ordering for removal.
///
/// # Arguments
///
/// * `log_dir` - Path to the directory containing log files
/// * `max_files` - Maximum number of log files to retain (0 = keep all)
///
/// # Returns
///
/// Returns `Ok(())` on success.
///
/// # Errors
///
/// Returns an error if:
/// - The log directory cannot be read
/// - Individual directory entries cannot be read
/// - Old log files cannot be deleted
fn cleanup_run_logs(log_dir: &std::path::Path, max_files: usize) -> Result<(), Error> {
    if max_files == 0 {
        return Ok(());
    }

    let mut entries = collect_run_log_entries(log_dir)?;
    if entries.len() <= max_files {
        return Ok(());
    }

    entries.sort_by_key(|entry| entry.file_name());
    let remove_count = entries.len().saturating_sub(max_files);
    for entry in entries.into_iter().take(remove_count) {
        fs::remove_file(entry.path())
            .with_context(|| format!("can't remove old log file {}", entry.path().display()))?;
    }

    Ok(())
}

/// Collects all Cadmus log file entries from the specified directory.
///
/// Only files matching the pattern `cadmus-*.json` are collected.
///
/// # Arguments
///
/// * `log_dir` - Path to the directory to scan
///
/// # Returns
///
/// Returns a vector of directory entries representing log files.
///
/// # Errors
///
/// Returns an error if the directory cannot be read or entries are inaccessible.
fn collect_run_log_entries(log_dir: &std::path::Path) -> Result<Vec<DirEntry>, Error> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(log_dir)
        .with_context(|| format!("can't read log directory {}", log_dir.display()))?
    {
        let entry = entry.context("can't read log directory entry")?;
        if is_run_log_entry(&entry) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// Determines whether a directory entry is a Cadmus log file.
///
/// Returns `true` if the filename starts with `cadmus-` and ends with `.json`.
///
/// # Arguments
///
/// * `entry` - Directory entry to check
///
/// # Returns
///
/// `true` if the entry is a log file, `false` otherwise.
fn is_run_log_entry(entry: &DirEntry) -> bool {
    let file_name = entry.file_name();
    let file_name = file_name.to_string_lossy();
    if !file_name.starts_with(LOG_FILE_PREFIX) {
        return false;
    }

    file_name.ends_with(LOG_FILE_SUFFIX)
}

/// Initializes the logging system with JSON output and optional OpenTelemetry export.
///
/// This function sets up the complete logging infrastructure:
/// - Creates the log directory if it doesn't exist
/// - Cleans up old log files based on retention policy
/// - Configures a rolling file appender with non-blocking I/O
/// - Applies log level filtering from settings or environment
/// - Sets up JSON formatting for structured logs
/// - Initializes OpenTelemetry export if the `otel` feature is enabled
///
/// The function should only be called once at application startup.
/// The logging system remains active until `shutdown_logging()` is called.
///
/// # Arguments
///
/// * `settings` - Logging configuration including level, directory, and retention
///
/// # Returns
///
/// Returns `Ok(())` on successful initialization.
///
/// # Errors
///
/// Returns an error if:
/// - The current working directory cannot be determined
/// - The log directory cannot be created
/// - Log file cleanup fails
/// - The rolling file appender cannot be initialized
/// - The log filter configuration is invalid
/// - The tracing subscriber cannot be initialized
/// - OpenTelemetry initialization fails (when `otel` feature is enabled)
///
/// # Example
///
/// ```
/// use cadmus_core::settings::LoggingSettings;
/// use cadmus_core::logging::init_logging;
///
/// let settings = LoggingSettings {
///     enabled: true,
///     level: "debug".to_string(),
///     max_files: 5,
///     directory: "logs".into(),
///     otlp_endpoint: Some("http://localhost:4318".to_string()),
/// };
///
/// init_logging(&settings)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn init_logging(settings: &LoggingSettings) -> Result<(), Error> {
    if !settings.enabled {
        return Ok(());
    }

    let current_working_dir =
        std::env::current_dir().context("can't get current working directory")?;
    let log_dir = current_working_dir.join(&settings.directory);
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("can't create log directory {}", &log_dir.display()))?;

    cleanup_run_logs(&log_dir, settings.max_files)?;

    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::NEVER)
        .filename_prefix(format!("{}{}", LOG_FILE_PREFIX, get_run_id()))
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(settings.max_files)
        .build(&log_dir)
        .context("can't initialize rolling log file appender")?;

    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    let _ = LOG_GUARD.set(Mutex::new(Some(guard)));

    let filter = build_filter(settings)?;

    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(non_blocking)
        .with_current_span(true);

    let subscriber = tracing_subscriber::registry().with(filter).with(fmt_layer);

    #[cfg(feature = "otel")]
    let subscriber = subscriber.with(telemetry::init_telemetry(settings, get_run_id())?);

    subscriber
        .try_init()
        .context("can't initialize tracing subscriber")?;

    eprintln!(
        "Cadmus run started with ID: {} (version {})",
        get_run_id(),
        GIT_VERSION
    );

    Ok(())
}

/// Gracefully shuts down the logging system and flushes buffered data.
///
/// This function ensures all buffered log data is written to disk and, if enabled,
/// exported to OpenTelemetry endpoints before the application exits. It:
/// - Flushes the file appender buffer (happens automatically via `LOG_GUARD` drop)
/// - Shuts down OpenTelemetry providers (when `otel` feature is enabled)
/// - Ensures no log data is lost on exit
///
/// This function should be called once at application shutdown.
///
/// # Example
///
/// ```no_run
/// use cadmus_core::logging::{init_logging, shutdown_logging};
/// use cadmus_core::settings::LoggingSettings;
///
/// // At application start
/// let settings = LoggingSettings::default();
/// init_logging(&settings)?;
///
/// // ... application runs ...
///
/// // At application exit
/// shutdown_logging();
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn shutdown_logging() {
    if let Some(mutex) = LOG_GUARD.get() {
        if let Ok(mut guard_opt) = mutex.lock() {
            if let Some(guard) = guard_opt.take() {
                let (tx, rx) = mpsc::channel();

                thread::spawn(move || {
                    drop(guard);
                    let _ = tx.send(());
                });

                let _ = rx.recv_timeout(Duration::from_secs(5));
                eprintln!("Logging shutdown complete.");
            }
        }
    }

    #[cfg(feature = "otel")]
    telemetry::shutdown_telemetry();
}

/// Builds an `EnvFilter` from settings or environment variables.
///
/// The function checks for the `RUST_LOG` environment variable first, which
/// overrides the `level` setting. If `RUST_LOG` is not set, it uses the
/// level from `LoggingSettings` (defaulting to "info" if empty).
///
/// # Arguments
///
/// * `settings` - Logging settings containing the default level
///
/// # Returns
///
/// Returns a configured `EnvFilter` instance.
///
/// # Errors
///
/// Returns an error if the log level string cannot be parsed.
///
/// # Example Filter Syntax
///
/// ```bash
/// # Global level
/// RUST_LOG=debug
///
/// # Per-module levels
/// RUST_LOG=cadmus::view=trace,info
///
/// # Complex filtering
/// RUST_LOG=warn,cadmus::document=debug,cadmus::sync=trace
/// ```
fn build_filter(settings: &LoggingSettings) -> Result<EnvFilter, Error> {
    if let Ok(filter) = EnvFilter::try_from_default_env() {
        return Ok(filter);
    }

    let level = settings.level.trim();
    let level = if level.is_empty() { "info" } else { level };

    EnvFilter::builder()
        .parse(level)
        .context("invalid logging level")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_log_file(dir: &std::path::Path, index: usize) -> Result<(), Error> {
        let file_name = format!("{}{:04}.{}", LOG_FILE_PREFIX, index, LOG_FILE_SUFFIX);
        fs::write(dir.join(file_name), b"{}")?;
        Ok(())
    }

    fn collect_log_file_names(dir: &std::path::Path) -> Result<Vec<String>, Error> {
        let mut entries = collect_run_log_entries(dir)?;
        entries.sort_by_key(|entry| entry.file_name());
        Ok(entries
            .into_iter()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect())
    }

    #[test]
    fn test_cleanup_run_logs_removes_oldest_entries() -> Result<(), Error> {
        let temp_dir = TempDir::new()?;
        for index in 1..=5 {
            create_log_file(temp_dir.path(), index)?;
        }

        cleanup_run_logs(temp_dir.path(), 3)?;

        let remaining = collect_log_file_names(temp_dir.path())?;
        assert_eq!(remaining.len(), 3);
        assert_eq!(
            remaining,
            vec![
                format!("{}0003.{}", LOG_FILE_PREFIX, LOG_FILE_SUFFIX),
                format!("{}0004.{}", LOG_FILE_PREFIX, LOG_FILE_SUFFIX),
                format!("{}0005.{}", LOG_FILE_PREFIX, LOG_FILE_SUFFIX),
            ]
        );

        Ok(())
    }

    #[test]
    fn test_cleanup_run_logs_max_files_zero_keeps_all() -> Result<(), Error> {
        let temp_dir = TempDir::new()?;
        for index in 1..=3 {
            create_log_file(temp_dir.path(), index)?;
        }

        cleanup_run_logs(temp_dir.path(), 0)?;

        let remaining = collect_log_file_names(temp_dir.path())?;
        assert_eq!(remaining.len(), 3);

        Ok(())
    }
}

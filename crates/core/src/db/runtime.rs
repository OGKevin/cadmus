use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

/// Global lazy-initialized Tokio runtime for database operations.
pub static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tracing::info!("initializing global Tokio runtime for database operations");
    Runtime::new().expect("failed to create Tokio runtime for database operations")
});

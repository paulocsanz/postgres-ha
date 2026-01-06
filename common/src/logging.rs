//! Structured logging initialization
//!
//! Provides consistent logging initialization across all postgres-ha components.

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Guard that keeps the tracing subscriber active.
/// Drop this at the end of main to flush logs.
pub struct LogGuard;

/// Initialize structured logging for a component.
///
/// Returns a guard that should be held for the lifetime of the program.
///
/// # Example
/// ```ignore
/// let _guard = init_logging("patroni-runner");
/// info!("Starting up...");
/// ```
pub fn init_logging(_component: &str) -> LogGuard {
    let filter = EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into());

    let format = fmt::layer().with_target(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(format)
        .init();

    LogGuard
}

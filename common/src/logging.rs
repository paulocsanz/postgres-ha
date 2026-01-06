//! Structured logging with operation IDs and timing
//!
//! Provides consistent logging initialization across all postgres-ha components.

use tracing::Span;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use uuid::Uuid;

/// Initialize structured logging for a component.
///
/// Returns a root span with an operation ID that can be used to correlate logs.
///
/// # Example
/// ```ignore
/// let _guard = init_logging("patroni-runner");
/// info!("Starting up...");
/// ```
pub fn init_logging(component: &str) -> Span {
    let operation_id = Uuid::new_v4().to_string()[..8].to_string();

    let filter = EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into());

    let format = fmt::layer().with_target(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(format)
        .init();

    tracing::info_span!("op", id = %operation_id, component = %component)
}

/// Create a timed operation span for measuring duration.
///
/// The span records the start time and can be used with tracing's timing features.
///
/// # Example
/// ```ignore
/// let _span = timed_span!("bootstrap");
/// // ... do work ...
/// // Duration is logged when span is dropped
/// ```
#[macro_export]
macro_rules! timed_span {
    ($name:expr) => {
        tracing::info_span!($name, start_ms = %chrono::Utc::now().timestamp_millis())
    };
    ($name:expr, $($field:tt)*) => {
        tracing::info_span!($name, start_ms = %chrono::Utc::now().timestamp_millis(), $($field)*)
    };
}

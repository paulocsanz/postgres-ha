//! Shared utilities for postgres-patroni binaries
//!
//! This crate provides PostgreSQL-specific utilities for:
//! - Volume and data directory path resolution
//! - SSL certificate management
//! - Patroni runner components
//! - Common helper functions

pub mod bootstrap;
mod paths;
pub mod patroni;
mod ssl;

// Re-export path utilities
pub use paths::{pgdata, ssl_dir, volume_root, EXPECTED_VOLUME_MOUNT_PATH};

// Re-export SSL utilities
pub use ssl::{cert_expires_within, is_valid_x509v3_cert};

// Re-export common utilities
pub use common::{ConfigExt, RailwayEnv, Telemetry, TelemetryEvent};

use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::process::Command;

/// Check if Patroni mode is enabled
pub fn is_patroni_enabled() -> bool {
    bool::env_parse("PATRONI_ENABLED", false)
}

/// Run a command with sudo
pub async fn sudo_command(args: &[&str]) -> Result<()> {
    let status = Command::new("sudo")
        .args(args)
        .stdin(Stdio::null())
        .status()
        .await
        .context("Failed to run sudo command")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("sudo command failed with status: {}", status);
    }
}

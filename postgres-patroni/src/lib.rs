//! Shared utilities for postgres-patroni binaries
//!
//! This module provides PostgreSQL-specific utilities for volume paths
//! and SSL certificate management.

use anyhow::{Context, Result};
use std::env;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub use common::{ConfigExt, RailwayEnv, Telemetry, TelemetryEvent};

pub const EXPECTED_VOLUME_MOUNT_PATH: &str = "/var/lib/postgresql/data";

/// Get the volume root path from environment or default
pub fn volume_root() -> String {
    String::env_or("RAILWAY_VOLUME_MOUNT_PATH", EXPECTED_VOLUME_MOUNT_PATH)
}

/// Get the SSL directory path
pub fn ssl_dir() -> String {
    format!("{}/certs", volume_root())
}

/// Get the PGDATA path
pub fn pgdata() -> String {
    env::var("PGDATA").unwrap_or_else(|_| format!("{}/pgdata", volume_root()))
}

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

/// Check if a certificate is valid x509v3 with DNS:localhost
pub async fn is_valid_x509v3_cert(cert_path: &str) -> bool {
    if !Path::new(cert_path).exists() {
        return false;
    }

    let result = Command::new("openssl")
        .args(["x509", "-noout", "-text", "-in", cert_path])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            text.contains("DNS:localhost")
        }
        _ => false,
    }
}

/// Check if a certificate will expire within the given seconds
pub async fn cert_expires_within(cert_path: &str, seconds: u64) -> bool {
    if !Path::new(cert_path).exists() {
        return true;
    }

    let result = Command::new("openssl")
        .args([
            "x509",
            "-checkend",
            &seconds.to_string(),
            "-noout",
            "-in",
            cert_path,
        ])
        .output()
        .await;

    match result {
        Ok(output) => !output.status.success(),
        Err(_) => true,
    }
}

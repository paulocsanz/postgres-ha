//! Shared utilities for postgres-patroni binaries
//!
//! This module provides PostgreSQL-specific utilities for volume paths,
//! SSL certificate management, and YAML parsing.

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

/// Parse a simple YAML value from a line like "key: value"
pub fn parse_yaml_value(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }

    let value = parts[1].trim();
    let value = value
        .trim_start_matches('"')
        .trim_end_matches('"')
        .trim_start_matches('\'')
        .trim_end_matches('\'');

    Some(value.to_string())
}

/// Extract a value from a YAML file given a section and key
pub fn extract_yaml_value(content: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let mut section_indent = 0;

    for line in content.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if trimmed.starts_with(&format!("{}:", section)) {
            in_section = true;
            section_indent = indent;
            continue;
        }

        if in_section {
            if !trimmed.is_empty() && indent <= section_indent && !trimmed.starts_with('#') {
                in_section = false;
                continue;
            }

            if trimmed.starts_with(&format!("{}:", key)) {
                return parse_yaml_value(trimmed);
            }
        }
    }

    None
}

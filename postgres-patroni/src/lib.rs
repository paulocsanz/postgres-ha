//! Shared utilities for postgres-patroni binaries

use std::env;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use anyhow::{Context, Result};

pub const EXPECTED_VOLUME_MOUNT_PATH: &str = "/var/lib/postgresql/data";

/// Get the volume root path from environment or default
pub fn volume_root() -> String {
    env::var("RAILWAY_VOLUME_MOUNT_PATH").unwrap_or_else(|_| EXPECTED_VOLUME_MOUNT_PATH.to_string())
}

/// Get the SSL directory path
pub fn ssl_dir() -> String {
    format!("{}/certs", volume_root())
}

/// Get the PGDATA path
pub fn pgdata() -> String {
    env::var("PGDATA").unwrap_or_else(|_| format!("{}/pgdata", volume_root()))
}

/// Check if running on Railway
pub fn is_railway() -> bool {
    env::var("RAILWAY_ENVIRONMENT").is_ok()
}

/// Check if Patroni mode is enabled
pub fn is_patroni_enabled() -> bool {
    env::var("PATRONI_ENABLED")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false)
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

/// Run openssl command
pub async fn openssl(args: &[&str]) -> Result<String> {
    let output = Command::new("openssl")
        .args(args)
        .output()
        .await
        .context("Failed to run openssl")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        anyhow::bail!(
            "openssl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
        return true; // Treat missing cert as expiring
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
        Ok(output) => !output.status.success(), // Exit 1 means will expire
        Err(_) => true,
    }
}

/// Parse a simple YAML value from a line like "key: value" or "key: 'value'" or 'key: "value"'
pub fn parse_yaml_value(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }

    let value = parts[1].trim();
    // Strip quotes if present
    let value = value
        .trim_start_matches('"')
        .trim_end_matches('"')
        .trim_start_matches('\'')
        .trim_end_matches('\'');

    Some(value.to_string())
}

/// Extract a value from a YAML file given a section and key
/// Simple parser that looks for patterns like:
///   section:
///     key: value
pub fn extract_yaml_value(content: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let mut section_indent = 0;

    for line in content.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        // Check if we found the section
        if trimmed.starts_with(&format!("{}:", section)) {
            in_section = true;
            section_indent = indent;
            continue;
        }

        // If we're in the section and at correct indent level
        if in_section {
            // If we hit a line at same or less indent (except empty lines), we're out of section
            if !trimmed.is_empty() && indent <= section_indent && !trimmed.starts_with('#') {
                in_section = false;
                continue;
            }

            // Look for our key
            if trimmed.starts_with(&format!("{}:", key)) {
                return parse_yaml_value(trimmed);
            }
        }
    }

    None
}

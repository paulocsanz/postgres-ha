//! SSL certificate utilities
//!
//! Functions for validating SSL certificates and checking expiration.

use std::path::Path;
use tokio::process::Command;

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

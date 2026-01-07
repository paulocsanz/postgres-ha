//! SSL certificate utilities
//!
//! Functions for validating SSL certificates and checking expiration.
//! Uses the openssl crate for direct certificate parsing without subprocess calls.

use anyhow::{Context, Result};
use openssl::pkey::PKey;
use openssl::x509::{X509StoreContext, X509};
use std::fs;
use std::path::Path;

/// Validate SSL certificate setup in the given directory.
///
/// Checks that:
/// 1. server.crt, server.key, and root.crt all exist and are parseable
/// 2. The server certificate's public key matches the private key
/// 3. The server certificate is signed by the root CA
pub fn is_valid_x509v3_cert(cert_path: &str) -> Result<bool> {
    // Derive ssl_dir from cert_path (expects path like "{ssl_dir}/server.crt")
    let ssl_dir = Path::new(cert_path)
        .parent()
        .context("Invalid cert path: no parent directory")?;

    let server_crt_path = ssl_dir.join("server.crt");
    let server_key_path = ssl_dir.join("server.key");
    let root_crt_path = ssl_dir.join("root.crt");

    // Check all required files exist
    if !server_crt_path.exists() || !server_key_path.exists() || !root_crt_path.exists() {
        return Ok(false);
    }

    // Parse server certificate
    let server_crt_pem =
        fs::read(&server_crt_path).context("Failed to read server certificate")?;
    let server_cert =
        X509::from_pem(&server_crt_pem).context("Failed to parse server certificate as PEM")?;

    // Parse server private key
    let server_key_pem = fs::read(&server_key_path).context("Failed to read server private key")?;
    let server_key =
        PKey::private_key_from_pem(&server_key_pem).context("Failed to parse server private key")?;

    // Parse root CA certificate
    let root_crt_pem = fs::read(&root_crt_path).context("Failed to read root CA certificate")?;
    let root_cert =
        X509::from_pem(&root_crt_pem).context("Failed to parse root CA certificate as PEM")?;

    // Verify cert/key pair match by comparing public keys
    let cert_pubkey = server_cert
        .public_key()
        .context("Failed to extract public key from certificate")?;
    if !cert_pubkey.public_eq(&server_key) {
        return Ok(false);
    }

    // Verify server cert is signed by root CA
    let mut store_builder =
        openssl::x509::store::X509StoreBuilder::new().context("Failed to create X509 store")?;
    store_builder
        .add_cert(root_cert)
        .context("Failed to add root CA to store")?;
    let store = store_builder.build();

    let mut store_ctx =
        X509StoreContext::new().context("Failed to create X509 store context")?;

    let chain = openssl::stack::Stack::new().context("Failed to create certificate chain")?;

    let is_valid = store_ctx
        .init(&store, &server_cert, &chain, |ctx| ctx.verify_cert())
        .context("Failed to verify certificate chain")?;

    Ok(is_valid)
}

/// Check if a certificate will expire within the given seconds
pub fn cert_expires_within(cert_path: &str, seconds: u64) -> Result<bool> {
    if !Path::new(cert_path).exists() {
        return Ok(true); // Treat missing cert as "needs renewal"
    }

    let pem_data = fs::read(cert_path).context("Failed to read certificate file")?;

    let cert = X509::from_pem(&pem_data).context("Failed to parse certificate as PEM")?;

    let not_after = cert.not_after();

    // openssl::asn1::Asn1TimeRef::diff returns the difference in days and seconds
    let now = openssl::asn1::Asn1Time::days_from_now(0).context("Failed to get current time")?;

    // Check if certificate expires within the threshold
    let diff = not_after.diff(&now).context("Failed to compute time difference")?;

    // Convert diff to total seconds (diff.days * 86400 + diff.secs)
    let total_seconds = (diff.days as i64 * 86400) + diff.secs as i64;

    // If total_seconds is negative, cert is already expired
    // If total_seconds < threshold, cert expires soon
    Ok(total_seconds < seconds as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_cert_is_invalid() {
        assert!(!is_valid_x509v3_cert("/nonexistent/cert.pem").unwrap());
    }

    #[test]
    fn test_missing_cert_expires_soon() {
        assert!(cert_expires_within("/nonexistent/cert.pem", 86400).unwrap());
    }
}

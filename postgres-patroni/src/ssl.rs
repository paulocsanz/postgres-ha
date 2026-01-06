//! SSL certificate utilities
//!
//! Functions for validating SSL certificates and checking expiration.
//! Uses the openssl crate for direct certificate parsing without subprocess calls.

use anyhow::{Context, Result};
use openssl::x509::X509;
use std::fs;
use std::path::Path;

/// Check if a certificate is valid x509v3 with DNS:localhost in SAN
pub fn is_valid_x509v3_cert(cert_path: &str) -> Result<bool> {
    if !Path::new(cert_path).exists() {
        return Ok(false);
    }

    let pem_data = fs::read(cert_path).context("Failed to read certificate file")?;

    let cert = X509::from_pem(&pem_data).context("Failed to parse certificate as PEM")?;

    // Check for DNS:localhost in Subject Alternative Names
    if let Some(san) = cert.subject_alt_names() {
        for name in san {
            if let Some(dns) = name.dnsname() {
                if dns == "localhost" {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
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

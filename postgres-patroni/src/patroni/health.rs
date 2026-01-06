//! Patroni health checking

use std::time::Duration;

/// Check Patroni health via REST API
pub async fn check_health(timeout_secs: u64) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    client
        .get("http://localhost:8008/health")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

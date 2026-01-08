//! HAProxy process monitoring
//!
//! Monitors HAProxy backend health and emits telemetry when no primary is available.

use anyhow::Result;
use common::{Telemetry, TelemetryEvent};
use std::process::Child;
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

const STATS_URL: &str = "http://localhost:8404/stats;csv";
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Run the monitoring loop for HAProxy
///
/// Monitors:
/// - HAProxy process health
/// - Backend availability (emits telemetry when no primary available)
pub fn run_monitoring_loop(
    mut child: Child,
    telemetry: &Telemetry,
    single_node_mode: bool,
) -> Result<()> {
    let pid = child.id();
    info!(pid, "HAProxy started, beginning monitoring");

    // Skip backend monitoring in single node mode - no Patroni health checks
    if single_node_mode {
        info!("Single node mode: skipping backend health monitoring");
        let status = child.wait()?;
        error!(?status, "HAProxy exited");
        std::process::exit(status.code().unwrap_or(1));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;

    let mut no_primary_alerted = false;

    loop {
        // Check if HAProxy is still running
        match child.try_wait() {
            Ok(Some(status)) => {
                error!(?status, "HAProxy exited unexpectedly");
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(None) => {} // Still running
            Err(e) => {
                error!(error = %e, "Failed to check HAProxy status");
                std::process::exit(1);
            }
        }

        // Check backend health
        match check_primary_backend(&client) {
            Ok(healthy_count) => {
                if healthy_count == 0 {
                    if !no_primary_alerted {
                        warn!("No healthy primary backend - cluster has no leader");
                        telemetry.send(TelemetryEvent::DcsUnavailable {
                            node: "haproxy".to_string(),
                            scope: "postgresql_primary_backend".to_string(),
                        });
                        no_primary_alerted = true;
                    }
                } else {
                    if no_primary_alerted {
                        info!(healthy_count, "Primary backend recovered");
                    }
                    no_primary_alerted = false;
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to check backend health");
            }
        }

        thread::sleep(CHECK_INTERVAL);
    }
}

/// Check how many healthy servers are in the primary backend
fn check_primary_backend(client: &reqwest::blocking::Client) -> Result<usize> {
    let resp = client.get(STATS_URL).send()?;
    let body = resp.text()?;

    // HAProxy CSV format: pxname,svname,status,...
    // We want rows where pxname=postgresql_primary_backend and status=UP
    let healthy_count = body
        .lines()
        .filter(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            // pxname is column 0, svname is column 1, status is column 17
            parts.len() > 17
                && parts[0] == "postgresql_primary_backend"
                && parts[1] != "BACKEND" // Skip the backend summary row
                && parts[17] == "UP"
        })
        .count();

    Ok(healthy_count)
}

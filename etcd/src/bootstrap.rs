//! Bootstrap logic for etcd cluster
//!
//! Handles leader election, recovery detection, and cluster initialization.

use crate::cluster::{
    add_self_to_cluster, check_cluster_health, clear_directory, get_current_cluster,
    has_local_data, promote_self, remove_stale_self,
};
use crate::config::{get_leader_endpoint, get_my_peer_url, parse_initial_cluster, peer_to_client_url, Config};
use anyhow::{anyhow, Result};
use common::{etcdctl, etcdctl_probe, Telemetry, TelemetryEvent};
use std::path::Path;
use tokio::fs;
use tokio::time::sleep;
use tracing::{info, warn};

/// Check if any other peer has a healthy cluster (for recovery detection)
pub async fn check_existing_cluster(initial_cluster: &str, my_name: &str) -> Result<Option<String>> {
    info!("Checking for existing cluster on other peers...");

    let cluster = parse_initial_cluster(initial_cluster)?;
    for (name, peer_url) in cluster.iter() {
        if name == my_name {
            continue;
        }

        let client_endpoint = peer_to_client_url(peer_url);
        info!(peer = %name, endpoint = %client_endpoint, "Checking peer");

        if etcdctl_probe(&["endpoint", "health", &format!("--endpoints={}", client_endpoint)])
            .await?
        {
            info!(peer = %name, "Found healthy cluster");
            return Ok(Some(client_endpoint));
        }
    }

    Ok(None)
}

/// Wait for leader or any healthy peer
pub async fn wait_for_any_healthy_peer(
    config: &Config,
    preferred_leader: &str,
) -> Result<(String, String)> {
    let cluster = parse_initial_cluster(&config.initial_cluster)?;

    info!(leader = %preferred_leader, "Waiting for bootstrap leader or any healthy peer");

    let start = std::time::Instant::now();
    while start.elapsed() < config.peer_wait_timeout {
        // Try preferred leader first
        if let Some(endpoint) = get_leader_endpoint(&config.initial_cluster, preferred_leader)? {
            if etcdctl_probe(&["endpoint", "health", &format!("--endpoints={}", endpoint)]).await? {
                info!(leader = %preferred_leader, "Leader is healthy");
                return Ok((preferred_leader.to_string(), endpoint));
            }
            info!(leader = %preferred_leader, "Leader health check failed");
        }

        // Try any other peer
        for (name, peer_url) in cluster.iter() {
            if name == &config.etcd_name || name == preferred_leader {
                continue;
            }

            let client_endpoint = peer_to_client_url(peer_url);
            if etcdctl_probe(&["endpoint", "health", &format!("--endpoints={}", client_endpoint)])
                .await?
            {
                info!(peer = %name, "Found healthy peer");
                return Ok((name.clone(), client_endpoint));
            }
            info!(peer = %name, "Peer health check failed");
        }

        info!(
            elapsed = ?start.elapsed(),
            timeout = ?config.peer_wait_timeout,
            "No healthy peers yet"
        );

        sleep(config.peer_check_interval).await;
    }

    Err(anyhow!("Timeout waiting for any healthy peer"))
}

/// Clean stale data on startup (only if no bootstrap marker)
pub async fn clean_stale_data(config: &Config, telemetry: &Telemetry) -> Result<()> {
    let data_path = Path::new(&config.data_dir);
    if !data_path.exists() {
        return Ok(());
    }

    let has_data = has_local_data(&config.data_dir).await?;
    let marker_exists = Path::new(&config.bootstrap_marker()).exists();

    if has_data && !marker_exists {
        info!("Found stale data from incomplete bootstrap - cleaning");
        match clear_directory(data_path).await {
            Ok(()) => {
                telemetry.send(TelemetryEvent::EtcdDataCleared {
                    node: config.etcd_name.clone(),
                    reason: "stale data from incomplete bootstrap".to_string(),
                });
                info!("Data directory cleaned");
            }
            Err(e) => {
                telemetry.send(TelemetryEvent::ComponentError {
                    component: "etcd".to_string(),
                    error: e.to_string(),
                    context: "clearing stale data on startup".to_string(),
                });
                return Err(e);
            }
        }
    } else if has_data {
        info!("Found data with bootstrap marker - preserving");
    }

    Ok(())
}

/// Monitor and mark bootstrap complete
pub async fn monitor_and_mark_bootstrap(
    config: &Config,
    joined_as_learner: bool,
    telemetry: Telemetry,
) -> Result<()> {
    let mut promoted = false;

    loop {
        sleep(std::time::Duration::from_secs(5)).await;

        let is_healthy = check_cluster_health(&config.initial_cluster).await?;

        if is_healthy {
            if joined_as_learner && !promoted {
                info!("Healthy, attempting promotion");
                match promote_self(&config.initial_cluster, &config.etcd_name, &telemetry).await {
                    Ok(_) => {
                        promoted = true;
                    }
                    Err(e) => {
                        warn!(error = %e, "Promotion failed, will retry");
                    }
                }
            }

            let marker_path = config.bootstrap_marker();
            if !Path::new(&marker_path).exists() && (!joined_as_learner || promoted) {
                fs::write(&marker_path, "1").await?;
                info!("Bootstrap marked complete");
            }
        }
    }
}

/// Result of bootstrap determination
pub struct BootstrapParams {
    pub initial_cluster: String,
    pub initial_cluster_state: String,
    pub joined_as_learner: bool,
}

/// Determine bootstrap parameters for the leader node
pub async fn bootstrap_as_leader(
    config: &Config,
    telemetry: &Telemetry,
) -> Result<Option<BootstrapParams>> {
    let marker_exists = Path::new(&config.bootstrap_marker()).exists();
    let cluster = parse_initial_cluster(&config.initial_cluster)?;

    if marker_exists {
        return Ok(Some(BootstrapParams {
            initial_cluster: config.initial_cluster.clone(),
            initial_cluster_state: "existing".to_string(),
            joined_as_learner: false,
        }));
    }

    // Check for recovery scenario - existing cluster on other peers
    if let Some(existing_endpoint) =
        check_existing_cluster(&config.initial_cluster, &config.etcd_name).await?
    {
        info!("RECOVERY MODE: Found existing cluster");

        telemetry.send(TelemetryEvent::EtcdRecoveryMode {
            node: config.etcd_name.clone(),
            reason: "Leader volume lost, cluster exists".to_string(),
        });

        let my_peer_url = get_my_peer_url(&config.initial_cluster, &config.etcd_name)?
            .ok_or_else(|| anyhow!("Could not find my peer URL in ETCD_INITIAL_CLUSTER"))?;

        if let Err(e) = remove_stale_self(&existing_endpoint, &config.etcd_name, &my_peer_url, telemetry).await {
            warn!(error = %e, "Failed to remove stale self, continuing anyway");
        }

        let output = etcdctl(&[
            "member",
            "add",
            &config.etcd_name,
            "--learner",
            &format!("--peer-urls={}", my_peer_url),
            &format!("--endpoints={}", existing_endpoint),
        ])
        .await;

        match output {
            Ok(out) => {
                telemetry.send(TelemetryEvent::EtcdNodeJoined {
                    node: config.etcd_name.clone(),
                    joined_as: "learner".to_string(),
                });

                let mut cluster_str = String::new();
                for line in out.lines() {
                    if line.contains("ETCD_INITIAL_CLUSTER=") {
                        if let Some(c) = line
                            .split("ETCD_INITIAL_CLUSTER=")
                            .nth(1)
                            .map(|s| s.trim_matches('"').to_string())
                        {
                            cluster_str = c;
                            break;
                        }
                    }
                }

                if cluster_str.is_empty() {
                    cluster_str =
                        get_current_cluster(&existing_endpoint, &config.etcd_name, &my_peer_url)
                            .await?;
                }

                info!(cluster = %cluster_str, "Joining as learner (recovery)");
                return Ok(Some(BootstrapParams {
                    initial_cluster: cluster_str,
                    initial_cluster_state: "existing".to_string(),
                    joined_as_learner: true,
                }));
            }
            Err(e) => {
                warn!(error = %e, "Failed to add as learner during recovery");
                return Ok(None); // Signal retry needed
            }
        }
    }

    // Fresh bootstrap - single node cluster
    let my_peer_url = get_my_peer_url(&config.initial_cluster, &config.etcd_name)?
        .ok_or_else(|| anyhow!("Could not find my peer URL in ETCD_INITIAL_CLUSTER"))?;

    let single_node_cluster = format!("{}={}", config.etcd_name, my_peer_url);
    info!(cluster = %single_node_cluster, "Bootstrapping single-node cluster");

    telemetry.send(TelemetryEvent::EtcdBootstrap {
        node: config.etcd_name.clone(),
        is_leader: true,
        cluster_size: cluster.len(),
    });

    Ok(Some(BootstrapParams {
        initial_cluster: single_node_cluster,
        initial_cluster_state: "new".to_string(),
        joined_as_learner: false,
    }))
}

/// Determine bootstrap parameters for a follower node
pub async fn bootstrap_as_follower(
    config: &Config,
    bootstrap_leader: &str,
    telemetry: &Telemetry,
) -> Result<Option<BootstrapParams>> {
    let marker_exists = Path::new(&config.bootstrap_marker()).exists();

    if marker_exists {
        return Ok(Some(BootstrapParams {
            initial_cluster: config.initial_cluster.clone(),
            initial_cluster_state: "existing".to_string(),
            joined_as_learner: false,
        }));
    }

    // Wait for a healthy peer
    let (healthy_peer, endpoint) =
        match wait_for_any_healthy_peer(config, bootstrap_leader).await {
            Ok(result) => result,
            Err(e) => {
                warn!(error = %e, "Failed to find healthy peer");
                return Ok(None); // Signal retry needed
            }
        };

    match add_self_to_cluster(config, &healthy_peer, &endpoint, telemetry).await {
        Ok(cluster_str) => {
            info!(cluster = %cluster_str, via = %healthy_peer, "Joining as learner");
            telemetry.send(TelemetryEvent::EtcdNodeJoined {
                node: config.etcd_name.clone(),
                joined_as: "learner".to_string(),
            });
            Ok(Some(BootstrapParams {
                initial_cluster: cluster_str,
                initial_cluster_state: "existing".to_string(),
                joined_as_learner: true,
            }))
        }
        Err(e) => {
            warn!(error = %e, "Failed to add as learner");
            Ok(None) // Signal retry needed
        }
    }
}

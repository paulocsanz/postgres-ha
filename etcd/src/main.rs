//! etcd bootstrap wrapper with leader-based startup and learner mode
//!
//! Bootstraps etcd cluster using single-node + learner pattern to avoid deadlocks:
//! 1. Leader (alphabetically first) bootstraps single-node cluster
//! 2. Other nodes wait, then join as learners (non-voting)
//! 3. Learners promote to voting members once healthy
//!
//! Recovery: If leader loses volume, it detects existing cluster and joins as learner

use anyhow::{anyhow, Context, Result};
use common::{etcdctl, init_logging, ConfigExt, Telemetry, TelemetryEvent};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{error, info, warn};

struct Config {
    data_dir: String,
    max_retries: u32,
    retry_delay: Duration,
    peer_wait_timeout: Duration,
    peer_check_interval: Duration,
    etcd_name: String,
    initial_cluster: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            data_dir: String::env_or("ETCD_DATA_DIR", "/var/lib/etcd"),
            max_retries: u32::env_parse("ETCD_MAX_RETRIES", 60),
            retry_delay: Duration::from_secs(u64::env_parse("ETCD_RETRY_DELAY", 5)),
            peer_wait_timeout: Duration::from_secs(u64::env_parse("ETCD_PEER_WAIT_TIMEOUT", 300)),
            peer_check_interval: Duration::from_secs(u64::env_parse("ETCD_PEER_CHECK_INTERVAL", 5)),
            etcd_name: String::env_required("ETCD_NAME")?,
            initial_cluster: String::env_required("ETCD_INITIAL_CLUSTER")?,
        })
    }

    fn bootstrap_marker(&self) -> String {
        format!("{}/.bootstrap_complete", self.data_dir)
    }
}

/// Parse the initial cluster string into a map of name -> peer_url
fn parse_initial_cluster(cluster: &str) -> HashMap<String, String> {
    cluster
        .split(',')
        .filter_map(|entry| {
            let parts: Vec<&str> = entry.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// Get bootstrap leader (alphabetically first node name)
fn get_bootstrap_leader(initial_cluster: &str) -> String {
    let cluster = parse_initial_cluster(initial_cluster);
    cluster.keys().min().cloned().unwrap_or_default()
}

/// Get leader's client endpoint (port 2379)
fn get_leader_endpoint(initial_cluster: &str, leader: &str) -> Option<String> {
    let cluster = parse_initial_cluster(initial_cluster);
    cluster.get(leader).map(|url| url.replace(":2380", ":2379"))
}

/// Get my peer URL from ETCD_INITIAL_CLUSTER
fn get_my_peer_url(initial_cluster: &str, etcd_name: &str) -> Option<String> {
    let cluster = parse_initial_cluster(initial_cluster);
    cluster.get(etcd_name).cloned()
}

/// Clear all contents of a directory without removing the directory itself
async fn clear_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).await?;
        } else {
            fs::remove_file(&path).await?;
        }
    }
    Ok(())
}

/// Check cluster health via localhost or voting member
async fn check_cluster_health(initial_cluster: &str) -> bool {
    if etcdctl(&["endpoint", "health", "--endpoints=http://127.0.0.1:2379"])
        .await
        .is_ok()
    {
        return true;
    }

    if let Some(endpoint) = get_voting_member_endpoint(initial_cluster).await {
        return etcdctl(&["endpoint", "health", &format!("--endpoints={}", endpoint)])
            .await
            .is_ok();
    }

    false
}

/// Check if any other peer has a healthy cluster (for recovery detection)
async fn check_existing_cluster(initial_cluster: &str, my_name: &str) -> Option<String> {
    info!("Checking for existing cluster on other peers...");

    let cluster = parse_initial_cluster(initial_cluster);
    for (name, peer_url) in cluster.iter() {
        if name == my_name {
            continue;
        }

        let client_endpoint = peer_url.replace(":2380", ":2379");
        info!(peer = %name, endpoint = %client_endpoint, "Checking peer");

        if etcdctl(&["endpoint", "health", &format!("--endpoints={}", client_endpoint)])
            .await
            .is_ok()
        {
            info!(peer = %name, "Found healthy cluster");
            return Some(client_endpoint);
        }
    }

    None
}

/// Get member list from etcd
async fn get_member_list(endpoint: &str) -> Result<Vec<MemberInfo>> {
    let output = etcdctl(&[
        "member",
        "list",
        &format!("--endpoints={}", endpoint),
        "-w",
        "simple",
    ])
    .await?;

    let members: Vec<MemberInfo> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            if parts.len() >= 5 {
                Some(MemberInfo {
                    id: parts[0].to_string(),
                    name: parts[2].to_string(),
                    peer_url: parts[3].to_string(),
                    is_learner: parts.get(5).map(|s| *s == "true").unwrap_or(false),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(members)
}

#[derive(Debug)]
struct MemberInfo {
    id: String,
    name: String,
    peer_url: String,
    is_learner: bool,
}

/// Remove stale member entry for this node
async fn remove_stale_self(endpoint: &str, my_name: &str, my_peer_url: &str) -> Result<()> {
    info!("Checking for stale member entry...");

    let members = get_member_list(endpoint).await?;

    for member in members {
        if member.name == my_name || member.peer_url == my_peer_url {
            info!(id = %member.id, "Removing stale member entry");
            etcdctl(&[
                "member",
                "remove",
                &member.id,
                &format!("--endpoints={}", endpoint),
            ])
            .await?;
            info!("Stale member removed");
            return Ok(());
        }
    }

    info!("No stale member entry found");
    Ok(())
}

/// Wait for leader or any healthy peer
async fn wait_for_any_healthy_peer(config: &Config, preferred_leader: &str) -> Result<String> {
    let cluster = parse_initial_cluster(&config.initial_cluster);

    info!(leader = %preferred_leader, "Waiting for bootstrap leader or any healthy peer");

    let start = std::time::Instant::now();
    while start.elapsed() < config.peer_wait_timeout {
        // Try preferred leader first
        if let Some(endpoint) = get_leader_endpoint(&config.initial_cluster, preferred_leader) {
            match etcdctl(&["endpoint", "health", &format!("--endpoints={}", endpoint)]).await {
                Ok(_) => {
                    info!(leader = %preferred_leader, "Leader is healthy");
                    return Ok(preferred_leader.to_string());
                }
                Err(e) => {
                    info!(leader = %preferred_leader, error = %e, "Leader health check failed");
                }
            }
        }

        // Try any other peer
        for (name, peer_url) in cluster.iter() {
            if name == &config.etcd_name || name == preferred_leader {
                continue;
            }

            let client_endpoint = peer_url.replace(":2380", ":2379");
            match etcdctl(&["endpoint", "health", &format!("--endpoints={}", client_endpoint)])
                .await
            {
                Ok(_) => {
                    info!(peer = %name, "Found healthy peer");
                    return Ok(name.clone());
                }
                Err(e) => {
                    info!(peer = %name, error = %e, "Peer health check failed");
                }
            }
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

/// Check if we have valid local etcd data
async fn has_local_data(data_dir: &str) -> bool {
    let wal_dir = format!("{}/member/wal", data_dir);
    if let Ok(mut entries) = fs::read_dir(&wal_dir).await {
        if entries.next_entry().await.ok().flatten().is_some() {
            return true;
        }
    }
    false
}

/// Add this node to an existing cluster as a learner
async fn add_self_to_cluster(config: &Config, leader: &str) -> Result<String> {
    let endpoint = get_leader_endpoint(&config.initial_cluster, leader)
        .ok_or_else(|| anyhow!("Could not find leader endpoint"))?;

    let my_peer_url = get_my_peer_url(&config.initial_cluster, &config.etcd_name)
        .ok_or_else(|| anyhow!("Could not find my peer URL"))?;

    info!(node = %config.etcd_name, via = %endpoint, "Adding self as learner");

    // Check if already a member
    let members = get_member_list(&endpoint).await?;
    for member in &members {
        if member.name == config.etcd_name || member.peer_url == my_peer_url {
            if !has_local_data(&config.data_dir).await {
                warn!("Registered as member but no local data - removing stale entry");
                remove_stale_self(&endpoint, &config.etcd_name, &my_peer_url).await?;

                // Clean partial data
                let _ = clear_directory(Path::new(&config.data_dir)).await;
                break;
            } else {
                info!("Already a member with local data");
                return get_current_cluster(&endpoint, &config.etcd_name, &my_peer_url).await;
            }
        }
    }

    // Add as learner
    let output = etcdctl(&[
        "member",
        "add",
        &config.etcd_name,
        "--learner",
        &format!("--peer-urls={}", my_peer_url),
        &format!("--endpoints={}", endpoint),
    ])
    .await?;

    info!("Successfully added as learner");

    // Extract ETCD_INITIAL_CLUSTER from output
    for line in output.lines() {
        if line.contains("ETCD_INITIAL_CLUSTER=") {
            let cluster = line
                .split("ETCD_INITIAL_CLUSTER=")
                .nth(1)
                .map(|s| s.trim_matches('"').to_string());
            if let Some(c) = cluster {
                if !c.is_empty() {
                    return Ok(c);
                }
            }
        }
    }

    info!("Extracting cluster from member list");
    get_current_cluster(&endpoint, &config.etcd_name, &my_peer_url).await
}

/// Build current cluster membership for joining node
async fn get_current_cluster(endpoint: &str, my_name: &str, my_peer_url: &str) -> Result<String> {
    let members = get_member_list(endpoint).await?;

    let mut cluster_parts: Vec<String> = members
        .iter()
        .filter(|m| !m.name.is_empty() && !m.peer_url.is_empty())
        .map(|m| format!("{}={}", m.name, m.peer_url))
        .collect();

    if !cluster_parts
        .iter()
        .any(|p| p.starts_with(&format!("{}=", my_name)))
    {
        cluster_parts.push(format!("{}={}", my_name, my_peer_url));
    }

    Ok(cluster_parts.join(","))
}

/// Get my member ID from etcd cluster
async fn get_my_member_id(endpoint: &str, my_name: &str) -> Option<String> {
    if let Ok(members) = get_member_list(endpoint).await {
        for member in members {
            if member.name == my_name {
                return Some(member.id);
            }
        }
    }
    None
}

/// Check if this member is a learner
async fn is_learner(endpoint: &str, my_name: &str) -> bool {
    if let Ok(members) = get_member_list(endpoint).await {
        for member in members {
            if member.name == my_name {
                return member.is_learner;
            }
        }
    }
    false
}

/// Find a voting member endpoint
async fn get_voting_member_endpoint(initial_cluster: &str) -> Option<String> {
    let cluster = parse_initial_cluster(initial_cluster);

    for (_name, peer_url) in cluster.iter() {
        let client_endpoint = peer_url.replace(":2380", ":2379");
        if etcdctl(&["member", "list", &format!("--endpoints={}", client_endpoint)])
            .await
            .is_ok()
        {
            return Some(client_endpoint);
        }
    }

    None
}

/// Promote self from learner to voting member
async fn promote_self(
    initial_cluster: &str,
    my_name: &str,
    telemetry: &Telemetry,
) -> Result<()> {
    let endpoint = get_voting_member_endpoint(initial_cluster)
        .await
        .ok_or_else(|| anyhow!("Could not find voting member endpoint"))?;

    let member_id = get_my_member_id(&endpoint, my_name)
        .await
        .ok_or_else(|| anyhow!("Could not find my member ID"))?;

    if !is_learner(&endpoint, my_name).await {
        info!("Already a voting member");
        return Ok(());
    }

    info!(id = %member_id, via = %endpoint, "Promoting from learner to voting member");

    match etcdctl(&[
        "member",
        "promote",
        &member_id,
        &format!("--endpoints={}", endpoint),
    ])
    .await
    {
        Ok(_) => {
            info!("Promoted to voting member");
            telemetry.send(TelemetryEvent::EtcdNodePromoted {
                node: my_name.to_string(),
            });
            Ok(())
        }
        Err(e) => {
            if e.to_string().contains("is not a learner") {
                info!("Already a voting member");
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// Clean stale data on startup (only if no bootstrap marker)
async fn clean_stale_data(config: &Config) -> Result<()> {
    let data_path = Path::new(&config.data_dir);
    if !data_path.exists() {
        return Ok(());
    }

    let has_data = has_local_data(&config.data_dir).await;
    let marker_exists = Path::new(&config.bootstrap_marker()).exists();

    if has_data && !marker_exists {
        info!("Found stale data from incomplete bootstrap - cleaning");
        clear_directory(data_path).await?;
        info!("Data directory cleaned");
    } else if has_data {
        info!("Found data with bootstrap marker - preserving");
    }

    Ok(())
}

/// Start etcd process
async fn start_etcd(
    initial_cluster: &str,
    initial_cluster_state: &str,
) -> Result<tokio::process::Child> {
    info!(
        cluster = %initial_cluster,
        state = %initial_cluster_state,
        "Starting etcd"
    );

    let child = Command::new("/usr/local/bin/etcd")
        .arg("--auto-compaction-retention=1")
        .arg("--max-learners=2")
        .env("ETCD_INITIAL_CLUSTER", initial_cluster)
        .env("ETCD_INITIAL_CLUSTER_STATE", initial_cluster_state)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to start etcd")?;

    Ok(child)
}

/// Monitor and mark bootstrap complete
async fn monitor_and_mark_bootstrap(
    config: &Config,
    joined_as_learner: bool,
    telemetry: Telemetry,
) -> Result<()> {
    let mut promoted = false;

    loop {
        sleep(Duration::from_secs(5)).await;

        if check_cluster_health(&config.initial_cluster).await {
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
            if !Path::new(&marker_path).exists() {
                if !joined_as_learner || promoted {
                    fs::write(&marker_path, "1").await?;
                    info!("Bootstrap marked complete");
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = init_logging("etcd");

    let telemetry = Telemetry::from_env("etcd");
    let config = Config::from_env()?;

    fs::create_dir_all(&config.data_dir)
        .await
        .context("Failed to create data directory")?;

    clean_stale_data(&config).await?;

    let bootstrap_leader = get_bootstrap_leader(&config.initial_cluster);
    let is_leader = config.etcd_name == bootstrap_leader;
    let cluster = parse_initial_cluster(&config.initial_cluster);

    info!(
        leader = %bootstrap_leader,
        node = %config.etcd_name,
        is_leader = is_leader,
        "Cluster bootstrap"
    );

    let mut attempt = 1;
    while attempt <= config.max_retries {
        info!(attempt, max = config.max_retries, "Starting etcd");

        let (initial_cluster, initial_cluster_state, joined_as_learner) = if is_leader {
            let marker_exists = Path::new(&config.bootstrap_marker()).exists();

            if !marker_exists {
                if let Some(existing_endpoint) =
                    check_existing_cluster(&config.initial_cluster, &config.etcd_name).await
                {
                    info!("RECOVERY MODE: Found existing cluster");

                    telemetry.send(TelemetryEvent::EtcdRecoveryMode {
                        node: config.etcd_name.clone(),
                        reason: "Leader volume lost, cluster exists".to_string(),
                    });

                    let my_peer_url = get_my_peer_url(&config.initial_cluster, &config.etcd_name)
                        .ok_or_else(|| anyhow!("Could not find my peer URL"))?;

                    let _ =
                        remove_stale_self(&existing_endpoint, &config.etcd_name, &my_peer_url)
                            .await;

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
                                cluster_str = get_current_cluster(
                                    &existing_endpoint,
                                    &config.etcd_name,
                                    &my_peer_url,
                                )
                                .await?;
                            }

                            info!(cluster = %cluster_str, "Joining as learner (recovery)");
                            (cluster_str, "existing".to_string(), true)
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to add as learner during recovery");
                            telemetry.send(TelemetryEvent::EtcdStartupFailed {
                                node: config.etcd_name.clone(),
                                attempt,
                                max_attempts: config.max_retries,
                                error: e.to_string(),
                            });
                            attempt += 1;
                            sleep(config.retry_delay).await;
                            continue;
                        }
                    }
                } else {
                    let my_peer_url = get_my_peer_url(&config.initial_cluster, &config.etcd_name)
                        .ok_or_else(|| anyhow!("Could not find my peer URL"))?;

                    let single_node_cluster = format!("{}={}", config.etcd_name, my_peer_url);
                    info!(cluster = %single_node_cluster, "Bootstrapping single-node cluster");

                    telemetry.send(TelemetryEvent::EtcdBootstrap {
                        node: config.etcd_name.clone(),
                        is_leader: true,
                        cluster_size: cluster.len(),
                    });

                    (single_node_cluster, "new".to_string(), false)
                }
            } else {
                (config.initial_cluster.clone(), "existing".to_string(), false)
            }
        } else {
            let marker_exists = Path::new(&config.bootstrap_marker()).exists();

            if !marker_exists {
                let healthy_peer = match wait_for_any_healthy_peer(&config, &bootstrap_leader).await
                {
                    Ok(peer) => peer,
                    Err(e) => {
                        warn!(error = %e, "Failed to find healthy peer");
                        telemetry.send(TelemetryEvent::EtcdStartupFailed {
                            node: config.etcd_name.clone(),
                            attempt,
                            max_attempts: config.max_retries,
                            error: e.to_string(),
                        });
                        attempt += 1;
                        sleep(config.retry_delay).await;
                        continue;
                    }
                };

                match add_self_to_cluster(&config, &healthy_peer).await {
                    Ok(cluster_str) => {
                        info!(cluster = %cluster_str, via = %healthy_peer, "Joining as learner");
                        telemetry.send(TelemetryEvent::EtcdNodeJoined {
                            node: config.etcd_name.clone(),
                            joined_as: "learner".to_string(),
                        });
                        (cluster_str, "existing".to_string(), true)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to add as learner");
                        telemetry.send(TelemetryEvent::EtcdStartupFailed {
                            node: config.etcd_name.clone(),
                            attempt,
                            max_attempts: config.max_retries,
                            error: e.to_string(),
                        });
                        attempt += 1;
                        sleep(config.retry_delay).await;
                        continue;
                    }
                }
            } else {
                (config.initial_cluster.clone(), "existing".to_string(), false)
            }
        };

        let mut child = start_etcd(&initial_cluster, &initial_cluster_state).await?;
        info!(pid = ?child.id(), "etcd started");

        let monitor_config = Config::from_env()?;
        let monitor_telemetry = telemetry.clone();
        let monitor_handle = tokio::spawn(async move {
            let _ = monitor_and_mark_bootstrap(&monitor_config, joined_as_learner, monitor_telemetry)
                .await;
        });

        let status = child.wait().await?;
        monitor_handle.abort();

        if status.success() {
            info!("etcd exited cleanly");
            return Ok(());
        }

        let exit_code = status.code().unwrap_or(-1);
        info!(exit_code, "etcd exited");

        let marker_exists = Path::new(&config.bootstrap_marker()).exists();
        if !marker_exists && has_local_data(&config.data_dir).await {
            info!("Bootstrap incomplete - cleaning data");
            let _ = clear_directory(Path::new(&config.data_dir)).await;
        } else if marker_exists {
            info!("Bootstrap complete - preserving data");
        }

        attempt += 1;
        if attempt <= config.max_retries {
            info!(delay = ?config.retry_delay, "Retrying");
            sleep(config.retry_delay).await;
        }
    }

    error!(attempts = config.max_retries, "Failed to start etcd");
    std::process::exit(1);
}

//! etcd bootstrap wrapper with leader-based startup and learner mode
//!
//! Problem: etcd nodes starting at different times fail to form cluster because
//! all nodes waiting for each other on TCP creates a deadlock. etcd also has
//! hard timeouts that corrupt local state if quorum isn't reached. Additionally,
//! adding voting members directly can disrupt quorum during cluster formation.
//!
//! Solution: Single-node bootstrap with learner mode (etcd v3.4+)
//! 1. Determine bootstrap leader (alphabetically first node name)
//! 2. Leader bootstraps single-node cluster (instant quorum)
//! 3. Other nodes wait for leader, add themselves as LEARNERS (non-voting)
//! 4. Learners sync data, then auto-promote to voting members once healthy
//!
//! Recovery mode (leader volume loss):
//! - Before bootstrapping, leader checks if other peers have a healthy cluster
//! - If yes: leader removes its stale entry and joins as learner (not bootstrap)
//! - This prevents split-brain when leader loses volume but cluster still exists

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::env;
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
            data_dir: env::var("ETCD_DATA_DIR").unwrap_or_else(|_| "/var/lib/etcd".to_string()),
            max_retries: env::var("ETCD_MAX_RETRIES")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .unwrap_or(60),
            retry_delay: Duration::from_secs(
                env::var("ETCD_RETRY_DELAY")
                    .unwrap_or_else(|_| "5".to_string())
                    .parse()
                    .unwrap_or(5),
            ),
            peer_wait_timeout: Duration::from_secs(
                env::var("ETCD_PEER_WAIT_TIMEOUT")
                    .unwrap_or_else(|_| "300".to_string())
                    .parse()
                    .unwrap_or(300),
            ),
            peer_check_interval: Duration::from_secs(
                env::var("ETCD_PEER_CHECK_INTERVAL")
                    .unwrap_or_else(|_| "5".to_string())
                    .parse()
                    .unwrap_or(5),
            ),
            etcd_name: env::var("ETCD_NAME").context("ETCD_NAME must be set")?,
            initial_cluster: env::var("ETCD_INITIAL_CLUSTER")
                .context("ETCD_INITIAL_CLUSTER must be set")?,
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

/// Run etcdctl command and return output
async fn etcdctl(args: &[&str]) -> Result<String> {
    let output = Command::new("etcdctl")
        .args(args)
        .output()
        .await
        .context("Failed to run etcdctl")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow!(
            "etcdctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Check cluster health via localhost or voting member
async fn check_cluster_health(initial_cluster: &str) -> bool {
    // Try localhost first (works for voting members)
    if etcdctl(&["endpoint", "health", "--endpoints=http://127.0.0.1:2379"])
        .await
        .is_ok()
    {
        return true;
    }

    // For learners, localhost fails - check via a voting member
    if let Some(endpoint) = get_voting_member_endpoint(initial_cluster).await {
        return etcdctl(&["endpoint", "health", &format!("--endpoints={}", endpoint)])
            .await
            .is_ok();
    }

    false
}

/// Check if any other peer has a healthy cluster (for recovery detection)
async fn check_existing_cluster(initial_cluster: &str, my_name: &str) -> Option<String> {
    info!("Checking if other peers have an existing cluster...");

    let cluster = parse_initial_cluster(initial_cluster);
    for (name, peer_url) in cluster.iter() {
        if name == my_name {
            continue;
        }

        let client_endpoint = peer_url.replace(":2380", ":2379");
        info!("Checking peer {} at {}...", name, client_endpoint);

        if etcdctl(&[
            "endpoint",
            "health",
            &format!("--endpoints={}", client_endpoint),
        ])
        .await
        .is_ok()
        {
            info!("Found healthy cluster at peer {}", name);
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
                    _status: parts[1].to_string(),
                    name: parts[2].to_string(),
                    peer_url: parts[3].to_string(),
                    _client_url: parts[4].to_string(),
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
    _status: String,
    name: String,
    peer_url: String,
    _client_url: String,
    is_learner: bool,
}

/// Remove stale member entry for this node
async fn remove_stale_self(endpoint: &str, my_name: &str, my_peer_url: &str) -> Result<()> {
    info!("Checking for stale member entry to remove...");

    let members = get_member_list(endpoint).await?;

    for member in members {
        if member.name == my_name || member.peer_url == my_peer_url {
            info!("Found stale member entry (ID: {}), removing...", member.id);
            etcdctl(&[
                "member",
                "remove",
                &member.id,
                &format!("--endpoints={}", endpoint),
            ])
            .await?;
            info!("Successfully removed stale member entry");
            return Ok(());
        }
    }

    info!("No stale member entry found");
    Ok(())
}

/// Wait for leader or any healthy peer to be accepting connections
/// Returns the name of the node we can connect to
async fn wait_for_any_healthy_peer(config: &Config, preferred_leader: &str) -> Result<String> {
    let cluster = parse_initial_cluster(&config.initial_cluster);

    info!("Waiting for bootstrap leader {} or any healthy peer...", preferred_leader);

    let start = std::time::Instant::now();
    while start.elapsed() < config.peer_wait_timeout {
        // Try preferred leader first
        if let Some(endpoint) = get_leader_endpoint(&config.initial_cluster, preferred_leader) {
            match etcdctl(&["endpoint", "health", &format!("--endpoints={}", endpoint)]).await {
                Ok(_) => {
                    info!("Bootstrap leader {} is healthy", preferred_leader);
                    return Ok(preferred_leader.to_string());
                }
                Err(e) => {
                    info!("Leader {} health check failed: {}", preferred_leader, e);
                }
            }
        }

        // Try any other peer
        for (name, peer_url) in cluster.iter() {
            if name == &config.etcd_name || name == preferred_leader {
                continue;
            }

            let client_endpoint = peer_url.replace(":2380", ":2379");
            match etcdctl(&["endpoint", "health", &format!("--endpoints={}", client_endpoint)]).await {
                Ok(_) => {
                    info!("Found healthy peer {} (bootstrap leader {} not yet available)", name, preferred_leader);
                    return Ok(name.clone());
                }
                Err(e) => {
                    info!("Peer {} health check failed: {}", name, e);
                }
            }
        }

        info!(
            "No healthy peers yet ({:?}/{:?})",
            start.elapsed(),
            config.peer_wait_timeout
        );

        sleep(config.peer_check_interval).await;
    }

    Err(anyhow!("Timeout waiting for any healthy peer"))
}

/// Check if we have valid local etcd data (WAL files exist)
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

    info!(
        "Adding self ({}) as learner to cluster via {}...",
        config.etcd_name, endpoint
    );

    // Check if already a member
    let members = get_member_list(&endpoint).await?;
    for member in &members {
        if member.name == config.etcd_name || member.peer_url == my_peer_url {
            // Check if we actually have local data
            if !has_local_data(&config.data_dir).await {
                warn!("WARNING: Registered as member but no local data (volume wiped?) - removing stale entry");
                remove_stale_self(&endpoint, &config.etcd_name, &my_peer_url).await?;

                // Clean partial data
                let data_path = Path::new(&config.data_dir);
                if data_path.exists() {
                    if let Ok(mut entries) = fs::read_dir(data_path).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            let path = entry.path();
                            if path.is_dir() {
                                let _ = fs::remove_dir_all(&path).await;
                            } else {
                                let _ = fs::remove_file(&path).await;
                            }
                        }
                    }
                }
                break;
            } else {
                info!("Already a member of the cluster with local data");
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

    info!("Successfully added as learner to cluster");

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

    // Fallback to building from member list
    info!("Could not extract cluster from member add output, using member list");
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

    // Add ourselves if not in list
    if !cluster_parts.iter().any(|p| p.starts_with(&format!("{}=", my_name))) {
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

/// Find a voting member endpoint to use for cluster operations
async fn get_voting_member_endpoint(initial_cluster: &str) -> Option<String> {
    let cluster = parse_initial_cluster(initial_cluster);

    for (_name, peer_url) in cluster.iter() {
        let client_endpoint = peer_url.replace(":2380", ":2379");
        if etcdctl(&[
            "member",
            "list",
            &format!("--endpoints={}", client_endpoint),
        ])
        .await
        .is_ok()
        {
            return Some(client_endpoint);
        }
    }

    None
}

/// Promote self from learner to voting member
async fn promote_self(initial_cluster: &str, my_name: &str) -> Result<()> {
    let endpoint = get_voting_member_endpoint(initial_cluster)
        .await
        .ok_or_else(|| anyhow!("Could not find a voting member endpoint for promotion"))?;

    info!("Using voting member endpoint for promotion: {}", endpoint);

    let member_id = get_my_member_id(&endpoint, my_name)
        .await
        .ok_or_else(|| anyhow!("Could not find my member ID for promotion"))?;

    if !is_learner(&endpoint, my_name).await {
        info!("Already a voting member, no promotion needed");
        return Ok(());
    }

    info!(
        "Promoting self (ID: {}) from learner to voting member via {}...",
        member_id, endpoint
    );

    match etcdctl(&[
        "member",
        "promote",
        &member_id,
        &format!("--endpoints={}", endpoint),
    ])
    .await
    {
        Ok(_) => {
            info!("Successfully promoted to voting member");
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

/// Clean stale data on startup if bootstrap never completed
async fn clean_stale_data(config: &Config) -> Result<()> {
    let data_path = Path::new(&config.data_dir);
    let marker_str = config.bootstrap_marker();
    let marker_path = Path::new(&marker_str);

    if data_path.exists() {
        let has_data = fs::read_dir(data_path)
            .await?
            .next_entry()
            .await?
            .is_some();

        if has_data {
            if !marker_path.exists() {
                info!("Found stale data from incomplete bootstrap - cleaning...");
                let mut entries = fs::read_dir(data_path).await?;
                while let Some(entry) = entries.next_entry().await? {
                    let path = entry.path();
                    if path.is_dir() {
                        fs::remove_dir_all(&path).await?;
                    } else {
                        fs::remove_file(&path).await?;
                    }
                }
                info!("Data directory cleaned, starting fresh");
            } else {
                info!("Found data with completed bootstrap marker - preserving");
            }
        }
    }

    Ok(())
}

/// Start etcd process and return its handle
async fn start_etcd(
    initial_cluster: &str,
    initial_cluster_state: &str,
) -> Result<tokio::process::Child> {
    info!(
        "Starting etcd with cluster={}, state={}",
        initial_cluster, initial_cluster_state
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

/// Monitor and mark bootstrap complete, promoting learner if needed
async fn monitor_and_mark_bootstrap(
    config: &Config,
    joined_as_learner: bool,
) -> Result<()> {
    let mut promoted = false;

    loop {
        sleep(Duration::from_secs(5)).await;

        if check_cluster_health(&config.initial_cluster).await {
            // If we joined as a learner, try to promote ourselves
            if joined_as_learner && !promoted {
                info!("Node healthy, attempting promotion from learner...");
                match promote_self(&config.initial_cluster, &config.etcd_name).await {
                    Ok(_) => {
                        promoted = true;
                        info!("Learner promotion successful");
                    }
                    Err(e) => {
                        warn!("Learner promotion failed, will retry: {}", e);
                    }
                }
            }

            // Mark bootstrap complete only after promotion (if applicable)
            let marker_path = config.bootstrap_marker();
            if !Path::new(&marker_path).exists() {
                if !joined_as_learner || promoted {
                    fs::write(&marker_path, "1").await?;
                    info!("Cluster healthy and fully joined - bootstrap marked complete");
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    let config = Config::from_env()?;

    // Ensure data directory exists
    fs::create_dir_all(&config.data_dir).await.context("Failed to create data directory")?;

    // Clean stale data on startup
    clean_stale_data(&config).await?;

    // Determine our role
    let bootstrap_leader = get_bootstrap_leader(&config.initial_cluster);
    let is_leader = config.etcd_name == bootstrap_leader;

    info!(
        "Bootstrap leader is: {} (I am {}, is_leader={})",
        bootstrap_leader, config.etcd_name, is_leader
    );

    let mut attempt = 1;
    while attempt <= config.max_retries {
        info!("Starting etcd (attempt {}/{})...", attempt, config.max_retries);

        let (initial_cluster, initial_cluster_state, joined_as_learner) = if is_leader {
            // Bootstrap leader logic
            let marker_exists = Path::new(&config.bootstrap_marker()).exists();

            if !marker_exists {
                // Check for existing cluster (recovery mode)
                if let Some(existing_endpoint) =
                    check_existing_cluster(&config.initial_cluster, &config.etcd_name).await
                {
                    info!("RECOVERY MODE: Found existing cluster, joining instead of bootstrapping");

                    let my_peer_url =
                        get_my_peer_url(&config.initial_cluster, &config.etcd_name)
                            .ok_or_else(|| anyhow!("Could not find my peer URL"))?;

                    // Remove stale entry and join as learner
                    let _ = remove_stale_self(&existing_endpoint, &config.etcd_name, &my_peer_url).await;

                    info!("Adding self as learner to existing cluster via {}...", existing_endpoint);

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
                            info!("Successfully added as learner for recovery");

                            // Extract cluster from output or build from member list
                            let mut cluster = String::new();
                            for line in out.lines() {
                                if line.contains("ETCD_INITIAL_CLUSTER=") {
                                    if let Some(c) = line
                                        .split("ETCD_INITIAL_CLUSTER=")
                                        .nth(1)
                                        .map(|s| s.trim_matches('"').to_string())
                                    {
                                        cluster = c;
                                        break;
                                    }
                                }
                            }

                            if cluster.is_empty() {
                                cluster = get_current_cluster(
                                    &existing_endpoint,
                                    &config.etcd_name,
                                    &my_peer_url,
                                )
                                .await?;
                            }

                            info!("Joining existing cluster as learner (recovery): {}", cluster);
                            (cluster, "existing".to_string(), true)
                        }
                        Err(e) => {
                            warn!("Failed to add as learner during recovery: {}", e);
                            attempt += 1;
                            sleep(config.retry_delay).await;
                            continue;
                        }
                    }
                } else {
                    // Normal bootstrap: no existing cluster found
                    let my_peer_url =
                        get_my_peer_url(&config.initial_cluster, &config.etcd_name)
                            .ok_or_else(|| anyhow!("Could not find my peer URL"))?;

                    let single_node_cluster = format!("{}={}", config.etcd_name, my_peer_url);
                    info!("Bootstrapping as single-node cluster: {}", single_node_cluster);

                    (single_node_cluster, "new".to_string(), false)
                }
            } else {
                // Already bootstrapped, just start normally
                (config.initial_cluster.clone(), "existing".to_string(), false)
            }
        } else {
            // Non-leader: wait for any healthy peer, join existing cluster as learner
            let marker_exists = Path::new(&config.bootstrap_marker()).exists();

            if !marker_exists {
                let healthy_peer = match wait_for_any_healthy_peer(&config, &bootstrap_leader).await {
                    Ok(peer) => peer,
                    Err(e) => {
                        warn!("Failed to find any healthy peer, retrying: {}", e);
                        attempt += 1;
                        sleep(config.retry_delay).await;
                        continue;
                    }
                };

                match add_self_to_cluster(&config, &healthy_peer).await {
                    Ok(cluster) => {
                        info!("Joining existing cluster as learner via {}: {}", healthy_peer, cluster);
                        (cluster, "existing".to_string(), true)
                    }
                    Err(e) => {
                        warn!("Failed to add self as learner to cluster, retrying: {}", e);
                        attempt += 1;
                        sleep(config.retry_delay).await;
                        continue;
                    }
                }
            } else {
                (config.initial_cluster.clone(), "existing".to_string(), false)
            }
        };

        // Start etcd
        let mut child = start_etcd(&initial_cluster, &initial_cluster_state).await?;
        info!("Patroni started with PID {:?}", child.id());

        // Spawn monitor task
        let monitor_config = Config::from_env()?;
        let monitor_handle = tokio::spawn(async move {
            let _ = monitor_and_mark_bootstrap(&monitor_config, joined_as_learner).await;
        });

        // Wait for etcd to exit
        let status = child.wait().await?;

        // Stop monitor
        monitor_handle.abort();

        if status.success() {
            info!("etcd exited cleanly");
            return Ok(());
        }

        let exit_code = status.code().unwrap_or(-1);
        info!("etcd exited with code {}", exit_code);

        // Only clean data if bootstrap never completed
        let marker_path = config.bootstrap_marker();
        if !Path::new(&marker_path).exists() {
            let data_path = Path::new(&config.data_dir);
            if data_path.exists() {
                let has_data = fs::read_dir(data_path)
                    .await?
                    .next_entry()
                    .await?
                    .is_some();

                if has_data {
                    info!("Bootstrap incomplete - cleaning data directory...");
                    let mut entries = fs::read_dir(data_path).await?;
                    while let Some(entry) = entries.next_entry().await? {
                        let path = entry.path();
                        if path.is_dir() {
                            let _ = fs::remove_dir_all(&path).await;
                        } else {
                            let _ = fs::remove_file(&path).await;
                        }
                    }
                }
            }
        } else {
            info!("Bootstrap was complete - preserving data directory");
        }

        attempt += 1;
        if attempt <= config.max_retries {
            info!("Retrying in {:?}...", config.retry_delay);
            sleep(config.retry_delay).await;
        }
    }

    error!("Failed to start etcd after {} attempts", config.max_retries);
    std::process::exit(1);
}

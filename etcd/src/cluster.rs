//! etcd cluster operations
//!
//! Functions for managing cluster membership, starting etcd, and health checking.

use crate::config::{get_my_peer_url, parse_initial_cluster, Config};
use anyhow::{anyhow, Context, Result};
use common::{etcdctl, Telemetry, TelemetryEvent};
use std::path::Path;
use std::process::Stdio;
use tokio::fs;
use tokio::process::Command;
use tracing::{info, warn};

/// Information about an etcd cluster member
#[derive(Debug)]
pub struct MemberInfo {
    pub id: String,
    pub name: String,
    pub peer_url: String,
    pub is_learner: bool,
}

/// Get member list from etcd
pub async fn get_member_list(endpoint: &str) -> Result<Vec<MemberInfo>> {
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

/// Check cluster health via localhost or voting member
pub async fn check_cluster_health(initial_cluster: &str) -> bool {
    if etcdctl(&["endpoint", "health", "--endpoints=http://127.0.0.1:2379"])
        .await
        .is_ok()
    {
        return true;
    }

    if let Ok(Some(endpoint)) = get_voting_member_endpoint(initial_cluster).await {
        return etcdctl(&["endpoint", "health", &format!("--endpoints={}", endpoint)])
            .await
            .is_ok();
    }

    false
}

/// Find a voting member endpoint
pub async fn get_voting_member_endpoint(initial_cluster: &str) -> Result<Option<String>> {
    let cluster = parse_initial_cluster(initial_cluster)?;

    for (_name, peer_url) in cluster.iter() {
        let client_endpoint = peer_url.replace(":2380", ":2379");
        if etcdctl(&["member", "list", &format!("--endpoints={}", client_endpoint)])
            .await
            .is_ok()
        {
            return Ok(Some(client_endpoint));
        }
    }

    Ok(None)
}

/// Get my member ID from etcd cluster
pub async fn get_my_member_id(endpoint: &str, my_name: &str) -> Option<String> {
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
pub async fn is_learner(endpoint: &str, my_name: &str) -> bool {
    if let Ok(members) = get_member_list(endpoint).await {
        for member in members {
            if member.name == my_name {
                return member.is_learner;
            }
        }
    }
    false
}

/// Remove stale member entry for this node
pub async fn remove_stale_self(endpoint: &str, my_name: &str, my_peer_url: &str) -> Result<()> {
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

/// Build current cluster membership for joining node
pub async fn get_current_cluster(
    endpoint: &str,
    my_name: &str,
    my_peer_url: &str,
) -> Result<String> {
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

/// Add this node to an existing cluster as a learner
pub async fn add_self_to_cluster(
    config: &Config,
    leader: &str,
    leader_endpoint: &str,
) -> Result<String> {
    let my_peer_url = get_my_peer_url(&config.initial_cluster, &config.etcd_name)?
        .ok_or_else(|| anyhow!("Could not find my peer URL in ETCD_INITIAL_CLUSTER"))?;

    info!(node = %config.etcd_name, via = %leader_endpoint, "Adding self as learner");

    // Check if already a member
    let members = get_member_list(leader_endpoint).await?;
    for member in &members {
        if member.name == config.etcd_name || member.peer_url == my_peer_url {
            if !has_local_data(&config.data_dir).await {
                warn!("Registered as member but no local data - removing stale entry");
                remove_stale_self(leader_endpoint, &config.etcd_name, &my_peer_url).await?;

                // Clean partial data
                let _ = clear_directory(Path::new(&config.data_dir)).await;
                break;
            } else {
                info!("Already a member with local data");
                return get_current_cluster(leader_endpoint, &config.etcd_name, &my_peer_url).await;
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
        &format!("--endpoints={}", leader_endpoint),
    ])
    .await?;

    info!(via = %leader, "Successfully added as learner");

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
    get_current_cluster(leader_endpoint, &config.etcd_name, &my_peer_url).await
}

/// Promote self from learner to voting member
pub async fn promote_self(
    initial_cluster: &str,
    my_name: &str,
    telemetry: &Telemetry,
) -> Result<()> {
    let endpoint = get_voting_member_endpoint(initial_cluster)
        .await?
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

/// Check if we have valid local etcd data
pub async fn has_local_data(data_dir: &str) -> bool {
    let wal_dir = format!("{}/member/wal", data_dir);
    if let Ok(mut entries) = fs::read_dir(&wal_dir).await {
        if entries.next_entry().await.ok().flatten().is_some() {
            return true;
        }
    }
    false
}

/// Clear all contents of a directory without removing the directory itself
pub async fn clear_directory(path: &Path) -> Result<()> {
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

/// Start etcd process
pub async fn start_etcd(
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

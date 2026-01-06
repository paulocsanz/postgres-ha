//! Configuration for etcd bootstrap
//!
//! Handles environment variable parsing and validation.

use anyhow::Result;
use common::ConfigExt;
use std::collections::HashMap;
use std::time::Duration;

/// Configuration for etcd bootstrap process
pub struct Config {
    pub data_dir: String,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub peer_wait_timeout: Duration,
    pub peer_check_interval: Duration,
    pub etcd_name: String,
    pub initial_cluster: String,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
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

    /// Path to the bootstrap completion marker file
    pub fn bootstrap_marker(&self) -> String {
        format!("{}/.bootstrap_complete", self.data_dir)
    }
}

/// Parse the initial cluster string into a map of name -> peer_url
///
/// Format: "name1=http://host1:2380,name2=http://host2:2380"
pub fn parse_initial_cluster(cluster: &str) -> HashMap<String, String> {
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
pub fn get_bootstrap_leader(initial_cluster: &str) -> String {
    let cluster = parse_initial_cluster(initial_cluster);
    cluster.keys().min().cloned().unwrap_or_default()
}

/// Get leader's client endpoint (port 2379)
pub fn get_leader_endpoint(initial_cluster: &str, leader: &str) -> Option<String> {
    let cluster = parse_initial_cluster(initial_cluster);
    cluster.get(leader).map(|url| url.replace(":2380", ":2379"))
}

/// Get my peer URL from ETCD_INITIAL_CLUSTER
pub fn get_my_peer_url(initial_cluster: &str, etcd_name: &str) -> Option<String> {
    let cluster = parse_initial_cluster(initial_cluster);
    cluster.get(etcd_name).cloned()
}

//! HAProxy configuration from environment variables

use anyhow::{Context, Result};
use common::ConfigExt;

/// Configuration for HAProxy
pub struct Config {
    pub postgres_nodes: String,
    pub max_conn: String,
    pub timeout_connect: String,
    pub timeout_client: String,
    pub timeout_server: String,
    pub check_interval: String,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let postgres_nodes = String::env_required("POSTGRES_NODES").context(
            "POSTGRES_NODES is required.\n\
             Format: hostname:pgport:patroniport,hostname:pgport:patroniport,...\n\
             Example: postgres-1.railway.internal:5432:8008,postgres-2.railway.internal:5432:8008",
        )?;

        Ok(Self {
            postgres_nodes,
            max_conn: String::env_or("HAPROXY_MAX_CONN", "1000"),
            timeout_connect: String::env_or("HAPROXY_TIMEOUT_CONNECT", "10s"),
            timeout_client: String::env_or("HAPROXY_TIMEOUT_CLIENT", "30m"),
            timeout_server: String::env_or("HAPROXY_TIMEOUT_SERVER", "30m"),
            check_interval: String::env_or("HAPROXY_CHECK_INTERVAL", "3s"),
        })
    }
}

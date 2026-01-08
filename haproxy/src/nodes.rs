//! PostgreSQL node parsing

use anyhow::{anyhow, Result};

/// PostgreSQL node information
#[derive(Debug)]
pub struct PostgresNode {
    pub name: String,
    pub host: String,
    pub pg_port: String,
    pub patroni_port: String,
}

/// Parse nodes from the POSTGRES_NODES environment variable
///
/// Format: "hostname:pgport:patroniport,hostname:pgport:patroniport,..."
pub fn parse_nodes(postgres_nodes: &str) -> Result<Vec<PostgresNode>> {
    postgres_nodes
        .split(',')
        .map(|node| {
            let parts: Vec<&str> = node.split(':').collect();
            if parts.len() != 3 {
                return Err(anyhow!(
                    "Invalid node format: {}. Expected: hostname:pgport:patroniport",
                    node
                ));
            }

            let host = parts[0].to_string();
            let name = host.split('.').next().unwrap_or(&host).to_string();

            Ok(PostgresNode {
                name,
                host,
                pg_port: parts[1].to_string(),
                patroni_port: parts[2].to_string(),
            })
        })
        .collect()
}

//! HAProxy configuration generator and launcher
//!
//! Generates HAProxy configuration dynamically from PostgreSQL node information
//! in environment variables. Supports single-node and multi-node modes with
//! TCP/HTTP health checks via Patroni.

use anyhow::{anyhow, Context, Result};
use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::Command;
use tracing::info;

const CONFIG_FILE: &str = "/usr/local/etc/haproxy/haproxy.cfg";

struct Config {
    postgres_nodes: String,
    max_conn: String,
    timeout_connect: String,
    timeout_client: String,
    timeout_server: String,
    check_interval: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let postgres_nodes =
            env::var("POSTGRES_NODES").context("POSTGRES_NODES is required.\nFormat: hostname:pgport:patroniport,hostname:pgport:patroniport,...\nExample: postgres-1.railway.internal:5432:8008,postgres-2.railway.internal:5432:8008")?;

        Ok(Self {
            postgres_nodes,
            max_conn: env::var("HAPROXY_MAX_CONN").unwrap_or_else(|_| "1000".to_string()),
            timeout_connect: env::var("HAPROXY_TIMEOUT_CONNECT")
                .unwrap_or_else(|_| "10s".to_string()),
            timeout_client: env::var("HAPROXY_TIMEOUT_CLIENT")
                .unwrap_or_else(|_| "30m".to_string()),
            timeout_server: env::var("HAPROXY_TIMEOUT_SERVER")
                .unwrap_or_else(|_| "30m".to_string()),
            check_interval: env::var("HAPROXY_CHECK_INTERVAL")
                .unwrap_or_else(|_| "3s".to_string()),
        })
    }
}

#[derive(Debug)]
struct PostgresNode {
    name: String,
    host: String,
    pg_port: String,
    patroni_port: String,
}

fn parse_nodes(postgres_nodes: &str) -> Result<Vec<PostgresNode>> {
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
            // Extract short name from hostname (e.g., postgres-1 from postgres-1.railway.internal)
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

fn generate_server_entries(nodes: &[PostgresNode], single_node_mode: bool) -> String {
    nodes
        .iter()
        .map(|node| {
            if single_node_mode {
                // Single node: skip Patroni health check, use TCP check on PostgreSQL port
                format!(
                    "    server {} {}:{} check resolvers railway resolve-prefer ipv4",
                    node.name, node.host, node.pg_port
                )
            } else {
                // Multi-node: use Patroni health check
                format!(
                    "    server {} {}:{} check port {} resolvers railway resolve-prefer ipv4",
                    node.name, node.host, node.pg_port, node.patroni_port
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn generate_config(config: &Config) -> Result<String> {
    let nodes = parse_nodes(&config.postgres_nodes)?;
    let node_count = nodes.len();
    let single_node_mode = node_count == 1;

    if single_node_mode {
        info!("Single node mode: HAProxy will route directly to PostgreSQL without Patroni health checks");
    }

    let server_entries = generate_server_entries(&nodes, single_node_mode);

    let primary_backend = if single_node_mode {
        format!(
            r#"backend postgresql_primary_backend
    default-server inter {} fall 3 rise 2 on-marked-down shutdown-sessions
{}"#,
            config.check_interval, server_entries
        )
    } else {
        format!(
            r#"backend postgresql_primary_backend
    option httpchk
    http-check send meth GET uri /primary
    http-check expect status 200
    default-server inter {} fall 3 rise 2 on-marked-down shutdown-sessions
{}"#,
            config.check_interval, server_entries
        )
    };

    let replica_backend = if single_node_mode {
        format!(
            r#"backend postgresql_replicas_backend
    balance roundrobin
    default-server inter {} fall 3 rise 2 on-marked-down shutdown-sessions
{}"#,
            config.check_interval, server_entries
        )
    } else {
        format!(
            r#"backend postgresql_replicas_backend
    balance roundrobin
    option httpchk
    http-check send meth GET uri /replica
    http-check expect status 200
    default-server inter {} fall 3 rise 2 on-marked-down shutdown-sessions
{}"#,
            config.check_interval, server_entries
        )
    };

    Ok(format!(
        r#"global
    maxconn {}
    log stdout format raw local0

defaults
    log global
    mode tcp
    retries 3
    timeout connect {}
    timeout client {}
    timeout server {}
    timeout check 5s

resolvers railway
    parse-resolv-conf
    resolve_retries 3
    timeout resolve 1s
    timeout retry   1s
    hold other      10s
    hold refused    10s
    hold nx         10s
    hold timeout    10s
    hold valid      10s
    hold obsolete   10s

# Stats page for monitoring
listen stats
    bind *:8404
    mode http
    stats enable
    stats uri /stats
    stats refresh 10s

# Primary PostgreSQL (read-write)
frontend postgresql_primary
    bind *:5432
    default_backend postgresql_primary_backend

{}

# Replica PostgreSQL (read-only)
frontend postgresql_replicas
    bind *:5433
    default_backend postgresql_replicas_backend

{}
"#,
        config.max_conn,
        config.timeout_connect,
        config.timeout_client,
        config.timeout_server,
        primary_backend,
        replica_backend
    ))
}

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    let config = Config::from_env()?;

    info!("Generating HAProxy config for nodes: {}", config.postgres_nodes);

    let haproxy_config = generate_config(&config)?;

    // Write config file
    fs::write(CONFIG_FILE, &haproxy_config).context("Failed to write HAProxy config")?;

    info!("HAProxy config written to {}", CONFIG_FILE);

    // Log config using info! to avoid stdout/stderr interleaving in container logs
    for line in haproxy_config.lines() {
        info!("  {}", line);
    }

    info!("Starting HAProxy...");

    // exec haproxy (replaces current process)
    let err = Command::new("haproxy").arg("-f").arg(CONFIG_FILE).exec();

    // exec only returns if there was an error
    Err(anyhow!("Failed to exec haproxy: {}", err))
}

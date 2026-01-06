//! HAProxy configuration template generation

use crate::config::Config;
use crate::nodes::PostgresNode;

/// Generate server entries for backend configuration
fn generate_server_entries(nodes: &[PostgresNode], single_node_mode: bool) -> String {
    nodes
        .iter()
        .map(|node| {
            if single_node_mode {
                format!(
                    "    server {} {}:{} check resolvers railway resolve-prefer ipv4",
                    node.name, node.host, node.pg_port
                )
            } else {
                format!(
                    "    server {} {}:{} check port {} resolvers railway resolve-prefer ipv4",
                    node.name, node.host, node.pg_port, node.patroni_port
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate primary backend configuration
fn generate_primary_backend(
    config: &Config,
    server_entries: &str,
    single_node_mode: bool,
) -> String {
    if single_node_mode {
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
    }
}

/// Generate replica backend configuration
fn generate_replica_backend(
    config: &Config,
    server_entries: &str,
    single_node_mode: bool,
) -> String {
    if single_node_mode {
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
    }
}

/// Generate complete HAProxy configuration
pub fn generate_config(config: &Config, nodes: &[PostgresNode]) -> String {
    let single_node_mode = nodes.len() == 1;
    let server_entries = generate_server_entries(nodes, single_node_mode);
    let primary_backend = generate_primary_backend(config, &server_entries, single_node_mode);
    let replica_backend = generate_replica_backend(config, &server_entries, single_node_mode);

    format!(
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
    )
}

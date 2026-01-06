//! HAProxy configuration generator and launcher
//!
//! Generates HAProxy configuration dynamically from PostgreSQL node information
//! in environment variables. Supports single-node and multi-node modes with
//! TCP/HTTP health checks via Patroni.

mod config;
mod nodes;
mod template;

use anyhow::{anyhow, Context, Result};
use common::{init_logging, Telemetry, TelemetryEvent};
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::Command;
use tracing::info;

use config::Config;
use nodes::parse_nodes;
use template::generate_config;

const CONFIG_FILE: &str = "/usr/local/etc/haproxy/haproxy.cfg";

fn main() -> Result<()> {
    let _guard = init_logging("haproxy");

    let telemetry = Telemetry::from_env("haproxy");
    let config = Config::from_env()?;
    let nodes = parse_nodes(&config.postgres_nodes)?;
    let single_node_mode = nodes.len() == 1;

    info!(
        nodes = %config.postgres_nodes,
        count = nodes.len(),
        "Generating HAProxy config"
    );

    if single_node_mode {
        info!("Single node mode: routing directly without Patroni health checks");
    }

    telemetry.send(TelemetryEvent::HaproxyConfigGenerating {
        nodes: nodes.iter().map(|n| n.name.clone()).collect(),
    });

    let haproxy_config = generate_config(&config, &nodes);

    fs::write(CONFIG_FILE, &haproxy_config).context("Failed to write HAProxy config")?;
    info!(path = CONFIG_FILE, "Config written");

    // Log config for debugging
    for line in haproxy_config.lines() {
        info!("  {}", line);
    }

    telemetry.send(TelemetryEvent::HaproxyStarted {
        node_count: nodes.len(),
        single_node_mode,
    });

    info!("Starting HAProxy...");

    // exec haproxy (replaces current process)
    let err = Command::new("haproxy").arg("-f").arg(CONFIG_FILE).exec();

    Err(anyhow!("Failed to exec haproxy: {}", err))
}

//! Patroni runner - Wrapper to run Patroni with proper setup
//!
//! This script generates the Patroni configuration and starts Patroni.
//! Runs as PID 1 in container with built-in health monitoring.
//! If Patroni dies or becomes unresponsive, exits to trigger container restart.

use anyhow::{anyhow, Context, Result};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use postgres_patroni::{pgdata, ssl_dir, volume_root};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::signal::unix::{signal, SignalKind};
use tokio::time::sleep;
use tracing::{error, info, warn};

struct Config {
    scope: String,
    name: String,
    connect_address: String,
    etcd_hosts: String,
    superuser: String,
    superuser_pass: String,
    repl_user: String,
    repl_pass: String,
    app_user: String,
    app_pass: String,
    app_db: String,
    data_dir: String,
    certs_dir: String,
    ttl: String,
    loop_wait: String,
    retry_timeout: String,
    health_check_interval: u64,
    health_check_timeout: u64,
    max_failures: u32,
    startup_grace_period: u64,
    adopt_existing_data: bool,
}

impl Config {
    fn from_env() -> Result<Self> {
        let name = env::var("PATRONI_NAME").context("PATRONI_NAME must be set")?;
        let connect_address =
            env::var("RAILWAY_PRIVATE_DOMAIN").context("RAILWAY_PRIVATE_DOMAIN must be set")?;
        let etcd_hosts =
            env::var("PATRONI_ETCD3_HOSTS").context("PATRONI_ETCD3_HOSTS must be set")?;

        Ok(Self {
            scope: env::var("PATRONI_SCOPE").unwrap_or_else(|_| "railway-pg-ha".to_string()),
            name,
            connect_address,
            etcd_hosts,
            superuser: env::var("PATRONI_SUPERUSER_USERNAME")
                .unwrap_or_else(|_| "postgres".to_string()),
            superuser_pass: env::var("PATRONI_SUPERUSER_PASSWORD").unwrap_or_default(),
            repl_user: env::var("PATRONI_REPLICATION_USERNAME")
                .unwrap_or_else(|_| "replicator".to_string()),
            repl_pass: env::var("PATRONI_REPLICATION_PASSWORD").unwrap_or_default(),
            app_user: env::var("POSTGRES_USER").unwrap_or_else(|_| "postgres".to_string()),
            app_pass: env::var("POSTGRES_PASSWORD").unwrap_or_default(),
            app_db: env::var("POSTGRES_DB")
                .or_else(|_| env::var("PGDATABASE"))
                .unwrap_or_else(|_| "railway".to_string()),
            data_dir: pgdata(),
            certs_dir: ssl_dir(),
            // Constraint: loop_wait + 2*retry_timeout <= ttl
            // Default: 10 + 2*10 = 30 <= 30 ✓
            ttl: env::var("PATRONI_TTL").unwrap_or_else(|_| "30".to_string()),
            loop_wait: env::var("PATRONI_LOOP_WAIT").unwrap_or_else(|_| "10".to_string()),
            retry_timeout: env::var("PATRONI_RETRY_TIMEOUT").unwrap_or_else(|_| "10".to_string()),
            health_check_interval: env::var("PATRONI_HEALTH_CHECK_INTERVAL")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .unwrap_or(5),
            health_check_timeout: env::var("PATRONI_HEALTH_CHECK_TIMEOUT")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .unwrap_or(5),
            max_failures: env::var("PATRONI_MAX_HEALTH_FAILURES")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(3),
            startup_grace_period: env::var("PATRONI_STARTUP_GRACE_PERIOD")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .unwrap_or(60),
            adopt_existing_data: env::var("PATRONI_ADOPT_EXISTING_DATA")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
        })
    }
}

fn generate_patroni_config(config: &Config) -> String {
    format!(
        r#"scope: {scope}
name: {name}

restapi:
  listen: 0.0.0.0:8008
  connect_address: {connect_address}:8008

etcd3:
  hosts: {etcd_hosts}

bootstrap:
  dcs:
    ttl: {ttl}
    loop_wait: {loop_wait}
    retry_timeout: {retry_timeout}
    maximum_lag_on_failover: 1048576
    failsafe_mode: true
    postgresql:
      use_pg_rewind: true
      use_slots: true
      parameters:
        wal_level: replica
        hot_standby: "on"
        max_wal_senders: 10
        max_replication_slots: 10
        max_connections: 200
        password_encryption: scram-sha-256

  initdb:
    - encoding: UTF8
    - data-checksums
    - username: {superuser}

  pg_hba:
    - local all all trust
    - hostssl replication {repl_user} 0.0.0.0/0 scram-sha-256
    - hostssl replication {repl_user} ::/0 scram-sha-256
    - hostssl all all 0.0.0.0/0 scram-sha-256
    - hostssl all all ::/0 scram-sha-256
    - host replication {repl_user} 0.0.0.0/0 scram-sha-256
    - host replication {repl_user} ::/0 scram-sha-256
    - host all all 0.0.0.0/0 scram-sha-256
    - host all all ::/0 scram-sha-256

  post_bootstrap: /post_bootstrap.sh

postgresql:
  listen: "*:5432"
  connect_address: {connect_address}:5432
  data_dir: {data_dir}
  pgpass: /tmp/pgpass
  callbacks:
    on_role_change: /on_role_change.sh
  remove_data_directory_on_rewind_failure: true
  remove_data_directory_on_diverged_timelines: true
  create_replica_methods:
    - basebackup
  basebackup:
    checkpoint: "fast"
    wal-method: "stream"
  authentication:
    replication:
      username: "{repl_user}"
      password: "{repl_pass}"
    superuser:
      username: "{superuser}"
      password: "{superuser_pass}"
  app_user:
    username: "{app_user}"
    password: "{app_pass}"
    database: "{app_db}"
  parameters:
    unix_socket_directories: /var/run/postgresql
    ssl: "on"
    ssl_cert_file: "{certs_dir}/server.crt"
    ssl_key_file: "{certs_dir}/server.key"
    ssl_ca_file: "{certs_dir}/root.crt"
"#,
        scope = config.scope,
        name = config.name,
        connect_address = config.connect_address,
        etcd_hosts = config.etcd_hosts,
        ttl = config.ttl,
        loop_wait = config.loop_wait,
        retry_timeout = config.retry_timeout,
        superuser = config.superuser,
        superuser_pass = config.superuser_pass,
        repl_user = config.repl_user,
        repl_pass = config.repl_pass,
        app_user = config.app_user,
        app_pass = config.app_pass,
        app_db = config.app_db,
        data_dir = config.data_dir,
        certs_dir = config.certs_dir,
    )
}

fn update_pg_hba_for_replication(config: &Config) -> Result<()> {
    let pg_hba_path = format!("{}/pg_hba.conf", config.data_dir);

    if !Path::new(&pg_hba_path).exists() {
        return Ok(());
    }

    info!(
        "Checking pg_hba.conf for replication support (user: {})...",
        config.repl_user
    );

    let content = fs::read_to_string(&pg_hba_path)?;

    // Check if replication entries exist for our specific user
    if content.contains(&format!("replication {}", config.repl_user))
        || content.contains(&format!("replication\t{}", config.repl_user))
    {
        info!(
            "Replication entries for {} already exist in pg_hba.conf",
            config.repl_user
        );
        return Ok(());
    }

    info!(
        "Adding replication entries for user {} to pg_hba.conf...",
        config.repl_user
    );

    let new_entries = format!(
        r#"# Replication entries added by Patroni migration for user {}
hostssl replication {} 0.0.0.0/0 scram-sha-256
hostssl replication {} ::/0 scram-sha-256
host replication {} 0.0.0.0/0 scram-sha-256
host replication {} ::/0 scram-sha-256

"#,
        config.repl_user,
        config.repl_user,
        config.repl_user,
        config.repl_user,
        config.repl_user
    );

    let new_content = format!("{}{}", new_entries, content);
    fs::write(&pg_hba_path, new_content)?;

    // Set permissions
    let perms = std::fs::Permissions::from_mode(0o600);
    fs::set_permissions(&pg_hba_path, perms)?;

    info!(
        "pg_hba.conf updated with replication entries for {}",
        config.repl_user
    );

    Ok(())
}

async fn check_health(timeout_secs: u64) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    client
        .get("http://localhost:8008/health")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn start_patroni() -> Result<Child> {
    let child = Command::new("patroni")
        .arg("/tmp/patroni.yml")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to start patroni")?;

    Ok(child)
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

    info!("=== Patroni Runner ===");

    let config = Config::from_env()?;

    info!(
        "Node: {} (address: {})",
        config.name, config.connect_address
    );

    let volume_root = volume_root();
    let bootstrap_marker = format!("{}/.patroni_bootstrap_complete", volume_root);

    // Update pg_hba.conf for replication if adopting existing data
    if config.adopt_existing_data {
        update_pg_hba_for_replication(&config)?;
    }

    // Check for valid data
    let pg_control_path = format!("{}/global/pg_control", config.data_dir);
    let has_pg_control = Path::new(&pg_control_path).exists();
    let has_marker = Path::new(&bootstrap_marker).exists();

    if config.adopt_existing_data && has_pg_control && !has_marker {
        info!("PATRONI_ADOPT_EXISTING_DATA=true - migrating from vanilla PostgreSQL");
        info!("Preserving existing data and adopting into Patroni cluster");
        fs::write(&bootstrap_marker, "").context("Failed to create bootstrap marker")?;
    } else if has_pg_control && has_marker {
        info!("Found valid data with bootstrap marker");
    } else if has_pg_control {
        info!("Found pg_control but NO bootstrap marker - stale data from failed bootstrap");
    } else {
        info!("No PostgreSQL data found");
    }

    // Generate Patroni configuration
    let patroni_config = generate_patroni_config(&config);
    fs::write("/tmp/patroni.yml", &patroni_config).context("Failed to write patroni.yml")?;

    info!(
        "Starting Patroni (scope: {}, etcd: {})",
        config.scope, config.etcd_hosts
    );

    // Ensure data directory has correct permissions
    fs::create_dir_all(&config.data_dir).ok();
    let perms = std::fs::Permissions::from_mode(0o700);
    fs::set_permissions(&config.data_dir, perms).ok();

    // Unset PG* environment variables
    env::remove_var("PGPASSWORD");
    env::remove_var("PGUSER");
    env::remove_var("PGHOST");
    env::remove_var("PGPORT");
    env::remove_var("PGDATABASE");

    // Start Patroni
    let mut child = start_patroni().await?;
    let patroni_pid = child.id().ok_or_else(|| anyhow!("Failed to get Patroni PID"))?;
    info!("Patroni started with PID {}", patroni_pid);

    // Set up signal handlers
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    // Wait for startup grace period
    info!(
        "Waiting {}s for Patroni to initialize...",
        config.startup_grace_period
    );

    let mut startup_elapsed = 0u64;
    while startup_elapsed < config.startup_grace_period {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM during startup, stopping Patroni...");
                let _ = kill(Pid::from_raw(patroni_pid as i32), Signal::SIGTERM);
                let _ = child.wait().await;
                return Ok(());
            }
            _ = sigint.recv() => {
                info!("Received SIGINT during startup, stopping Patroni...");
                let _ = kill(Pid::from_raw(patroni_pid as i32), Signal::SIGTERM);
                let _ = child.wait().await;
                return Ok(());
            }
            _status = child.wait() => {
                error!("Patroni process died during startup");
                std::process::exit(1);
            }
            _ = sleep(Duration::from_secs(5)) => {
                startup_elapsed += 5;

                // Try health check early
                if check_health(config.health_check_timeout).await {
                    info!("Patroni healthy after {}s, starting health monitoring", startup_elapsed);
                    break;
                }
            }
        }
    }

    // Main health monitoring loop
    let mut failures = 0u32;
    info!(
        "Health monitoring active (interval={}s, max_failures={})",
        config.health_check_interval, config.max_failures
    );

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM, stopping Patroni...");
                let _ = kill(Pid::from_raw(patroni_pid as i32), Signal::SIGTERM);
                let _ = child.wait().await;
                return Ok(());
            }
            _ = sigint.recv() => {
                info!("Received SIGINT, stopping Patroni...");
                let _ = kill(Pid::from_raw(patroni_pid as i32), Signal::SIGTERM);
                let _ = child.wait().await;
                return Ok(());
            }
            _status = child.wait() => {
                error!("Patroni process died unexpectedly");
                std::process::exit(1);
            }
            _ = sleep(Duration::from_secs(config.health_check_interval)) => {
                if check_health(config.health_check_timeout).await {
                    if failures > 0 {
                        info!("Patroni recovered after {} failed health checks", failures);
                    }
                    failures = 0;
                } else {
                    failures += 1;
                    warn!("Health check failed ({}/{})", failures, config.max_failures);

                    if failures >= config.max_failures {
                        error!("CRITICAL: Patroni unresponsive after {} checks - exiting to trigger restart", config.max_failures);
                        let _ = kill(Pid::from_raw(patroni_pid as i32), Signal::SIGTERM);
                        sleep(Duration::from_secs(2)).await;
                        let _ = kill(Pid::from_raw(patroni_pid as i32), Signal::SIGKILL);
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}

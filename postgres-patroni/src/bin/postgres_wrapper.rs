//! Wrapper script for Patroni-enabled PostgreSQL startup
//!
//! Validates volume mounts, checks PGDATA configuration, generates SSL certificates
//! if missing or expired, handles permission setup, and decides between Patroni HA
//! mode or standalone PostgreSQL mode based on the PATRONI_ENABLED flag.

use anyhow::{anyhow, Context, Result};
use postgres_patroni::{
    cert_expires_within, is_patroni_enabled, is_railway, is_valid_x509v3_cert, pgdata, ssl_dir,
    sudo_command, EXPECTED_VOLUME_MOUNT_PATH,
};
use std::env;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{error, info};

const INIT_SSL_SCRIPT: &str = "/docker-entrypoint-initdb.d/init-ssl.sh";

async fn run_init_ssl() -> Result<()> {
    let status = tokio::process::Command::new("bash")
        .arg(INIT_SSL_SCRIPT)
        .status()
        .await
        .context("Failed to run init-ssl script")?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("init-ssl script failed"))
    }
}

async fn check_and_generate_ssl() -> Result<()> {
    let ssl_dir = ssl_dir();
    let server_crt = format!("{}/server.crt", ssl_dir);

    if !Path::new(&server_crt).exists() {
        info!("SSL certificates missing, generating...");
        run_init_ssl().await?;
        return Ok(());
    }

    // Check/renew existing SSL certs
    // Regenerate if the certificate is not a x509v3 certificate
    let is_valid = timeout(Duration::from_secs(30), async {
        is_valid_x509v3_cert(&server_crt).await
    })
    .await
    .unwrap_or(false);

    if !is_valid {
        info!("Did not find a x509v3 certificate, regenerating certificates...");
        run_init_ssl().await?;
        return Ok(());
    }

    // Regenerate if the certificate has expired or will expire (30 days)
    let expires_soon = timeout(Duration::from_secs(30), async {
        cert_expires_within(&server_crt, 2592000).await // 30 days in seconds
    })
    .await
    .unwrap_or(true);

    if expires_soon {
        info!("Certificate has or will expire soon, regenerating certificates...");
        run_init_ssl().await?;
    }

    Ok(())
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

    let pgdata = pgdata();
    let data_dir = EXPECTED_VOLUME_MOUNT_PATH;

    // Check if the Railway volume is mounted to the correct path
    if is_railway() {
        let volume_mount_path =
            env::var("RAILWAY_VOLUME_MOUNT_PATH").unwrap_or_else(|_| String::new());

        if volume_mount_path != EXPECTED_VOLUME_MOUNT_PATH {
            error!(
                "Railway volume not mounted to the correct path, expected {} but got {}",
                EXPECTED_VOLUME_MOUNT_PATH, volume_mount_path
            );
            error!("Please update the volume mount path to the expected path and redeploy the service");
            std::process::exit(1);
        }
    }

    // Check if PGDATA starts with the expected volume mount path
    if !pgdata.starts_with(EXPECTED_VOLUME_MOUNT_PATH) {
        error!(
            "PGDATA variable does not start with the expected volume mount path, expected to start with {}",
            EXPECTED_VOLUME_MOUNT_PATH
        );
        error!(
            "Please update the PGDATA variable to start with the expected volume mount path and redeploy the service"
        );
        std::process::exit(1);
    }

    let postgres_conf_file = format!("{}/postgresql.conf", pgdata);

    if is_patroni_enabled() {
        info!("=== Patroni mode enabled ===");

        // Ensure data directory exists and has correct permissions (Railway mounts as root)
        if !Path::new(data_dir).exists() {
            info!("Creating data directory...");
            sudo_command(&["mkdir", "-p", data_dir]).await?;
        }

        // Recursive chown - handles leftover root-owned files from failed bootstraps
        info!("Setting data directory ownership...");
        let chown_result = timeout(
            Duration::from_secs(120),
            sudo_command(&["chown", "-R", "postgres:postgres", data_dir]),
        )
        .await;

        match chown_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!("Failed to set ownership: {}", e);
                std::process::exit(1);
            }
            Err(_) => {
                error!("ERROR: chown timed out after 120s - volume may have issues");
                std::process::exit(1);
            }
        }

        sudo_command(&["chmod", "700", data_dir]).await?;

        // Check for required passwords on fresh installs
        let pg_version_file = format!("{}/PG_VERSION", data_dir);
        if !Path::new(&pg_version_file).exists() {
            if env::var("POSTGRES_PASSWORD").is_err() || env::var("POSTGRES_PASSWORD")?.is_empty() {
                error!("ERROR: POSTGRES_PASSWORD is required for new database initialization.");
                std::process::exit(1);
            }
            if env::var("PATRONI_REPLICATION_PASSWORD").is_err()
                || env::var("PATRONI_REPLICATION_PASSWORD")?.is_empty()
            {
                error!("ERROR: PATRONI_REPLICATION_PASSWORD is required for HA mode.");
                std::process::exit(1);
            }
        }

        // Generate SSL certs if missing (replicas need this - they don't run post_bootstrap)
        check_and_generate_ssl().await?;

        // Run Patroni as postgres user (initdb refuses to run as root)
        info!("Starting Patroni runner...");
        let err = Command::new("gosu")
            .args(["postgres", "/usr/local/bin/patroni-runner.sh"])
            .exec();

        // exec only returns if there was an error
        Err(anyhow!("Failed to exec patroni-runner: {}", err))
    } else {
        // === Standalone PostgreSQL mode (matches postgres-ssl behavior) ===
        let ssl_dir = ssl_dir();
        let server_crt = format!("{}/server.crt", ssl_dir);

        // Regenerate if the certificate is not a x509v3 certificate
        if Path::new(&server_crt).exists() && !is_valid_x509v3_cert(&server_crt).await {
            info!("Did not find a x509v3 certificate, regenerating certificates...");
            run_init_ssl().await?;
        }

        // Regenerate if the certificate has expired or will expire (30 days)
        if Path::new(&server_crt).exists() && cert_expires_within(&server_crt, 2592000).await {
            info!("Certificate has or will expire soon, regenerating certificates...");
            run_init_ssl().await?;
        }

        // Generate a certificate if the database was initialized but is missing a certificate
        if Path::new(&postgres_conf_file).exists() && !Path::new(&server_crt).exists() {
            info!("Database initialized without certificate, generating certificates...");
            run_init_ssl().await?;
        }

        // Unset PGHOST to force psql to use Unix socket path
        env::remove_var("PGHOST");
        // Unset PGPORT also since postgres checks for validity
        env::remove_var("PGPORT");

        // Get command line args to pass to docker-entrypoint.sh
        let args: Vec<String> = env::args().skip(1).collect();
        let log_to_stdout = env::var("LOG_TO_STDOUT")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);

        info!("Starting standalone PostgreSQL...");

        let mut cmd = Command::new("/usr/local/bin/docker-entrypoint.sh");
        cmd.args(&args);

        if log_to_stdout {
            cmd.stderr(Stdio::inherit());
        }

        let err = cmd.exec();

        // exec only returns if there was an error
        Err(anyhow!("Failed to exec docker-entrypoint.sh: {}", err))
    }
}

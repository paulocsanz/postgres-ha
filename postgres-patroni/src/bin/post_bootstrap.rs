//! Patroni post-bootstrap script
//!
//! Runs ONCE after PostgreSQL initialization on the primary node.
//! IMPORTANT: Patroni runs this as a subprocess WITHOUT environment variables.
//! We MUST read credentials from /tmp/patroni.yml

use anyhow::{anyhow, Context, Result};
use common::{init_logging, Telemetry, TelemetryEvent};
use postgres_patroni::{extract_yaml_value, parse_yaml_value, volume_root};
use std::env;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;
use tracing::{error, info};

const PATRONI_CONFIG: &str = "/tmp/patroni.yml";

struct Credentials {
    repl_user: String,
    repl_pass: String,
    superuser: String,
    superuser_pass: String,
    app_user: String,
    app_pass: String,
    app_db: String,
}

fn read_credentials() -> Result<Credentials> {
    let content =
        std::fs::read_to_string(PATRONI_CONFIG).context("Failed to read Patroni config")?;

    let repl_user = extract_nested_value(&content, "authentication", "replication", "username")
        .ok_or_else(|| anyhow!("Could not extract replication username"))?;
    let repl_pass = extract_nested_value(&content, "authentication", "replication", "password")
        .ok_or_else(|| anyhow!("Could not extract replication password"))?;
    let superuser = extract_nested_value(&content, "authentication", "superuser", "username")
        .ok_or_else(|| anyhow!("Could not extract superuser username"))?;
    let superuser_pass = extract_nested_value(&content, "authentication", "superuser", "password")
        .ok_or_else(|| anyhow!("Could not extract superuser password"))?;

    let app_user = extract_yaml_value(&content, "app_user", "username").unwrap_or_default();
    let app_pass = extract_yaml_value(&content, "app_user", "password").unwrap_or_default();
    let app_db = extract_yaml_value(&content, "app_user", "database").unwrap_or_default();

    Ok(Credentials {
        repl_user,
        repl_pass,
        superuser,
        superuser_pass,
        app_user,
        app_pass,
        app_db,
    })
}

fn extract_nested_value(
    content: &str,
    section1: &str,
    section2: &str,
    key: &str,
) -> Option<String> {
    let mut in_section1 = false;
    let mut in_section2 = false;
    let mut section1_indent = 0;
    let mut section2_indent = 0;

    for line in content.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if trimmed.starts_with(&format!("{}:", section1)) {
            in_section1 = true;
            section1_indent = indent;
            continue;
        }

        if in_section1 {
            if !trimmed.is_empty() && indent <= section1_indent && !trimmed.starts_with('#') {
                in_section1 = false;
                in_section2 = false;
                continue;
            }

            if trimmed.starts_with(&format!("{}:", section2)) {
                in_section2 = true;
                section2_indent = indent;
                continue;
            }

            if in_section2 {
                if !trimmed.is_empty() && indent <= section2_indent && !trimmed.starts_with('#') {
                    in_section2 = false;
                    continue;
                }

                if trimmed.starts_with(&format!("{}:", key)) {
                    return parse_yaml_value(trimmed);
                }
            }
        }
    }

    None
}

fn run_psql(superuser: &str, sql: &str) -> Result<String> {
    let output = Command::new("env")
        .args(["-i"])
        .env("PATH", env::var("PATH").unwrap_or_default())
        .args([
            "psql",
            "-v",
            "ON_ERROR_STOP=1",
            "-h",
            "/var/run/postgresql",
            "-U",
            superuser,
            "-d",
            "postgres",
            "-c",
            sql,
        ])
        .stdin(Stdio::null())
        .output()
        .context("Failed to run psql")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow!(
            "psql failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn run_psql_script(superuser: &str, sql: &str) -> Result<String> {
    let mut child = Command::new("env")
        .args(["-i"])
        .env("PATH", env::var("PATH").unwrap_or_default())
        .args([
            "psql",
            "-v",
            "ON_ERROR_STOP=1",
            "-h",
            "/var/run/postgresql",
            "-U",
            superuser,
            "-d",
            "postgres",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn psql")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(sql.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow!(
            "psql failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn main() -> Result<()> {
    let _guard = init_logging("post-bootstrap");

    let start = Instant::now();
    let telemetry = Telemetry::from_env("postgres-ha");
    let node_name = env::var("PATRONI_NAME").unwrap_or_else(|_| "unknown".to_string());

    info!("Post-bootstrap starting...");

    telemetry.send(TelemetryEvent::BootstrapStarted {
        node: node_name.clone(),
        is_fresh: true,
    });

    if !Path::new(PATRONI_CONFIG).exists() {
        error!(path = PATRONI_CONFIG, "Patroni config not found");
        telemetry.send(TelemetryEvent::BootstrapFailed {
            node: node_name,
            error: "Patroni config not found".to_string(),
            phase: "read_config".to_string(),
        });
        std::process::exit(1);
    }

    let creds = match read_credentials() {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to read credentials");
            telemetry.send(TelemetryEvent::BootstrapFailed {
                node: node_name,
                error: e.to_string(),
                phase: "read_credentials".to_string(),
            });
            std::process::exit(1);
        }
    };

    if creds.repl_user.is_empty() || creds.repl_pass.is_empty() {
        error!("Missing replication credentials");
        std::process::exit(1);
    }

    if creds.superuser.is_empty() {
        error!("Missing superuser");
        std::process::exit(1);
    }

    info!(superuser = %creds.superuser, "Setting up users");

    let sql = format!(
        r#"
SET password_encryption = 'scram-sha-256';

DO $$
BEGIN
    EXECUTE format('ALTER ROLE %I WITH PASSWORD %L', '{superuser}', '{superuser_pass}');
    RAISE NOTICE 'Set password for superuser: {superuser}';
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{repl_user}') THEN
        EXECUTE format('CREATE ROLE %I WITH REPLICATION LOGIN PASSWORD %L', '{repl_user}', '{repl_pass}');
        RAISE NOTICE 'Created replication user: {repl_user}';
    ELSE
        EXECUTE format('ALTER ROLE %I WITH REPLICATION LOGIN PASSWORD %L', '{repl_user}', '{repl_pass}');
        RAISE NOTICE 'Updated replication user: {repl_user}';
    END IF;
END
$$;

DO $$
BEGIN
    IF '{app_user}' = '{superuser}' THEN
        RAISE NOTICE 'App user same as superuser, skipping';
    ELSIF '{app_user}' = '' OR '{app_pass}' = '' THEN
        RAISE NOTICE 'App user not configured, skipping';
    ELSIF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{app_user}') THEN
        EXECUTE format('CREATE ROLE %I WITH LOGIN PASSWORD %L', '{app_user}', '{app_pass}');
        RAISE NOTICE 'Created app user: {app_user}';
    ELSE
        EXECUTE format('ALTER ROLE %I WITH PASSWORD %L', '{app_user}', '{app_pass}');
        RAISE NOTICE 'Updated app user: {app_user}';
    END IF;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'postgres') THEN
        EXECUTE format('CREATE ROLE postgres WITH SUPERUSER LOGIN PASSWORD %L', '{superuser_pass}');
        RAISE NOTICE 'Created postgres superuser for compatibility';
    ELSE
        ALTER ROLE postgres WITH SUPERUSER;
        RAISE NOTICE 'Ensured postgres has superuser privileges';
    END IF;
END
$$;
"#,
        superuser = creds.superuser,
        superuser_pass = creds.superuser_pass,
        repl_user = creds.repl_user,
        repl_pass = creds.repl_pass,
        app_user = creds.app_user,
        app_pass = creds.app_pass,
    );

    if let Err(e) = run_psql_script(&creds.superuser, &sql) {
        error!(error = %e, "Failed to create users");
        telemetry.send(TelemetryEvent::BootstrapFailed {
            node: node_name,
            error: e.to_string(),
            phase: "create_users".to_string(),
        });
        std::process::exit(1);
    }

    // Create app database if configured
    if !creds.app_db.is_empty() && creds.app_db != "postgres" {
        info!(database = %creds.app_db, "Checking app database");

        let db_exists = run_psql(
            &creds.superuser,
            &format!(
                "SELECT 1 FROM pg_database WHERE datname = '{}'",
                creds.app_db
            ),
        )?;

        if !db_exists.contains('1') {
            info!(database = %creds.app_db, "Creating app database");
            run_psql(
                &creds.superuser,
                &format!("CREATE DATABASE \"{}\"", creds.app_db),
            )?;
        }

        if !creds.app_user.is_empty() && creds.app_user != creds.superuser {
            let grant_sql = format!(
                r#"
DO $$
BEGIN
    EXECUTE format('GRANT ALL PRIVILEGES ON DATABASE %I TO %I', '{db}', '{user}');
END
$$;
"#,
                db = creds.app_db,
                user = creds.app_user,
            );
            run_psql_script(&creds.superuser, &grant_sql)?;
        }
    }

    let mut users_created = vec![creds.superuser.clone(), creds.repl_user.clone()];
    if !creds.app_user.is_empty() && creds.app_user != creds.superuser {
        users_created.push(creds.app_user.clone());
    }

    info!(
        superuser = %creds.superuser,
        replication = %creds.repl_user,
        app_user = %creds.app_user,
        database = %creds.app_db,
        "Users created"
    );

    // Mark bootstrap complete
    let marker_path = format!("{}/.patroni_bootstrap_complete", volume_root());
    std::fs::write(&marker_path, "").context("Failed to write bootstrap marker")?;

    let duration_ms = start.elapsed().as_millis() as u64;
    telemetry.send(TelemetryEvent::BootstrapCompleted {
        node: node_name,
        duration_ms,
        users_created,
    });

    info!(duration_ms, "Post-bootstrap completed");

    Ok(())
}

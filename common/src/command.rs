//! Command execution utilities
//!
//! Provides consistent command execution with proper error handling and logging.

use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, instrument};

/// Result of a command execution.
#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub code: Option<i32>,
}

/// Run a command and return its output.
///
/// This is a low-level function that returns both stdout and stderr.
/// Use `run_checked` if you want to treat non-zero exit as an error.
#[instrument(skip_all, fields(cmd = %cmd))]
pub async fn run(cmd: &str, args: &[&str]) -> Result<CommandOutput> {
    debug!(args = ?args, "Running command");

    let output = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .context(format!("Failed to execute {}", cmd))?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        success: output.status.success(),
        code: output.status.code(),
    })
}

/// Run a command and return stdout if successful, error otherwise.
///
/// # Example
/// ```ignore
/// let version = run_checked("postgres", &["--version"]).await?;
/// ```
pub async fn run_checked(cmd: &str, args: &[&str]) -> Result<String> {
    let output = run(cmd, args).await?;
    if output.success {
        Ok(output.stdout)
    } else {
        let code = output
            .code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        Err(anyhow!("{} failed (exit {}): {}", cmd, code, output.stderr))
    }
}

/// Run a command with sudo.
///
/// # Example
/// ```ignore
/// sudo(&["chown", "postgres:postgres", "/data"]).await?;
/// ```
pub async fn sudo(args: &[&str]) -> Result<String> {
    run_checked("sudo", args).await
}

/// Run an etcdctl command.
///
/// # Example
/// ```ignore
/// let members = etcdctl(&["member", "list"]).await?;
/// ```
pub async fn etcdctl(args: &[&str]) -> Result<String> {
    run_checked("etcdctl", args).await
}

/// Probe with etcdctl - returns Ok(true) if healthy, Ok(false) if unhealthy.
///
/// Unlike `etcdctl`, this distinguishes spawn errors (Err) from
/// endpoint-unhealthy errors (Ok(false)).
///
/// Use this for health probing where you want to try multiple endpoints.
pub async fn etcdctl_probe(args: &[&str]) -> Result<bool> {
    let output = run("etcdctl", args).await?;
    Ok(output.success)
}

/// Run an openssl command.
///
/// # Example
/// ```ignore
/// let cert_info = openssl(&["x509", "-in", "cert.pem", "-text"]).await?;
/// ```
pub async fn openssl(args: &[&str]) -> Result<String> {
    run_checked("openssl", args).await
}

/// Run a psql command.
///
/// # Example
/// ```ignore
/// let result = psql(&["-c", "SELECT 1"]).await?;
/// ```
pub async fn psql(args: &[&str]) -> Result<String> {
    run_checked("psql", args).await
}

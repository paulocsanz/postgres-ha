//! Patroni callback for role changes (failover detection)
//!
//! Called by Patroni with: $1=action $2=role $3=scope
//! Sends telemetry to Railway backboard for monitoring/alerting

use chrono::Utc;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    let action = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let role = args.get(2).map(|s| s.as_str()).unwrap_or("");
    let scope = args.get(3).map(|s| s.as_str()).unwrap_or("");

    // Only proceed for role changes
    if action != "on_role_change" {
        std::process::exit(0);
    }

    let node_name = env::var("PATRONI_NAME").unwrap_or_else(|_| "unknown".to_string());
    let node_address =
        env::var("RAILWAY_PRIVATE_DOMAIN").unwrap_or_else(|_| "unknown".to_string());
    let project_id = env::var("RAILWAY_PROJECT_ID").unwrap_or_default();
    let environment_id = env::var("RAILWAY_ENVIRONMENT_ID").unwrap_or_default();
    let service_id = env::var("RAILWAY_SERVICE_ID").unwrap_or_default();

    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Determine event type based on new role
    let (event_type, message) = match role {
        "master" | "primary" => (
            "POSTGRES_HA_FAILOVER",
            "Node promoted to primary (failover completed)",
        ),
        "replica" | "standby" => ("POSTGRES_HA_REJOINED", "Node rejoined cluster as replica"),
        _ => ("POSTGRES_HA_ROLE_CHANGE", "Node role changed"),
    };

    // Log locally for container logs
    println!(
        "[{}] {}: {} (node={}, scope={}, service={})",
        timestamp, event_type, message, node_name, scope, service_id
    );

    let metadata = format!(
        "node={}, role={}, scope={}, address={}, serviceId={}, projectId={}, environmentId={}",
        node_name, role, scope, node_address, service_id, project_id, environment_id
    );

    let graphql_endpoint = env::var("RAILWAY_GRAPHQL_ENDPOINT")
        .unwrap_or_else(|_| "https://backboard.railway.app/graphql/internal".to_string());

    let payload = serde_json::json!({
        "query": "mutation telemetrySend($input: TelemetrySendInput!) { telemetrySend(input: $input) }",
        "variables": {
            "input": {
                "command": event_type,
                "error": message,
                "stacktrace": metadata,
                "projectId": project_id,
                "environmentId": environment_id,
                "version": "postgres-ha"
            }
        }
    });

    // Send telemetry asynchronously (fire and forget)
    // Use a short timeout to not block Patroni
    let _ = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build();

        if let Ok(client) = client {
            let _ = client
                .post(&graphql_endpoint)
                .header("Content-Type", "application/json")
                .json(&payload)
                .send();
        }
    });

    // Always exit 0 to not block Patroni
    std::process::exit(0);
}

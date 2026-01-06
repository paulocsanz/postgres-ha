//! Patroni callback for role changes (failover detection)
//!
//! Called by Patroni with: $1=action $2=role $3=scope
//! Sends telemetry to Railway backboard for monitoring/alerting

use common::{ConfigExt, Telemetry, TelemetryEvent};
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

    let node_name = String::env_or("PATRONI_NAME", "unknown");
    let telemetry = Telemetry::from_env("postgres-ha");

    let event = match role {
        "master" | "primary" => TelemetryEvent::PostgresFailover {
            node: node_name,
            new_role: role.to_string(),
            scope: scope.to_string(),
        },
        "replica" | "standby" => TelemetryEvent::PostgresRejoined {
            node: node_name,
            role: role.to_string(),
            scope: scope.to_string(),
        },
        _ => TelemetryEvent::ComponentError {
            component: "patroni".to_string(),
            error: format!("Unknown role: {}", role),
            context: "on_role_change".to_string(),
        },
    };

    telemetry.send(event);

    // Always exit 0 to not block Patroni
    std::process::exit(0);
}

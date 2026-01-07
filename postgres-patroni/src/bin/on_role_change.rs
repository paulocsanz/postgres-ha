//! Patroni callback for role changes (failover detection)
//!
//! Called by Patroni with: $1=action $2=role $3=scope
//! Sends telemetry to Railway backboard for monitoring/alerting

use common::{Telemetry, TelemetryEvent};
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    let action = args.get(1);
    let role = args.get(2);
    let scope = args.get(3);

    // Only proceed for role changes
    if action.map(|s| s.as_str()) != Some("on_role_change") {
        std::process::exit(0);
    }

    let node_name = env::var("PATRONI_NAME").ok();
    let telemetry = Telemetry::from_env("postgres-ha");

    let event = match (role.map(|s| s.as_str()), scope, node_name) {
        (Some("master" | "primary"), Some(scope), Some(node)) => TelemetryEvent::PostgresFailover {
            node,
            new_role: role.unwrap().to_string(),
            scope: scope.to_string(),
        },
        (Some("replica" | "standby"), Some(scope), Some(node)) => TelemetryEvent::PostgresRejoined {
            node,
            role: role.unwrap().to_string(),
            scope: scope.to_string(),
        },
        _ => TelemetryEvent::ComponentError {
            component: "patroni".to_string(),
            error: format!(
                "Unexpected on_role_change state: role={:?}, scope={:?}, node={:?}, args={:?}",
                role,
                scope,
                env::var("PATRONI_NAME"),
                args
            ),
            context: "on_role_change".to_string(),
        },
    };

    telemetry.send(event);

    // Always exit 0 to not block Patroni
    std::process::exit(0);
}

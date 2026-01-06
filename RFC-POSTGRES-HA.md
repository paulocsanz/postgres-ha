# RFC: PostgreSQL High Availability Architecture

## Overview

This document describes the architecture and implementation of PostgreSQL High Availability (HA) for Railway. The system provides automatic failover, streaming replication, and intelligent connection routing with approximately 10-second failover times.

## Architecture

### Component Diagram

```
                    Application
                        │
                        ▼
              ┌─────────────────────┐
              │      HAProxy        │
              │  ┌───────┬───────┐  │
              │  │:5432  │:5433  │  │
              │  │(R/W)  │(R/O)  │  │
              └──┴───┬───┴───┬───┴──┘
                     │       │
         ┌───────────┼───────┼───────────┐
         ▼           ▼       ▼           ▼
    ┌─────────┐ ┌─────────┐ ┌─────────┐
    │postgres-1│ │postgres-2│ │postgres-3│
    │(Primary) │ │(Replica) │ │(Replica) │
    │ Patroni  │ │ Patroni  │ │ Patroni  │
    └────┬─────┘ └────┬─────┘ └────┬─────┘
         │            │            │
         └────────────┼────────────┘
                      ▼
         ┌────────────────────────┐
         │    etcd Cluster        │
         │  ┌──────┐ ┌──────┐ ┌──────┐
         │  │etcd-1│ │etcd-2│ │etcd-3│
         │  └──────┘ └──────┘ └──────┘
         └────────────────────────┘
```

### Components

| Component | Count | Purpose |
|-----------|-------|---------|
| PostgreSQL + Patroni | 3 | Database nodes with HA orchestration |
| etcd | 3 | Distributed consensus for leader election |
| HAProxy | 1 | Connection routing and health monitoring |

**Total services**: 7

## How It Works

### 1. Leader Election (etcd + Patroni)

The cluster uses a distributed consensus model where:

1. **etcd** maintains the cluster state and provides distributed locking
2. **Patroni** runs on each PostgreSQL node and participates in leader election
3. One node holds the leader lock and becomes the primary (read-write)
4. Other nodes become replicas (read-only)

Patroni continuously renews its leader lease (TTL: 45 seconds). If the primary fails to renew, other nodes contend for leadership.

### 2. Streaming Replication

Replicas continuously stream WAL (Write-Ahead Log) from the primary:

```
Primary                 Replica
   │                       │
   │◄──── pg_basebackup ───┤  (initial sync)
   │                       │
   │──── WAL stream ──────►│  (continuous)
   │                       │
```

Key configuration:
- **Replication method**: Physical streaming replication
- **Replication slots**: Prevents WAL segment deletion before replica consumption
- **pg_rewind**: Enables fast recovery of failed primaries as replicas

### 3. Connection Routing (HAProxy)

HAProxy provides intelligent routing based on Patroni health endpoints:

| Port | Backend | Health Check | Use Case |
|------|---------|--------------|----------|
| 5432 | Primary only | `GET /primary` → 200 | Writes, transactions |
| 5433 | Replicas (round-robin) | `GET /replica` → 200 | Read scaling |

Health check parameters:
- **Interval**: 3 seconds
- **Fall threshold**: 3 failures to mark down
- **Rise threshold**: 2 successes to mark up

## Failover Process

### Timeline

```
T+0s    Primary crashes or becomes unresponsive
T+3s    HAProxy health check fails (first failure)
T+6s    HAProxy marks primary DOWN (3 failed checks)
T+8s    Patroni leader election via etcd consensus
T+10s   New primary elected, HAProxy routes traffic
```

**Total failover time: ~10 seconds**

### What Happens During Failover

1. **Write connections** (port 5432): Dropped immediately when primary marked down
2. **Read connections** (port 5433): Unaffected (replicas remain healthy)
3. **New connections**: Automatically route to new primary
4. **Old primary recovery**: Rejoins as replica (uses pg_rewind for fast sync)

### Failover Telemetry

On role change, each node sends telemetry to Railway:

```
Event Types:
├── POSTGRES_HA_FAILOVER    # Node promoted to primary
├── POSTGRES_HA_REJOINED    # Node rejoined as replica
└── POSTGRES_HA_ROLE_CHANGE # Generic role change
```

## Cluster Bootstrap

### etcd Bootstrap (Two-Phase)

1. **Leader bootstrap**: First node (alphabetically) creates single-node cluster
2. **Follower join**: Other nodes join as learners, then promote to voters

This prevents quorum disruption during startup.

### PostgreSQL Bootstrap

1. First PostgreSQL node initializes database via Patroni
2. Creates replication user and application database
3. Other nodes perform `pg_basebackup` from primary
4. Begin streaming WAL continuously

## Bash Scripts Deep Dive

This section provides detailed analysis of each service's entrypoint and utility scripts, including their purpose, implementation details, benefits, and potential risks.

---

### 1. etcd: `entrypoint.sh`

**Location**: `/etcd/entrypoint.sh`

#### Purpose

Orchestrates etcd cluster bootstrap with leader-based startup and learner mode to prevent quorum disruption and split-brain scenarios.

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                    etcd Bootstrap Flow                       │
├─────────────────────────────────────────────────────────────┤
│  1. Clean stale data if bootstrap never completed           │
│  2. Determine bootstrap leader (alphabetically first)        │
│  3. Leader path:                                             │
│     └─ Check for existing cluster (recovery detection)       │
│        ├─ Found: Join as learner (prevents split-brain)      │
│        └─ Not found: Bootstrap single-node cluster           │
│  4. Follower path:                                           │
│     └─ Wait for leader → Join as learner → Promote to voter  │
│  5. Monitor health → Mark bootstrap complete → Promote       │
└─────────────────────────────────────────────────────────────┘
```

#### Key Functions

| Function | Description |
|----------|-------------|
| `get_bootstrap_leader()` | Returns alphabetically first node from `ETCD_INITIAL_CLUSTER` |
| `check_existing_cluster()` | Probes other peers to detect if cluster already exists |
| `remove_stale_self()` | Removes old member entry when node rejoins after volume loss |
| `add_self_to_cluster()` | Adds node as learner (non-voting) member |
| `promote_self()` | Promotes learner to voting member after data sync |
| `monitor_and_mark_bootstrap()` | Background loop that handles promotion and bootstrap marker |

#### Configuration Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ETCD_DATA_DIR` | `/var/lib/etcd` | Data directory |
| `ETCD_MAX_RETRIES` | `60` | Max startup attempts |
| `ETCD_RETRY_DELAY` | `5` | Seconds between retries |
| `ETCD_PEER_WAIT_TIMEOUT` | `300` | Max seconds to wait for leader |

#### Benefits

1. **No deadlock on startup**: Leader bootstraps as single-node (instant quorum), followers join after
2. **Split-brain prevention**: Leader checks for existing cluster before bootstrapping
3. **Safe cluster joins**: Learner mode prevents new members from disrupting elections
4. **Automatic recovery**: Handles volume loss gracefully by rejoining as learner
5. **Stale data cleanup**: Removes incomplete bootstrap data on retry

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Leader determination race | Multiple leaders could try to bootstrap | Uses deterministic alphabetical ordering |
| Stale data corruption | etcd fails with member ID mismatch | Cleans data if bootstrap marker absent |
| Promotion failure | Node stays as learner (non-voting) | Continuous retry in monitor loop |
| Network partition during join | Follower can't reach leader | 300s timeout with graceful retry |
| Volume loss on all nodes | Complete data loss | No mitigation (by design - requires backups) |

#### Critical Implementation Details

```bash
# Bootstrap marker prevents re-initialization on restart
BOOTSTRAP_COMPLETE_MARKER="$DATA_DIR/.bootstrap_complete"

# Stale data cleanup - only if marker is missing
if [ -d "$DATA_DIR" ] && [ "$(ls -A "$DATA_DIR")" ]; then
  if [ ! -f "$BOOTSTRAP_COMPLETE_MARKER" ]; then
    rm -rf "${DATA_DIR:?}"/*  # Safe: only cleans incomplete bootstraps
  fi
fi
```

---

### 2. HAProxy: `entrypoint.sh`

**Location**: `/haproxy/entrypoint.sh`

#### Purpose

Dynamically generates HAProxy configuration from environment variables and starts the load balancer with health-based routing.

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                  HAProxy Config Generation                   │
├─────────────────────────────────────────────────────────────┤
│  1. Parse POSTGRES_NODES environment variable                │
│     Format: hostname:pgport:patroniport,hostname:pgport...   │
│                                                              │
│  2. Detect single-node vs multi-node mode                    │
│     ├─ Single: TCP health check (no Patroni dependency)      │
│     └─ Multi: HTTP health check via Patroni REST API         │
│                                                              │
│  3. Generate backends:                                       │
│     ├─ Primary (5432): Routes to /primary endpoint           │
│     └─ Replicas (5433): Routes to /replica endpoint          │
│                                                              │
│  4. Start HAProxy with generated config                      │
└─────────────────────────────────────────────────────────────┘
```

#### Generated Configuration Structure

```
Frontend (5432) ──► Backend (primary)
                    └─ HTTP check: GET /primary → 200
                    └─ on-marked-down: shutdown-sessions

Frontend (5433) ──► Backend (replicas)
                    └─ HTTP check: GET /replica → 200
                    └─ balance: roundrobin
```

#### Configuration Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `POSTGRES_NODES` | (required) | Comma-separated node list |
| `HAPROXY_MAX_CONN` | `1000` | Maximum connections |
| `HAPROXY_TIMEOUT_CONNECT` | `10s` | Connection timeout |
| `HAPROXY_TIMEOUT_CLIENT` | `30m` | Client idle timeout |
| `HAPROXY_TIMEOUT_SERVER` | `30m` | Server idle timeout |
| `HAPROXY_CHECK_INTERVAL` | `3s` | Health check interval |

#### Benefits

1. **Dynamic configuration**: No hardcoded hostnames; config generated from environment
2. **Graceful failover**: `on-marked-down shutdown-sessions` forces reconnection to new primary
3. **Single-node compatibility**: Works without Patroni for standalone PostgreSQL
4. **DNS resolution**: Uses Railway's DNS resolver for service discovery
5. **Stats dashboard**: Port 8404 provides real-time monitoring

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Invalid `POSTGRES_NODES` format | HAProxy fails to start | Validation with clear error message |
| DNS resolution failure | Backend unreachable | `resolve_retries 3` with 1s timeout |
| Health check false positive | Traffic to unhealthy node | `fall 3` requires 3 consecutive failures |
| Long-lived connections during failover | Connections to old primary hang | `shutdown-sessions` terminates immediately |
| Single HAProxy instance | Single point of failure | Can run multiple instances (not in current config) |

#### Critical Implementation Details

```bash
# Single-node detection changes health check behavior
NODE_COUNT=$(count_nodes)
if [ "$NODE_COUNT" -eq 1 ]; then
    SINGLE_NODE_MODE="true"
    # Uses TCP check on PostgreSQL port, not Patroni HTTP
fi

# Server entry generation with Patroni port for health checks
echo "server ${name} ${host}:${pgport} check port ${patroniport} resolvers railway"
```

---

### 3. PostgreSQL: `wrapper.sh`

**Location**: `/postgres-patroni/wrapper.sh`

#### Purpose

Container entrypoint that validates environment, manages SSL certificates, and routes to either Patroni (HA mode) or standalone PostgreSQL.

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                    wrapper.sh Flow                           │
├─────────────────────────────────────────────────────────────┤
│  1. Validate volume mount path (Railway-specific)           │
│  2. Validate PGDATA is within volume mount                   │
│                                                              │
│  3. If PATRONI_ENABLED=true:                                 │
│     ├─ Create/chown data directory (Railway mounts as root)  │
│     ├─ Validate required passwords on fresh install          │
│     ├─ Generate/renew SSL certificates                       │
│     └─ exec gosu postgres patroni-runner.sh                  │
│                                                              │
│  4. Else (standalone mode):                                  │
│     ├─ Check/renew SSL certificates                          │
│     ├─ Unset PGHOST/PGPORT (Railway-specific)                │
│     └─ exec docker-entrypoint.sh                             │
└─────────────────────────────────────────────────────────────┘
```

#### Validation Checks

| Check | Failure Condition | Exit? |
|-------|-------------------|-------|
| Volume mount path | `RAILWAY_VOLUME_MOUNT_PATH != /var/lib/postgresql/data` | Yes |
| PGDATA location | PGDATA doesn't start with expected volume path | Yes |
| SSL certificate validity | Not X.509v3 (missing SAN) | Regenerate |
| SSL certificate expiry | Expires within 30 days | Regenerate |
| Required passwords | Missing `POSTGRES_PASSWORD` or `PATRONI_REPLICATION_PASSWORD` on fresh install | Yes |

#### Benefits

1. **Early failure detection**: Volume misconfiguration caught before data corruption
2. **Automatic SSL renewal**: Certificates regenerated before expiry
3. **Permission handling**: Fixes Railway root-mounted volumes (`chown -R postgres:postgres`)
4. **Graceful timeout**: 120s timeout on chown prevents hanging on volume issues
5. **Dual-mode support**: Same image works for HA and standalone

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| `chown` timeout | Container stuck | 120s timeout with error exit |
| Missing passwords | Database unreachable | Explicit validation with clear error |
| SSL regeneration failure | PostgreSQL won't start | `set -e` fails fast |
| Volume not mounted | Data loss on restart | Path validation before any operations |
| X.509v3 detection failure | Unnecessary cert regeneration | Conservative: regen if uncertain |

#### Critical Implementation Details

```bash
# Railway-specific: volumes mount as root, need to fix ownership
if ! timeout 120 sudo chown -R postgres:postgres "$DATA_DIR"; then
    echo "ERROR: chown timed out after 120s - volume may have issues"
    exit 1
fi

# SSL certificate X.509v3 check (SAN requirement)
if ! openssl x509 -noout -text -in "$SSL_DIR/server.crt" | grep -q "DNS:localhost"; then
    echo "Did not find a x509v3 certificate, regenerating..."
    bash "$INIT_SSL_SCRIPT"
fi
```

---

### 4. PostgreSQL: `patroni-runner.sh`

**Location**: `/postgres-patroni/patroni-runner.sh`

#### Purpose

Generates Patroni YAML configuration, starts Patroni as a child process, and implements health monitoring with automatic container restart on failure.

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                  patroni-runner.sh Flow                      │
├─────────────────────────────────────────────────────────────┤
│  1. Validate required environment variables                  │
│  2. Handle PATRONI_ADOPT_EXISTING_DATA (vanilla→HA migration)│
│  3. Generate /tmp/patroni.yml from environment               │
│  4. Set umask 0077 (correct pg_basebackup permissions)       │
│  5. Unset PG* vars (prevents pgpass override)                │
│  6. Start Patroni as background process                      │
│  7. Trap SIGTERM/INT for graceful shutdown                   │
│  8. Wait for startup grace period                            │
│  9. Health monitoring loop:                                  │
│     ├─ Check process alive (kill -0)                         │
│     ├─ Check HTTP endpoint (/health)                         │
│     └─ Exit after MAX_FAILURES consecutive failures          │
└─────────────────────────────────────────────────────────────┘
```

#### Generated Patroni Configuration

The script generates `/tmp/patroni.yml` with:

- **DCS settings**: TTL, loop_wait, failsafe_mode
- **PostgreSQL parameters**: wal_level, hot_standby, SSL paths
- **Authentication**: Superuser, replication, and app user credentials
- **Callbacks**: `on_role_change` for failover telemetry
- **Bootstrap config**: pg_hba rules, initdb options

#### Health Monitoring Parameters

| Variable | Default | Description |
|----------|---------|-------------|
| `PATRONI_HEALTH_CHECK_INTERVAL` | `5` | Seconds between checks |
| `PATRONI_HEALTH_CHECK_TIMEOUT` | `5` | HTTP request timeout |
| `PATRONI_MAX_HEALTH_FAILURES` | `3` | Failures before exit |
| `PATRONI_STARTUP_GRACE_PERIOD` | `60` | Seconds before health checks start |

#### Benefits

1. **Self-healing**: Container restarts on Patroni hang (not just crash)
2. **Graceful shutdown**: Traps signals and forwards to Patroni
3. **Early health check exit**: Can start monitoring before grace period if healthy
4. **PG* variable isolation**: Prevents credential confusion with pgpass
5. **Migration support**: `PATRONI_ADOPT_EXISTING_DATA` for vanilla→HA conversion

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Health check false negative | Unnecessary restart | 3 consecutive failures required |
| Patroni hung but process alive | Undetected failure | HTTP endpoint check, not just `kill -0` |
| pgpass credential mismatch | Replication auth failure | Unset PGPASSWORD/PGUSER before start |
| umask not set | pg_basebackup files too permissive | Explicit `umask 0077` |
| Grace period too short | Restart during slow bootstrap | 60s default, configurable |

#### Critical Implementation Details

```bash
# CRITICAL: Unset PG* vars to prevent pgpass override
# PGPASSWORD takes precedence, causing wrong password for replication
unset PGPASSWORD PGUSER PGHOST PGPORT PGDATABASE

# Health monitoring with both process and HTTP check
while true; do
    if ! kill -0 "$PATRONI_PID" 2>/dev/null; then
        exit 1  # Process died
    fi
    if ! curl -sf --max-time 5 http://localhost:8008/health; then
        failures=$((failures + 1))
        if [ $failures -ge $MAX_FAILURES ]; then
            kill -TERM "$PATRONI_PID"
            exit 1  # Trigger container restart
        fi
    fi
done
```

---

### 5. PostgreSQL: `init-ssl.sh`

**Location**: `/postgres-patroni/init-ssl.sh`

#### Purpose

Generates self-signed X.509v3 SSL certificates for PostgreSQL with proper Subject Alternative Names (SAN).

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                    SSL Certificate Chain                     │
├─────────────────────────────────────────────────────────────┤
│  Root CA (self-signed)                                       │
│  ├─ CN: root-ca                                              │
│  ├─ Validity: 820 days                                       │
│  └─ Files: root.crt, root.key                                │
│      │                                                       │
│      └─► Server Certificate (signed by Root CA)              │
│          ├─ CN: localhost                                    │
│          ├─ SAN: DNS:localhost                               │
│          ├─ Validity: 820 days                               │
│          └─ Files: server.crt, server.key                    │
└─────────────────────────────────────────────────────────────┘
```

#### Generated Files

| File | Permissions | Description |
|------|-------------|-------------|
| `root.crt` | 644 | Root CA certificate |
| `root.key` | 600 | Root CA private key |
| `server.crt` | 644 | Server certificate |
| `server.key` | 600 | Server private key |
| `server.csr` | 644 | Certificate signing request |
| `v3.ext` | 644 | X.509v3 extensions file |

#### Configuration Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SSL_CERT_DAYS` | `820` | Certificate validity period |
| `PGDATA` | (required) | PostgreSQL data directory |

#### Benefits

1. **X.509v3 compliance**: SAN extension required by modern TLS clients
2. **Persistence**: Certs stored in `/data/certs/`, survive pgdata rebuilds
3. **Automatic renewal**: Wrapper checks expiry and regenerates
4. **Correct permissions**: Private keys restricted to postgres user
5. **Patroni-aware**: Skips postgresql.conf modification in Patroni mode

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Self-signed certs | Clients must trust or skip verification | Use CA-signed certs in production |
| Same cert on all nodes | Cannot verify specific node identity | Acceptable for internal traffic |
| Key file permissions | Security vulnerability if readable | `chmod og-rwx` on generation |
| 820-day validity | Manual renewal if not auto-renewed | Wrapper checks expiry on every start |
| localhost CN only | Certificate hostname mismatch | Clients should use `sslmode=require` not `verify-full` |

#### Critical Implementation Details

```bash
# X.509v3 extensions file with SAN (required by modern TLS)
cat >| "$SSL_V3_EXT" <<EOF
[v3_req]
authorityKeyIdentifier = keyid, issuer
basicConstraints = critical, CA:TRUE
keyUsage = digitalSignature, nonRepudiation, keyEncipherment, dataEncipherment
subjectAltName = DNS:localhost
EOF

# Sign server cert with root CA
openssl x509 -req -in "$SSL_SERVER_CSR" -extfile "$SSL_V3_EXT" \
    -extensions v3_req -days "${SSL_CERT_DAYS:-820}" \
    -CA "$SSL_ROOT_CRT" -CAkey "$SSL_ROOT_KEY" -CAcreateserial \
    -out "$SSL_SERVER_CRT"
```

---

### 6. PostgreSQL: `post_bootstrap.sh`

**Location**: `/postgres-patroni/post_bootstrap.sh`

#### Purpose

Patroni post-bootstrap callback that creates database users and the application database. Runs once after initial PostgreSQL initialization on the primary node.

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                  post_bootstrap.sh Flow                      │
├─────────────────────────────────────────────────────────────┤
│  1. Read credentials from /tmp/patroni.yml                   │
│     (Patroni runs callbacks WITHOUT environment variables)   │
│                                                              │
│  2. Create/update users via psql:                            │
│     ├─ Superuser password                                    │
│     ├─ Replication user (replicator)                         │
│     ├─ App user (POSTGRES_USER)                              │
│     └─ Ensure 'postgres' role exists with SUPERUSER          │
│                                                              │
│  3. Create app database if configured                        │
│  4. Grant privileges on app database to app user             │
│  5. Create bootstrap marker file                             │
└─────────────────────────────────────────────────────────────┘
```

#### Credential Sources

| Credential | Source | Notes |
|------------|--------|-------|
| Superuser | `patroni.yml → authentication.superuser` | From `PATRONI_SUPERUSER_PASSWORD` |
| Replication | `patroni.yml → authentication.replication` | From `PATRONI_REPLICATION_PASSWORD` |
| App user | `patroni.yml → app_user` | From `POSTGRES_USER/PASSWORD` |

#### Benefits

1. **SQL injection protection**: Uses `format()` function for all password operations
2. **Idempotent**: `CREATE OR ALTER` pattern handles re-runs safely
3. **Compatibility**: Always ensures `postgres` role exists with SUPERUSER
4. **Clean environment**: Uses `env -i` to prevent PG* variable interference
5. **Separate database creation**: App database created outside transaction block

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| YAML parsing failure | Users not created | Explicit validation with error exit |
| Special chars in password | SQL injection or parse error | `format()` with `%L` placeholder |
| Missing patroni.yml | Script fails | Check file exists before parsing |
| psql connection failure | Users not created | `ON_ERROR_STOP=1` fails fast |
| Re-run on recovery | Duplicate user errors | `IF NOT EXISTS` / `IF EXISTS` checks |

#### Critical Implementation Details

```bash
# CRITICAL: Patroni runs callbacks without env vars
# Must read credentials from config file
REPL_PASS=$(grep -A2 'replication:' "$PATRONI_CONFIG" | grep 'password:' | head -1 | strip_yaml)

# SQL injection protection with format()
DO $$
BEGIN
    EXECUTE format('ALTER ROLE %I WITH PASSWORD %L', '${SUPERUSER}', '${SUPERUSER_PASS}');
END
$$;

# Clean environment to prevent PGPASSWORD interference
env -i PATH="$PATH" psql -v ON_ERROR_STOP=1 -h /var/run/postgresql -U "$SUPERUSER" -d postgres
```

---

### 7. PostgreSQL: `on_role_change.sh`

**Location**: `/postgres-patroni/on_role_change.sh`

#### Purpose

Patroni callback that sends telemetry to Railway's backboard service when a node's role changes (failover detection, replica rejoins).

#### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│                  on_role_change.sh Flow                      │
├─────────────────────────────────────────────────────────────┤
│  Called by Patroni: $1=action $2=role $3=scope               │
│                                                              │
│  1. Filter: Only process "on_role_change" actions            │
│                                                              │
│  2. Determine event type from new role:                      │
│     ├─ master/primary → POSTGRES_HA_FAILOVER                 │
│     ├─ replica/standby → POSTGRES_HA_REJOINED                │
│     └─ other → POSTGRES_HA_ROLE_CHANGE                       │
│                                                              │
│  3. Log locally (container stdout)                           │
│                                                              │
│  4. Send async GraphQL mutation to backboard:                │
│     └─ Non-blocking, 5s timeout, fire-and-forget             │
│                                                              │
│  5. Always exit 0 (never block Patroni)                      │
└─────────────────────────────────────────────────────────────┘
```

#### Event Types

| Event | Trigger | Urgency |
|-------|---------|---------|
| `POSTGRES_HA_FAILOVER` | Node promoted to primary | High - indicates failover completed |
| `POSTGRES_HA_REJOINED` | Node became replica | Medium - recovery or switchover |
| `POSTGRES_HA_ROLE_CHANGE` | Unknown role | Low - unexpected state |

#### Telemetry Payload

```json
{
  "command": "POSTGRES_HA_FAILOVER",
  "error": "Node promoted to primary (failover completed)",
  "stacktrace": "node=postgres-1, role=master, scope=pg-ha, ...",
  "projectId": "...",
  "environmentId": "...",
  "version": "postgres-ha"
}
```

#### Benefits

1. **Non-blocking**: Async curl with `&`, never delays Patroni
2. **Failover visibility**: Railway can alert on promotion events
3. **Local logging**: Always logs to container stdout regardless of network
4. **Graceful degradation**: Network failure doesn't affect PostgreSQL
5. **Always succeeds**: Exit 0 prevents Patroni callback errors

#### Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Network unreachable | Telemetry lost | Local logging as backup |
| curl not installed | Silent failure | `command -v curl` check |
| Backboard timeout | Delayed curl process | 5s max-time, background execution |
| Callback blocks Patroni | Delayed failover | `exit 0` always, async curl |
| Sensitive data in logs | Security concern | Only logs node name, role, scope |

#### Critical Implementation Details

```bash
# Always exit 0 - NEVER block Patroni
exit 0

# Async curl - fire and forget
curl -s -X POST \
    -H "Content-Type: application/json" \
    -d "${PAYLOAD}" \
    "${GRAPHQL_ENDPOINT}" \
    --max-time 5 \
    > /dev/null 2>&1 &  # Background, discard output
```

---

## Script Interaction Diagram

```
Container Start
      │
      ▼
┌─────────────┐
│ wrapper.sh  │ ◄─── Validates volume, manages SSL
└──────┬──────┘
       │
       ▼ (if PATRONI_ENABLED)
┌──────────────────┐
│ patroni-runner.sh│ ◄─── Generates config, starts Patroni, monitors health
└────────┬─────────┘
         │
         ├──────────────────────────┐
         ▼                          ▼
┌─────────────────┐        ┌────────────────┐
│ post_bootstrap.sh│       │on_role_change.sh│
│ (once on primary)│       │ (on failover)   │
└─────────────────┘        └────────────────┘
         │
         ▼
  ┌────────────┐
  │ init-ssl.sh│ ◄─── Called by wrapper.sh if certs missing
  └────────────┘
```

## Data Safety

### Replication Guarantees

```yaml
maximum_lag_on_failover: 1048576  # 1GB max lag for failover
use_pg_rewind: true               # Fast primary recovery
use_slots: true                   # Prevents WAL loss
failsafe_mode: true               # Prevents cascading failures
```

### SSL/TLS

All connections secured with TLS:
- Self-signed certificates per node
- Auto-renewal when expiring within 30 days
- Both client and replication connections encrypted

## Patroni Configuration

Key parameters managed by Patroni:

```yaml
postgresql:
  parameters:
    wal_level: replica
    hot_standby: "on"
    max_wal_senders: 10
    max_replication_slots: 10
    password_encryption: scram-sha-256

bootstrap:
  dcs:
    ttl: 45                    # Leader lease TTL
    loop_wait: 10              # Health check interval
    retry_timeout: 30          # DCS response timeout
```

## Health Monitoring

### Container Health Check

```dockerfile
HEALTHCHECK --interval=5s --timeout=3s --start-period=30s --retries=2
```

Checks:
- Patroni mode: `curl http://localhost:8008/health`
- Standalone mode: `pg_isready`

### Application Health Check

Query the cluster status:

```bash
curl http://postgres-1:8008/cluster | jq
```

Returns JSON with all members, roles, replication lag, and timeline info.

## Recovery Scenarios

### Primary Node Failure

1. Patroni detects leader lock expiration
2. Remaining nodes hold election via etcd
3. Winner promotes to primary, updates DCS
4. HAProxy detects new primary via `/primary` endpoint
5. Failed node rejoins as replica when recovered

### etcd Node Failure

- Cluster continues with 2/3 nodes (maintains quorum)
- Failed node rejoins and syncs when recovered
- If leader fails, new leader elected automatically

### Network Partition

- `failsafe_mode` prevents split-brain
- Node isolated from etcd demotes itself
- Rejoins cluster when connectivity restored

## Deployment Model

### Railway Production

```
Project
├── etcd-1, etcd-2, etcd-3     (3 services)
├── postgres-1, postgres-2, postgres-3  (3 services)
└── haproxy                     (1 service)
```

All services communicate via Railway private networking.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `PATRONI_NAME` | Unique node identifier |
| `PATRONI_SCOPE` | Cluster name |
| `PATRONI_ETCD3_HOSTS` | etcd connection string |
| `POSTGRES_USER` | Application database user |
| `POSTGRES_PASSWORD` | Application user password |
| `PATRONI_REPLICATION_PASSWORD` | Replication user password |

## Trade-offs

### Strengths

- **Fast failover**: ~10 second recovery time
- **Automatic recovery**: Failed nodes rejoin without intervention
- **Read scaling**: Distribute reads across replicas
- **Data safety**: Replication slots prevent data loss

### Limitations

- **Minimum 7 services**: Higher resource overhead
- **etcd dependency**: Additional operational complexity
- **Async replication**: Small window of potential data loss on failover
- **Single-region**: This design assumes all nodes in same region

## Future Considerations

- Synchronous replication option for zero data loss
- Multi-region deployment with witness nodes
- Automated scaling of replica count
- Integration with external backup systems

## References

- [Patroni Documentation](https://patroni.readthedocs.io/)
- [etcd Documentation](https://etcd.io/docs/)
- [HAProxy Configuration Manual](https://www.haproxy.com/documentation/)

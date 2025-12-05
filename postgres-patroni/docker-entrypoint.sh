#!/bin/bash
set -e

# Patroni mode entrypoint
# Only runs when PATRONI_ENABLED=true

DATA_DIR="/var/lib/postgresql/data"
CERTS_DIR="$DATA_DIR/certs"

echo "=== Patroni Entrypoint ==="

# Configuration from environment
SCOPE="${PATRONI_SCOPE:-railway-pg-ha}"
NAME="${PATRONI_NAME:-postgres-1}"
ETCD_HOSTS="${PATRONI_ETCD_HOSTS:-etcd-1.railway.internal:2379,etcd-2.railway.internal:2379,etcd-3.railway.internal:2379}"

# Credentials
SUPERUSER="${PATRONI_SUPERUSER_USERNAME:-${POSTGRES_USER:-postgres}}"
SUPERUSER_PASS="${PATRONI_SUPERUSER_PASSWORD:-${POSTGRES_PASSWORD:-postgres}}"
REPL_USER="${PATRONI_REPLICATION_USERNAME:-replicator}"
REPL_PASS="${PATRONI_REPLICATION_PASSWORD:-replicator_password}"

# Determine if this is the primary (first node) or a replica
IS_PRIMARY=false
if [ "$NAME" = "postgres-1" ] || [ "$NAME" = "${PATRONI_SCOPE}-1" ]; then
    IS_PRIMARY=true
fi

echo "Node: $NAME (primary: $IS_PRIMARY)"

# For replicas, clean any stale data to ensure fresh clone
if [ "$IS_PRIMARY" = "false" ] && [ -f "$DATA_DIR/PG_VERSION" ]; then
    echo "Cleaning stale data on replica for fresh clone..."
    find "$DATA_DIR" -mindepth 1 -maxdepth 1 ! -name 'certs' -exec rm -rf {} +
fi

# For primary with no data, clear etcd to ensure clean bootstrap
if [ "$IS_PRIMARY" = "true" ] && [ ! -f "$DATA_DIR/PG_VERSION" ]; then
    echo "Primary with no data - clearing etcd for fresh bootstrap..."
    for endpoint in $(echo $ETCD_HOSTS | tr ',' ' '); do
        curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE?recursive=true" 2>/dev/null || true
        break
    done
fi

# Write credentials for post_bootstrap script
cat > /tmp/patroni_creds.sh <<CREDEOF
export PATRONI_SUPERUSER_USERNAME="$SUPERUSER"
export PATRONI_SUPERUSER_PASSWORD="$SUPERUSER_PASS"
export PATRONI_REPLICATION_USERNAME="$REPL_USER"
export PATRONI_REPLICATION_PASSWORD="$REPL_PASS"
export POSTGRES_DB="${POSTGRES_DB:-}"
CREDEOF
chmod 600 /tmp/patroni_creds.sh

# Generate Patroni configuration
cat > /tmp/patroni.yml <<EOF
scope: ${SCOPE}
name: ${NAME}

restapi:
  listen: 0.0.0.0:8008
  connect_address: ${NAME}.railway.internal:8008

etcd:
  hosts: ${ETCD_HOSTS}

bootstrap:
  method: initdb
  dcs:
    ttl: ${PATRONI_TTL:-30}
    loop_wait: ${PATRONI_LOOP_WAIT:-10}
    retry_timeout: 10
    maximum_lag_on_failover: 1048576
    postgresql:
      use_pg_rewind: true
      use_slots: true
      parameters:
        wal_level: replica
        hot_standby: "on"
        wal_keep_size: 128MB
        max_wal_senders: 10
        max_replication_slots: 10
        max_connections: 200
        shared_buffers: 256MB
        effective_cache_size: 1GB
        ssl: "on"
        ssl_cert_file: "${CERTS_DIR}/server.crt"
        ssl_key_file: "${CERTS_DIR}/server.key"
        ssl_ca_file: "${CERTS_DIR}/ca.crt"

  initdb:
    - encoding: UTF8
    - data-checksums
    - locale: en_US.UTF-8

  pg_hba:
    - local all all trust
    - hostssl replication ${REPL_USER} 0.0.0.0/0 md5
    - hostssl all all 0.0.0.0/0 md5
    - host replication ${REPL_USER} 0.0.0.0/0 md5
    - host all all 0.0.0.0/0 md5

  post_bootstrap: /post_bootstrap.sh

postgresql:
  listen: 0.0.0.0:5432
  connect_address: ${NAME}.railway.internal:5432
  data_dir: ${DATA_DIR}
  pgpass: /tmp/pgpass
  authentication:
    replication:
      username: ${REPL_USER}
      password: ${REPL_PASS}
    superuser:
      username: ${SUPERUSER}
      password: ${SUPERUSER_PASS}
  parameters:
    unix_socket_directories: /var/run/postgresql
    ssl: "on"
    ssl_cert_file: "${CERTS_DIR}/server.crt"
    ssl_key_file: "${CERTS_DIR}/server.key"
    ssl_ca_file: "${CERTS_DIR}/ca.crt"

tags:
  nofailover: false
  noloadbalance: false
  clonefrom: false
  nosync: false
EOF

echo "=== Patroni Configuration ==="
echo "  Scope: $SCOPE"
echo "  Name: $NAME"
echo "  Data dir: $DATA_DIR"
echo "  etcd: $ETCD_HOSTS"

# Cleanup function: kill PostgreSQL if Patroni dies
cleanup() {
    echo "Patroni exiting, ensuring PostgreSQL is stopped..."
    pkill -9 -f "postgres" 2>/dev/null || true
    exit 1
}

trap cleanup EXIT SIGTERM SIGINT SIGQUIT

# Start Patroni
patroni /tmp/patroni.yml &
PATRONI_PID=$!

wait $PATRONI_PID
EXIT_CODE=$?

echo "Patroni exited with code $EXIT_CODE"
exit $EXIT_CODE

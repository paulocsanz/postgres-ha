#!/bin/bash
set -e

# Patroni mode entrypoint
# Only runs when PATRONI_ENABLED=true

DATA_DIR="/var/lib/postgresql/data"
CERTS_DIR="$DATA_DIR/certs"

echo "=== Patroni Entrypoint ==="

# Check for data in old pgdata subdirectory location and migrate if needed
if [ -f "$DATA_DIR/pgdata/PG_VERSION" ] && [ ! -f "$DATA_DIR/PG_VERSION" ]; then
    echo "Found data in old pgdata/ subdirectory, migrating to new location..."
    # Move everything from pgdata up one level (except certs if exists)
    mv "$DATA_DIR/pgdata"/* "$DATA_DIR/" 2>/dev/null || true
    rmdir "$DATA_DIR/pgdata" 2>/dev/null || true
    echo "Data migration complete"
fi

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

# Check if we need to adopt existing standalone data or reinitialize
ADOPTING_EXISTING=false
FORCE_REINIT="${PATRONI_FORCE_REINIT:-false}"

if [ -f "$DATA_DIR/PG_VERSION" ]; then
    if [ "$IS_PRIMARY" = "true" ]; then
        echo "Existing PostgreSQL data found on primary"

        # Check if etcd has mismatched state by querying for initialize key
        ETCD_HAS_STATE=false
        for endpoint in $(echo $ETCD_HOSTS | tr ',' ' '); do
            INIT_RESPONSE=$(curl -s "http://$endpoint/v2/keys/service/$SCOPE/initialize" 2>/dev/null || echo "")
            if echo "$INIT_RESPONSE" | grep -q '"value"'; then
                ETCD_HAS_STATE=true
                echo "etcd has existing cluster state"
            fi
            break
        done

        # Force reinit if requested OR if this looks like standalone data OR etcd has mismatched state
        if [ "$FORCE_REINIT" = "true" ]; then
            echo "PATRONI_FORCE_REINIT=true - forcing cluster reinitialization"
            ADOPTING_EXISTING=true
        elif [ ! -f "$DATA_DIR/patroni.dynamic.json" ]; then
            echo "Data appears to be from standalone PostgreSQL - preparing for adoption"
            ADOPTING_EXISTING=true
        elif [ "$ETCD_HAS_STATE" = "true" ]; then
            # patroni.dynamic.json exists but etcd might have stale state from failed attempt
            echo "Patroni data exists but checking etcd consistency..."
            # Try to get the system identifier from pg_control
            if command -v pg_controldata &> /dev/null; then
                LOCAL_SYSID=$(pg_controldata "$DATA_DIR" 2>/dev/null | grep "Database system identifier" | awk '{print $NF}')
                echo "Local system ID: $LOCAL_SYSID"
            fi
            # If we can't verify, assume we need to clear
            echo "Clearing potentially stale etcd state to be safe"
            ADOPTING_EXISTING=true
        fi

        if [ "$ADOPTING_EXISTING" = "true" ]; then
            # Clear ALL cluster state from etcd to allow fresh adoption
            echo "Clearing etcd cluster state for fresh adoption..."
            for endpoint in $(echo $ETCD_HOSTS | tr ',' ' '); do
                echo "  Clearing $endpoint..."
                # Delete the entire scope directory recursively
                curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE?recursive=true" 2>/dev/null || true
                # Also try v3 API format
                curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/initialize" 2>/dev/null || true
                curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/leader" 2>/dev/null || true
                curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/members" 2>/dev/null || true
                break
            done
            echo "etcd cluster state cleared"

            # Ensure replication settings are in postgresql.conf for adoption
            echo "Configuring PostgreSQL for replication..."
            PG_CONF="$DATA_DIR/postgresql.conf"
            if [ -f "$PG_CONF" ]; then
                # Add/update replication settings
                grep -q "^wal_level" "$PG_CONF" || echo "wal_level = replica" >> "$PG_CONF"
                grep -q "^max_wal_senders" "$PG_CONF" || echo "max_wal_senders = 10" >> "$PG_CONF"
                grep -q "^max_replication_slots" "$PG_CONF" || echo "max_replication_slots = 10" >> "$PG_CONF"
                grep -q "^hot_standby" "$PG_CONF" || echo "hot_standby = on" >> "$PG_CONF"
                grep -q "^wal_keep_size" "$PG_CONF" || echo "wal_keep_size = 128MB" >> "$PG_CONF"
            fi

            # Create a script to run after postgres starts to create replicator user
            cat > /tmp/setup_replication.sh <<REPLEOF
#!/bin/bash
sleep 10  # Wait for postgres to be ready
for i in {1..30}; do
    if pg_isready -U postgres; then
        echo "Creating replication user..."
        psql -U postgres -c "DO \\\$\\\$
        BEGIN
            IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = '${REPL_USER}') THEN
                CREATE ROLE ${REPL_USER} WITH REPLICATION PASSWORD '${REPL_PASS}' LOGIN;
            END IF;
        END
        \\\$\\\$;" && break
    fi
    sleep 2
done
REPLEOF
            chmod +x /tmp/setup_replication.sh
        else
            echo "Data was created by Patroni - normal startup"
        fi
    else
        echo "Existing data found on replica node - cleaning for fresh clone..."
        # Keep certs directory
        find "$DATA_DIR" -mindepth 1 -maxdepth 1 ! -name 'certs' -exec rm -rf {} +
        echo "Data directory cleaned, will clone from leader"
    fi
else
    echo "No existing data - Patroni will initialize (primary) or clone (replica)"

    # Only clear etcd state if we're the primary - replicas should just clone
    if [ "$IS_PRIMARY" = "true" ]; then
        echo "Primary node with no data - clearing ALL stale etcd cluster state..."
        for endpoint in $(echo $ETCD_HOSTS | tr ',' ' '); do
            echo "  Clearing $endpoint..."
            # Delete the entire scope directory recursively
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE?recursive=true" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/initialize" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/leader" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/members" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/config" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/history" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/status" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/sync" 2>/dev/null || true
            curl -s -X DELETE "http://$endpoint/v2/keys/service/$SCOPE/failover" 2>/dev/null || true
            break
        done
        echo "etcd cluster state cleared"
    else
        echo "Replica node with no data - will clone from leader"
    fi
fi

# Write credentials and settings for post_bootstrap script
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
        # SSL configuration
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

echo ""
echo "=== Patroni Configuration ==="
echo "  Scope: $SCOPE"
echo "  Name: $NAME"
echo "  Data dir: $DATA_DIR"
echo "  Is Primary: $IS_PRIMARY"
echo "  etcd: $ETCD_HOSTS"
echo ""

export PATRONI_CONFIG_FILE=/tmp/patroni.yml

# Cleanup function: kill PostgreSQL if Patroni dies
cleanup() {
    echo "Patroni exiting, ensuring PostgreSQL is stopped..."
    # Kill any PostgreSQL processes to prevent orphan/split-brain
    pkill -9 -f "postgres" 2>/dev/null || true
    exit 1
}

# Trap signals and unexpected exits
trap cleanup EXIT SIGTERM SIGINT SIGQUIT

# Run replication setup in background if adopting existing data
if [ "$ADOPTING_EXISTING" = "true" ] && [ -f /tmp/setup_replication.sh ]; then
    echo "Starting replication setup in background..."
    /tmp/setup_replication.sh &
fi

# Start Patroni (not exec, so trap works)
patroni /tmp/patroni.yml &
PATRONI_PID=$!

# Wait for Patroni and trigger cleanup on exit
wait $PATRONI_PID
EXIT_CODE=$?

echo "Patroni exited with code $EXIT_CODE"
exit $EXIT_CODE

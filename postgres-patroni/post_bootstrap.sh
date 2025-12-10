#!/bin/bash
# post_bootstrap.sh - Patroni post-bootstrap script
#
# Runs ONCE after PostgreSQL initialization on the primary node.
# Patroni 4.0+ requires users to be created here (bootstrap.users is deprecated)

set -e

echo "Post-bootstrap: starting..."

# Get credentials from environment
SUPERUSER="${PATRONI_SUPERUSER_USERNAME:-postgres}"
SUPERUSER_PASS="${PATRONI_SUPERUSER_PASSWORD}"
REPL_USER="${PATRONI_REPLICATION_USERNAME:-replicator}"
REPL_PASS="${PATRONI_REPLICATION_PASSWORD}"

# Debug: check if env vars are available
echo "DEBUG: SUPERUSER=${SUPERUSER}"
echo "DEBUG: REPL_USER=${REPL_USER}"
echo "DEBUG: SUPERUSER_PASS set: $([ -n \"${SUPERUSER_PASS}\" ] && echo 'yes' || echo 'no')"
echo "DEBUG: REPL_PASS set: $([ -n \"${REPL_PASS}\" ] && echo 'yes' || echo 'no')"

if [ -z "$SUPERUSER_PASS" ] || [ -z "$REPL_PASS" ]; then
    echo "ERROR: Missing passwords - SUPERUSER_PASS empty: $([ -z \"$SUPERUSER_PASS\" ] && echo yes || echo no), REPL_PASS empty: $([ -z \"$REPL_PASS\" ] && echo yes || echo no)"
    exit 1
fi

echo "Post-bootstrap: creating users..."

# Create superuser and replicator using simple SQL (avoid DO blocks for clarity)
psql -v ON_ERROR_STOP=1 -U postgres -d postgres <<EOSQL
-- Create superuser if not exists, or update password
DO \$\$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${SUPERUSER}') THEN
        EXECUTE format('CREATE ROLE %I WITH SUPERUSER CREATEDB CREATEROLE LOGIN PASSWORD %L', '${SUPERUSER}', '${SUPERUSER_PASS}');
    ELSE
        EXECUTE format('ALTER ROLE %I WITH PASSWORD %L', '${SUPERUSER}', '${SUPERUSER_PASS}');
    END IF;
END
\$\$;

-- Create replicator if not exists, or update password
DO \$\$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${REPL_USER}') THEN
        EXECUTE format('CREATE ROLE %I WITH REPLICATION LOGIN PASSWORD %L', '${REPL_USER}', '${REPL_PASS}');
    ELSE
        EXECUTE format('ALTER ROLE %I WITH PASSWORD %L', '${REPL_USER}', '${REPL_PASS}');
    END IF;
END
\$\$;
EOSQL

echo "Post-bootstrap: users created"

# Generate SSL certificates
echo "Post-bootstrap: generating SSL certificates..."
bash /docker-entrypoint-initdb.d/init-ssl.sh

# Mark bootstrap as complete - patroni-runner.sh checks for this marker
# to distinguish complete bootstrap from stale/failed data
touch "${RAILWAY_VOLUME_MOUNT_PATH:-/var/lib/postgresql/data}/.patroni_bootstrap_complete"

echo "Post-bootstrap completed"

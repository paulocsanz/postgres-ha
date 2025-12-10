#!/bin/bash
# post_bootstrap.sh - Runs ONCE after PostgreSQL initialization on primary
#
# Patroni 4.0+ removed bootstrap.users, so users must be created/updated here.
# Patroni passes:
#   $1 = connection string URL (not used - we connect via Unix socket)
#   PGPASSFILE = path to pgpass file for authentication

set -e

echo "Post-bootstrap: starting..."

PATRONI_CONFIG="/tmp/patroni.yml"
VOLUME_ROOT="${RAILWAY_VOLUME_MOUNT_PATH:-/var/lib/postgresql/data}"

if [ ! -f "$PATRONI_CONFIG" ]; then
    echo "ERROR: Patroni config not found at $PATRONI_CONFIG"
    exit 1
fi

# Extract value from YAML: get_yaml_value 'section' 'key'
# Looks for "section:" then finds "key:" within next 2 lines
get_yaml_value() {
    grep -A2 "$1:" "$PATRONI_CONFIG" | grep "$2:" | head -1 | sed 's/.*: *//' | sed 's/^["'"'"']//' | sed 's/["'"'"']$//'
}

SUPERUSER=$(get_yaml_value 'superuser' 'username')
SUPERUSER_PASS=$(get_yaml_value 'superuser' 'password')
REPL_USER=$(get_yaml_value 'replication' 'username')
REPL_PASS=$(get_yaml_value 'replication' 'password')
APP_USER=$(get_yaml_value 'app_user' 'username')
APP_PASS=$(get_yaml_value 'app_user' 'password')

echo "DEBUG: SUPERUSER=$SUPERUSER"
echo "DEBUG: REPL_USER=$REPL_USER"
echo "DEBUG: REPL_PASS length=${#REPL_PASS}"
echo "DEBUG: REPL_PASS first4=${REPL_PASS:0:4} last4=${REPL_PASS: -4}"

# Validate required credentials
if [ -z "$SUPERUSER" ]; then
    echo "ERROR: Could not extract SUPERUSER from config"
    exit 1
fi
if [ -z "$REPL_USER" ]; then
    echo "ERROR: Could not extract REPL_USER from config"
    exit 1
fi
if [ -z "$REPL_PASS" ]; then
    echo "ERROR: Could not extract REPL_PASS from config"
    exit 1
fi

# Use Unix socket with superuser (trust auth for local)
PSQL="psql -h /var/run/postgresql -U $SUPERUSER -d postgres"

# 1. Set superuser password
echo "Setting superuser password..."
$PSQL -c "ALTER ROLE \"$SUPERUSER\" WITH PASSWORD '$(echo "$SUPERUSER_PASS" | sed "s/'/''/g")'"
echo "Superuser password set"

# 2. Create replicator user (Patroni 4.0+ no longer creates it automatically)
echo "Creating replicator user..."
$PSQL -c "CREATE ROLE \"$REPL_USER\" WITH REPLICATION LOGIN PASSWORD '$(echo "$REPL_PASS" | sed "s/'/''/g")'"
echo "Replicator user created"

# 3. VERIFY: Show password hash type (should be SCRAM-SHA-256$...)
echo "Verifying password storage..."
$PSQL -c "SELECT rolname, LEFT(rolpassword, 14) as hash_prefix FROM pg_authid WHERE rolname IN ('$SUPERUSER', '$REPL_USER')"

# 4. CRITICAL TEST: Verify replicator can authenticate via TCP (uses pg_hba rules, not Unix socket trust)
echo "Testing replicator authentication via TCP..."
if PGPASSWORD="$REPL_PASS" psql -h 127.0.0.1 -U "$REPL_USER" -d postgres -c "SELECT 1 as auth_test" 2>&1; then
    echo "SUCCESS: Replicator authentication test PASSED"
else
    echo "ERROR: Replicator authentication test FAILED!"
    echo "--- pg_hba.conf ---"
    cat "${PGDATA}/pg_hba.conf" 2>/dev/null || echo "(could not read pg_hba.conf)"
    echo "--- END ---"
    # Don't exit - let bootstrap complete so we can see full logs
fi

# 5. Create/update app user if different from superuser
if [ -n "$APP_USER" ] && [ "$APP_USER" != "$SUPERUSER" ] && [ -n "$APP_PASS" ]; then
    echo "Setting up app user: $APP_USER"
    $PSQL -c "DO \$\$ BEGIN
        IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$APP_USER') THEN
            CREATE ROLE \"$APP_USER\" WITH LOGIN PASSWORD '$(echo "$APP_PASS" | sed "s/'/''/g")';
            RAISE NOTICE 'Created app user: $APP_USER';
        ELSE
            ALTER ROLE \"$APP_USER\" WITH PASSWORD '$(echo "$APP_PASS" | sed "s/'/''/g")';
            RAISE NOTICE 'Updated app user password: $APP_USER';
        END IF;
    END \$\$"
fi

# Generate SSL certificates
echo "Generating SSL certificates..."
bash /docker-entrypoint-initdb.d/init-ssl.sh

# Mark bootstrap complete
touch "$VOLUME_ROOT/.patroni_bootstrap_complete"

echo "Post-bootstrap: completed successfully"

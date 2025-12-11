#!/bin/bash
set -e

# Source Bitnami's helper functions
. /opt/bitnami/scripts/libos.sh

info "** Starting Pgpool-II setup **"

# Validate required environment variables
if [ -z "$PGPOOL_BACKEND_NODES" ]; then
    error "PGPOOL_BACKEND_NODES must be set"
    exit 1
fi

if [ -z "$PGPOOL_POSTGRES_PASSWORD" ]; then
    error "PGPOOL_POSTGRES_PASSWORD must be set"
    exit 1
fi

# Create necessary directories
mkdir -p /opt/bitnami/pgpool/conf /opt/bitnami/pgpool/etc /var/run/pgpool /opt/bitnami/pgpool/tmp /opt/bitnami/pgpool/logs
chmod 755 /var/run/pgpool

# Start with our template config
cp /opt/pgpool.conf.template /opt/bitnami/pgpool/conf/pgpool.conf

# Generate backend configuration from PGPOOL_BACKEND_NODES env var
# Format: "0:hostname1:5432,1:hostname2:5432,2:hostname3:5432"
info "Generating backend configuration from PGPOOL_BACKEND_NODES: $PGPOOL_BACKEND_NODES"
BACKEND_CONFIG=""
IFS=',' read -ra NODES <<< "$PGPOOL_BACKEND_NODES"
for node in "${NODES[@]}"; do
    IFS=':' read -r index host port <<< "$node"
    # Extract name from hostname (e.g., postgres-1-abc.railway.internal -> postgres-1-abc)
    name="${host%.railway.internal}"
    info "  Backend $index: $host:$port ($name)"
    BACKEND_CONFIG+="
backend_hostname${index} = '${host}'
backend_port${index} = ${port}
backend_weight${index} = 1
backend_flag${index} = 'ALLOW_TO_FAILOVER'
backend_application_name${index} = '${name}'
"
done

# Append backend config to pgpool.conf
echo "$BACKEND_CONFIG" >> /opt/bitnami/pgpool/conf/pgpool.conf
info "Backend configuration generated"

# Set password and user configurations by appending to config
cat >> /opt/bitnami/pgpool/conf/pgpool.conf <<EOF

# User/password configuration (generated at runtime)
health_check_user = '${PGPOOL_HEALTH_CHECK_USER:-postgres}'
health_check_password = '${PGPOOL_HEALTH_CHECK_PASSWORD:-$PGPOOL_POSTGRES_PASSWORD}'
sr_check_user = '${PGPOOL_SR_CHECK_USER:-replicator}'
sr_check_password = '${PGPOOL_SR_CHECK_PASSWORD:-$PGPOOL_POSTGRES_PASSWORD}'
recovery_user = '${PGPOOL_POSTGRES_USERNAME:-postgres}'
recovery_password = '${PGPOOL_POSTGRES_PASSWORD}'
EOF

# Create pool_hba.conf
cat > /opt/bitnami/pgpool/conf/pool_hba.conf <<'POOL_HBA_EOF'
local   all         all                               trust
host    all         all         0.0.0.0/0             md5
host    all         all         ::0/0                 md5
POOL_HBA_EOF

# Generate pool_passwd file for client authentication
# MD5 format: md5 + md5(password + username)
info "Generating pool_passwd..."
POOL_PASSWD_FILE="/opt/bitnami/pgpool/conf/pool_passwd"
: > "$POOL_PASSWD_FILE"

# Add app user (main user for connections)
APP_USER="${PGPOOL_POSTGRES_USERNAME:-railway}"
APP_HASH=$(printf '%s' "${PGPOOL_POSTGRES_PASSWORD}${APP_USER}" | md5sum | cut -d' ' -f1)
echo "${APP_USER}:md5${APP_HASH}" >> "$POOL_PASSWD_FILE"
info "  Added user: ${APP_USER}"

# Add health check user if different from app user
HEALTH_USER="${PGPOOL_HEALTH_CHECK_USER:-postgres}"
if [ "$HEALTH_USER" != "$APP_USER" ]; then
    HEALTH_PASS="${PGPOOL_HEALTH_CHECK_PASSWORD:-$PGPOOL_POSTGRES_PASSWORD}"
    HEALTH_HASH=$(printf '%s' "${HEALTH_PASS}${HEALTH_USER}" | md5sum | cut -d' ' -f1)
    echo "${HEALTH_USER}:md5${HEALTH_HASH}" >> "$POOL_PASSWD_FILE"
    info "  Added user: ${HEALTH_USER}"
fi

# Add SR check user (replicator)
SR_USER="${PGPOOL_SR_CHECK_USER:-replicator}"
if [ -n "$PGPOOL_SR_CHECK_PASSWORD" ] && [ "$SR_USER" != "$APP_USER" ] && [ "$SR_USER" != "$HEALTH_USER" ]; then
    SR_HASH=$(printf '%s' "${PGPOOL_SR_CHECK_PASSWORD}${SR_USER}" | md5sum | cut -d' ' -f1)
    echo "${SR_USER}:md5${SR_HASH}" >> "$POOL_PASSWD_FILE"
    info "  Added user: ${SR_USER}"
fi

chmod 600 "$POOL_PASSWD_FILE"

# Generate pcp.conf for admin authentication
USERNAME="${PGPOOL_ADMIN_USERNAME:-admin}"
ADMIN_HASH=$(printf '%s' "${PGPOOL_ADMIN_PASSWORD:-$PGPOOL_POSTGRES_PASSWORD}" | md5sum | cut -d' ' -f1)
echo "${USERNAME}:${ADMIN_HASH}" > /opt/bitnami/pgpool/conf/pcp.conf
chmod 600 /opt/bitnami/pgpool/conf/pcp.conf

# Create pcppass file for PCP client authentication
cat > /tmp/.pcppass <<EOF
localhost:9898:${USERNAME}:${PGPOOL_ADMIN_PASSWORD:-$PGPOOL_POSTGRES_PASSWORD}
*:9898:${USERNAME}:${PGPOOL_ADMIN_PASSWORD:-$PGPOOL_POSTGRES_PASSWORD}
EOF
chmod 600 /tmp/.pcppass

info "** Pgpool-II setup finished! **"

# Start patroni watcher in background
python3 /opt/patroni-watcher.py &

# Start pgpool
info "** Starting Pgpool-II **"
exec /opt/bitnami/pgpool/bin/pgpool -n -f /opt/bitnami/pgpool/conf/pgpool.conf -F /opt/bitnami/pgpool/conf/pcp.conf

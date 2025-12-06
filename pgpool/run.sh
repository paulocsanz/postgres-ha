#!/bin/bash
set -e

# Source Bitnami's setup functions and run initialization
. /opt/bitnami/scripts/libpgpool.sh
. /opt/bitnami/scripts/libos.sh

info "** Starting Pgpool-II setup **"
/opt/bitnami/scripts/pgpool/setup.sh
info "** Pgpool-II setup finished! **"

# Restore pool_hba.conf (Bitnami setup may overwrite it)
mkdir -p /opt/bitnami/pgpool/etc
cat > /opt/bitnami/pgpool/etc/pool_hba.conf <<'POOL_HBA_EOF'
local   all         all                               trust
host    all         all         0.0.0.0/0             md5
host    all         all         ::0/0                 md5
POOL_HBA_EOF

# Generate pcp.conf (pg_md5 is unreliable, use md5sum)
USERNAME="${PGPOOL_ADMIN_USERNAME:-admin}"
HASH=$(printf '%s' "${PGPOOL_ADMIN_PASSWORD}" | md5sum | cut -d' ' -f1)
echo "${USERNAME}:${HASH}" > /opt/bitnami/pgpool/conf/pcp.conf
chmod 600 /opt/bitnami/pgpool/conf/pcp.conf

# Create pcppass file for PCP client authentication
cat > /tmp/.pcppass <<EOF
localhost:9898:${USERNAME}:${PGPOOL_ADMIN_PASSWORD}
*:9898:${USERNAME}:${PGPOOL_ADMIN_PASSWORD}
EOF
chmod 600 /tmp/.pcppass

# Start patroni watcher in background
python3 /opt/patroni-watcher.py &

# Start pgpool
info "** Starting Pgpool-II **"
exec "${PGPOOL_BIN_DIR}/pgpool" -n -f "${PGPOOL_CONF_DIR}/pgpool.conf" -F "${PGPOOL_CONF_DIR}/pcp.conf"

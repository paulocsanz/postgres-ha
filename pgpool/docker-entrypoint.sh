#!/bin/bash
set -e

POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-postgres}"
REPLICATION_PASSWORD="${REPLICATION_PASSWORD:-replicator_password}"

cat > /etc/pgpool-II/pcp.conf <<EOF
postgres:$(pg_md5 ${POSTGRES_PASSWORD})
EOF

sed -i "s|sr_check_password = ''|sr_check_password = '${REPLICATION_PASSWORD}'|g" /etc/pgpool-II/pgpool.conf
sed -i "s|health_check_password = ''|health_check_password = '${POSTGRES_PASSWORD}'|g" /etc/pgpool-II/pgpool.conf
sed -i "s|recovery_password = ''|recovery_password = '${POSTGRES_PASSWORD}'|g" /etc/pgpool-II/pgpool.conf

if [ -n "${PGPOOL_NUM_INIT_CHILDREN}" ]; then
  sed -i "s|num_init_children = 32|num_init_children = ${PGPOOL_NUM_INIT_CHILDREN}|g" /etc/pgpool-II/pgpool.conf
fi

if [ -n "${PGPOOL_MAX_POOL}" ]; then
  sed -i "s|max_pool = 4|max_pool = ${PGPOOL_MAX_POOL}|g" /etc/pgpool-II/pgpool.conf
fi

mkdir -p /var/run/pgpool
chown postgres:postgres /var/run/pgpool

echo "Pgpool-II configuration:"
echo "  Backends: postgres-1, postgres-2, postgres-3"
echo "  num_init_children: $(grep num_init_children /etc/pgpool-II/pgpool.conf | grep -v '#' | awk '{print $3}')"
echo "  max_pool: $(grep 'max_pool = ' /etc/pgpool-II/pgpool.conf | grep -v '#' | awk '{print $3}')"

exec "$@"

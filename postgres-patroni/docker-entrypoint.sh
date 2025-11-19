#!/bin/bash
set -e

export PATRONI_SCOPE="${PATRONI_SCOPE:-pg-ha-cluster}"
export PATRONI_NAME="${PATRONI_NAME:-postgres-1}"
export PATRONI_ETCD_HOSTS="${PATRONI_ETCD_HOSTS:-etcd-1.railway.internal:2379,etcd-2.railway.internal:2379,etcd-3.railway.internal:2379}"

export PATRONI_SUPERUSER_USERNAME="${POSTGRES_USER:-postgres}"
export PATRONI_SUPERUSER_PASSWORD="${POSTGRES_PASSWORD:-postgres}"
export PATRONI_REPLICATION_USERNAME="${PATRONI_REPLICATION_USERNAME:-replicator}"
export PATRONI_REPLICATION_PASSWORD="${PATRONI_REPLICATION_PASSWORD:-replicator_password}"

export POSTGRESQL_DATA_DIR="${PGDATA:-/var/lib/postgresql/data}"
export PATRONI_TTL="${PATRONI_TTL:-30}"
export PATRONI_LOOP_WAIT="${PATRONI_LOOP_WAIT:-10}"

mkdir -p "${POSTGRESQL_DATA_DIR}"
chmod 0700 "${POSTGRESQL_DATA_DIR}"

echo "Starting Patroni with:"
echo "  Scope: ${PATRONI_SCOPE}"
echo "  Name: ${PATRONI_NAME}"
echo "  etcd: ${PATRONI_ETCD_HOSTS}"
echo "  Data dir: ${POSTGRESQL_DATA_DIR}"

exec "$@"

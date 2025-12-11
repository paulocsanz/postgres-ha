#!/bin/bash
set -e

echo "Setting up shared environment variables..."
echo ""

# Generate secure passwords
POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -d "=+/" | cut -c1-25)
REPLICATION_PASSWORD=$(openssl rand -base64 32 | tr -d "=+/" | cut -c1-25)

echo "Generated secure passwords"
echo ""

# Set shared variables
echo "Setting POSTGRES_USER..."
railway variables --set POSTGRES_USER=railway

echo "Setting POSTGRES_PASSWORD..."
railway variables --set POSTGRES_PASSWORD="$POSTGRES_PASSWORD"

echo "Setting POSTGRES_DB..."
railway variables --set POSTGRES_DB=railway

echo "Setting PATRONI_REPLICATION_USERNAME..."
railway variables --set PATRONI_REPLICATION_USERNAME=replicator

echo "Setting PATRONI_REPLICATION_PASSWORD..."
railway variables --set PATRONI_REPLICATION_PASSWORD="$REPLICATION_PASSWORD"

echo "Setting PATRONI_SCOPE..."
railway variables --set PATRONI_SCOPE=pg-ha-cluster

echo "Setting PATRONI_TTL..."
railway variables --set PATRONI_TTL=30

echo "Setting PATRONI_LOOP_WAIT..."
railway variables --set PATRONI_LOOP_WAIT=10

echo "Setting PATRONI_ETCD3_HOSTS..."
railway variables --set PATRONI_ETCD3_HOSTS="etcd-1.railway.internal:2379,etcd-2.railway.internal:2379,etcd-3.railway.internal:2379"

echo ""
echo "✅ Shared variables set!"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "⚠️  IMPORTANT: Save these credentials!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "POSTGRES_USER: railway"
echo "POSTGRES_PASSWORD: $POSTGRES_PASSWORD"
echo "POSTGRES_DB: railway"
echo "REPLICATION_PASSWORD: $REPLICATION_PASSWORD"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

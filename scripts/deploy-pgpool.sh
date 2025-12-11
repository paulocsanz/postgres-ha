#!/bin/bash
set -e

cd pgpool

echo "Creating pgpool service..."
railway service create pgpool 2>/dev/null || echo "Service pgpool already exists"

echo "Setting pgpool variables..."
# Backend nodes configuration
railway variables --service pgpool --set 'PGPOOL_BACKEND_NODES=0:${{postgres-1.RAILWAY_PRIVATE_DOMAIN}}:5432,1:${{postgres-2.RAILWAY_PRIVATE_DOMAIN}}:5432,2:${{postgres-3.RAILWAY_PRIVATE_DOMAIN}}:5432'

# Postgres password for health checks (health_check_user = postgres)
railway variables --service pgpool --set 'PGPOOL_POSTGRES_PASSWORD=${{shared.POSTGRES_PASSWORD}}'

# Replication password for SR checks (sr_check_user = replicator)
railway variables --service pgpool --set 'PGPOOL_SR_CHECK_PASSWORD=${{shared.PATRONI_REPLICATION_PASSWORD}}'

# Admin password for PCP commands (used by patroni-watcher)
railway variables --service pgpool --set 'PGPOOL_ADMIN_USERNAME=admin'
railway variables --service pgpool --set 'PGPOOL_ADMIN_PASSWORD=${{shared.POSTGRES_PASSWORD}}'

echo "Deploying pgpool..."
railway up --service pgpool --detach

cd ..
echo "✅ pgpool deployed"
echo ""
echo "⚠️  Note: Set numReplicas to 3 in Railway dashboard for HA"
echo "   Settings → Deploy → Replicas → 3"

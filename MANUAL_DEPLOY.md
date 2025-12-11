# Manual Deployment Steps

Run these commands **one at a time** in your terminal from the `templates/postgres-ha` directory.

## Important: etcd Crash Fix

The etcd crash is likely because the advertise URLs use `railway.internal` which doesn't exist until Railway's private networking is set up. We need to use container hostnames instead.

## Step 1: Deploy etcd-1

```bash
cd etcd-1

# Create service (select "Create a new service" when prompted)
railway service

# Set variables
railway variables set ETCD_NAME=etcd-1
railway variables set ETCD_INITIAL_CLUSTER="etcd-1=http://etcd-1:2380,etcd-2=http://etcd-2:2380,etcd-3=http://etcd-3:2380"
railway variables set ETCD_INITIAL_CLUSTER_STATE=new
railway variables set ETCD_INITIAL_CLUSTER_TOKEN=railway-pg-ha
railway variables set ETCD_LISTEN_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_ADVERTISE_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_LISTEN_PEER_URLS="http://0.0.0.0:2380"
railway variables set ETCD_INITIAL_ADVERTISE_PEER_URLS="http://etcd-1:2380"
railway variables set ETCD_DATA_DIR=/etcd-data

# Deploy
railway up --detach

cd ..
```

## Step 2: Deploy etcd-2

```bash
cd etcd-2

railway service  # Create "etcd-2"

railway variables set ETCD_NAME=etcd-2
railway variables set ETCD_INITIAL_CLUSTER="etcd-1=http://etcd-1:2380,etcd-2=http://etcd-2:2380,etcd-3=http://etcd-3:2380"
railway variables set ETCD_INITIAL_CLUSTER_STATE=new
railway variables set ETCD_INITIAL_CLUSTER_TOKEN=railway-pg-ha
railway variables set ETCD_LISTEN_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_ADVERTISE_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_LISTEN_PEER_URLS="http://0.0.0.0:2380"
railway variables set ETCD_INITIAL_ADVERTISE_PEER_URLS="http://etcd-2:2380"
railway variables set ETCD_DATA_DIR=/etcd-data

railway up --detach

cd ..
```

## Step 3: Deploy etcd-3

```bash
cd etcd-3

railway service  # Create "etcd-3"

railway variables set ETCD_NAME=etcd-3
railway variables set ETCD_INITIAL_CLUSTER="etcd-1=http://etcd-1:2380,etcd-2=http://etcd-2:2380,etcd-3=http://etcd-3:2380"
railway variables set ETCD_INITIAL_CLUSTER_STATE=new
railway variables set ETCD_INITIAL_CLUSTER_TOKEN=railway-pg-ha
railway variables set ETCD_LISTEN_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_ADVERTISE_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_LISTEN_PEER_URLS="http://0.0.0.0:2380"
railway variables set ETCD_INITIAL_ADVERTISE_PEER_URLS="http://etcd-3:2380"
railway variables set ETCD_DATA_DIR=/etcd-data

railway up --detach

cd ..
```

## Wait and Verify

After deploying all 3 etcd nodes, wait 30 seconds then check logs:

```bash
railway logs --service etcd-1
```

Look for: `etcd cluster is ready` or `health check passed`

---

## If etcd Still Crashes

The issue might be that Railway needs service discovery names. Try this alternative in Railway Dashboard:

1. Go to each etcd service → Settings → Networking
2. Enable "Private Networking"
3. Note the internal hostname (should be like `etcd-1.railway.internal`)

Then update the variables to use full domains:
- `ETCD_INITIAL_CLUSTER="etcd-1=http://etcd-1.railway.internal:2380,etcd-2=http://etcd-2.railway.internal:2380,etcd-3=http://etcd-3.railway.internal:2380"`
- `ETCD_ADVERTISE_CLIENT_URLS="http://etcd-1.railway.internal:2379"` (change per service)
- `ETCD_INITIAL_ADVERTISE_PEER_URLS="http://etcd-1.railway.internal:2380"` (change per service)

---

## Alternative: Single-Node Setup for Testing

If you just want to test the system quickly, you can start with a single-node setup:

### Simple etcd (1 node only)

```bash
cd etcd-1
railway service  # Create "etcd"

railway variables set ETCD_NAME=etcd
railway variables set ETCD_LISTEN_CLIENT_URLS="http://0.0.0.0:2379"
railway variables set ETCD_ADVERTISE_CLIENT_URLS="http://etcd.railway.internal:2379"
railway variables set ETCD_DATA_DIR=/etcd-data

railway up --detach
```

Then update PostgreSQL to use single etcd:
```bash
# In shared variables or per postgres service
PATRONI_ETCD3_HOSTS=etcd.railway.internal:2379
```

This is NOT HA but will let you test the rest of the system.

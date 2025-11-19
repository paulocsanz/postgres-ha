# Template Structure

```
templates/postgres-ha/
├── README.md                          # Main documentation
├── docker-compose.yml                 # Local testing setup
├── .gitignore
│
├── postgres-patroni/                  # PostgreSQL + Patroni service (deploy 3x)
│   ├── Dockerfile                     # Custom image with PG 17 + Patroni
│   ├── railway.toml                   # Railway service config (1 replica)
│   ├── patroni.yml                    # Patroni configuration template
│   └── docker-entrypoint.sh           # Initialization script
│
├── etcd-1/                            # etcd node 1
│   ├── Dockerfile                     # etcd v3.5.16 image
│   └── railway.toml                   # Service config with node-1 settings
│
├── etcd-2/                            # etcd node 2
│   ├── Dockerfile
│   └── railway.toml                   # Service config with node-2 settings
│
├── etcd-3/                            # etcd node 3
│   ├── Dockerfile
│   └── railway.toml                   # Service config with node-3 settings
│
├── pgpool/                            # Pgpool-II connection pooler
│   ├── Dockerfile                     # Pgpool 4.5 alpine image
│   ├── railway.toml                   # Service config (3 replicas for HA)
│   ├── pgpool.conf                    # Pgpool configuration
│   ├── pool_hba.conf                  # Host-based authentication
│   └── docker-entrypoint.sh           # Password injection script
│
└── failover-watcher/                  # Failover monitoring service
    ├── Dockerfile                     # Node.js 20 alpine
    ├── railway.toml                   # Service config (1 replica)
    ├── package.json                   # Node.js dependencies
    └── src/
        └── index.js                   # Watcher implementation
```

## Deployment Instructions

### For Railway (Production)

Each subfolder represents a separate Railway service. Deploy in this order:

1. **etcd-1, etcd-2, etcd-3** - Start these first (consensus layer)
2. **postgres-patroni** - Deploy 3 times with different names:
   - Service 1: Set `PATRONI_NAME=postgres-1`
   - Service 2: Set `PATRONI_NAME=postgres-2`
   - Service 3: Set `PATRONI_NAME=postgres-3`
3. **pgpool** - Deploy with `numReplicas=3` in railway.toml
4. **failover-watcher** - Deploy last (optional)

### For Local Testing

```bash
cd templates/postgres-ha
docker-compose up -d
```

Connects to: `postgresql://railway:railway@localhost:5432/railway`

## File Descriptions

### postgres-patroni/

- **Dockerfile**: Builds custom image with PostgreSQL 17 + Patroni 4.0.4
- **patroni.yml**: Patroni config using env var templating for dynamic values
- **docker-entrypoint.sh**: Sets default env vars and initializes data directory
- **railway.toml**: Defines build (dockerfile) and deploy settings (1 replica, always restart)

### etcd-1/, etcd-2/, etcd-3/

- **Dockerfile**: Uses official etcd image with health check
- **railway.toml**: Each has unique ETCD_NAME and advertise URLs
  - Hardcoded URLs use Railway's private networking: `etcd-{1,2,3}.railway.internal`

### pgpool/

- **Dockerfile**: Pgpool 4.5 alpine with custom configs
- **pgpool.conf**: Backend configuration pointing to postgres-{1,2,3}.railway.internal
  - Load balancing enabled
  - Streaming replication check enabled
  - Health checks every 5 seconds
- **pool_hba.conf**: Trust local, md5 for network connections
- **docker-entrypoint.sh**: Injects passwords from env vars into config
- **railway.toml**: **3 replicas** for horizontal scaling

### failover-watcher/

- **package.json**: Node.js project with node-fetch dependency
- **index.js**:
  - Polls Patroni cluster API every 5 seconds
  - Detects leader changes
  - Updates Railway env vars via GraphQL API
  - Logs failover events
- **railway.toml**: 1 replica (stateless, but no benefit to scaling)

## Key Configuration Points

### Private Networking

All services use Railway's private networking for service-to-service communication:

```
postgres-1.railway.internal:5432
postgres-2.railway.internal:5432
postgres-3.railway.internal:5432
etcd-1.railway.internal:2379
etcd-2.railway.internal:2379
etcd-3.railway.internal:2379
pgpool.railway.internal:5432
```

### Shared Variables

These must be set as shared environment variables in Railway:

- `POSTGRES_USER` (default: railway)
- `POSTGRES_PASSWORD` (auto-generate secure password)
- `POSTGRES_DB` (default: railway)
- `PATRONI_REPLICATION_PASSWORD` (auto-generate)

### Per-Service Variables

**postgres-patroni** (set individually per instance):
- `PATRONI_NAME`: postgres-1, postgres-2, or postgres-3

**All other services**: Use shared variables via Railway's variable referencing:
- `${shared.POSTGRES_PASSWORD}`
- `${shared.PATRONI_REPLICATION_PASSWORD}`

## Build Process

1. Railway reads `railway.toml` from each service folder
2. Builds Docker image using specified Dockerfile
3. Deploys with configured replicas and restart policy
4. Injects environment variables
5. Starts container with configured start command
6. Monitors via health checks

## Next Steps

1. Test locally with `docker-compose up`
2. Build images and push to registry (or Railway builds automatically)
3. Create Railway template definition JSON
4. Submit template to Railway marketplace

FROM railwayapp/pgpool:4

USER root

# Force rebuild 2025-11-27-03-30

# Don't try to use ENV vars to disable health checks - Bitnami ignores them
# We'll modify pgpool.conf directly before starting pgpool
ENV PGPOOL_ENABLE_POOL_HBA=yes

# Install Python and dependencies for patroni-watcher
RUN apt-get update && apt-get install -y \
    python3 \
    python3-pip \
    python3-requests \
    netcat-openbsd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Don't copy custom config - let Bitnami generate from env vars
COPY pool_hba.conf /opt/bitnami/pgpool/conf/pool_hba.conf
COPY patroni-watcher.py /usr/local/bin/patroni-watcher.py
RUN chmod +x /usr/local/bin/patroni-watcher.py

# Create custom run script that modifies configs and starts services
# This runs AFTER Bitnami's entrypoint creates the config files
COPY <<'EOF' /opt/bitnami/scripts/pgpool/run-with-watcher.sh
#!/bin/bash
set -e

echo "[run-with-watcher] Starting custom pgpool runner..." >&2

# At this point, Bitnami's entrypoint has already created configs from env vars
PGPOOL_CONF="/opt/bitnami/pgpool/conf/pgpool.conf"

# Wait briefly for config to be fully written
sleep 2

if [ -f "$PGPOOL_CONF" ]; then
  echo "[run-with-watcher] Found pgpool.conf, modifying to disable health checks..." >&2

  # Disable health check by setting period to 0
  sed -i 's/^health_check_period = .*/health_check_period = 0/' "$PGPOOL_CONF"

  # Disable sr_check by setting period to 0
  sed -i 's/^sr_check_period = .*/sr_check_period = 0/' "$PGPOOL_CONF"

  echo "[run-with-watcher] Config modifications complete" >&2
  echo "[run-with-watcher] Health check period: $(grep '^health_check_period' $PGPOOL_CONF)" >&2
  echo "[run-with-watcher] SR check period: $(grep '^sr_check_period' $PGPOOL_CONF)" >&2
else
  echo "[run-with-watcher] ERROR: pgpool.conf not found at $PGPOOL_CONF" >&2
  echo "[run-with-watcher] Listing /opt/bitnami/pgpool/conf:" >&2
  ls -la /opt/bitnami/pgpool/conf/ >&2 || true
  exit 1
fi

# Configure PCP admin user
PCP_CONF="/opt/bitnami/pgpool/conf/pcp.conf"
if [ -f "$PCP_CONF" ] && [ -n "${PGPOOL_ADMIN_USERNAME}" ] && [ -n "${PGPOOL_ADMIN_PASSWORD}" ]; then
  echo "[run-with-watcher] Adding admin user to pcp.conf..." >&2

  # Generate MD5 hash for PCP (format: md5<md5(password+username)>)
  HASH=$(echo -n "${PGPOOL_ADMIN_PASSWORD}${PGPOOL_ADMIN_USERNAME}" | md5sum | cut -d' ' -f1)
  echo "${PGPOOL_ADMIN_USERNAME}:md5${HASH}" >> "$PCP_CONF"

  echo "[run-with-watcher] Admin user added to pcp.conf" >&2
else
  echo "[run-with-watcher] WARNING: Could not configure PCP admin user" >&2
  echo "[run-with-watcher]   PCP_CONF exists: $([ -f "$PCP_CONF" ] && echo yes || echo no)" >&2
  echo "[run-with-watcher]   PGPOOL_ADMIN_USERNAME: ${PGPOOL_ADMIN_USERNAME:-not set}" >&2
fi

# Start patroni-watcher in background
echo "[run-with-watcher] Starting patroni-watcher..." >&2
python3 /usr/local/bin/patroni-watcher.py &
WATCHER_PID=$!
echo "[run-with-watcher] patroni-watcher started with PID $WATCHER_PID" >&2

# Trap to kill watcher on exit
cleanup() {
  echo "[run-with-watcher] Shutting down, killing watcher PID $WATCHER_PID..." >&2
  kill $WATCHER_PID 2>/dev/null || true
}
trap cleanup SIGTERM SIGINT EXIT

# Start pgpool with modified config
echo "[run-with-watcher] Starting pgpool..." >&2
exec /opt/bitnami/scripts/pgpool/run.sh
EOF

RUN chmod +x /opt/bitnami/scripts/pgpool/run-with-watcher.sh

USER 1001

# Use Bitnami's default entrypoint (don't override it)
# Our custom run script will execute after entrypoint creates configs
CMD ["/opt/bitnami/scripts/pgpool/run-with-watcher.sh"]

EXPOSE 5432 9999

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s \
  CMD pgpool -h localhost -p 5432 -c "SELECT 1" || exit 1

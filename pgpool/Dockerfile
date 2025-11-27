FROM railwayapp/pgpool:4

USER root

# Force rebuild 2025-11-27-02-47

# Don't try to use ENV vars to disable health checks - Bitnami ignores them
# We'll modify pgpool.conf directly in the entrypoint wrapper instead
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

# Create wrapper entrypoint that modifies config files directly
COPY <<'EOF' /entrypoint-wrapper.sh
#!/bin/bash
set -e

echo "[entrypoint] Starting pgpool wrapper..." >&2

# Workaround: Railway sometimes doesn't resolve PGPOOL_HEALTH_CHECK_PASSWORD
if [ -z "${PGPOOL_HEALTH_CHECK_PASSWORD}" ] && [ -n "${PGPOOL_POSTGRES_PASSWORD}" ]; then
  echo "[entrypoint] Copying PGPOOL_POSTGRES_PASSWORD to PGPOOL_HEALTH_CHECK_PASSWORD" >&2
  export PGPOOL_HEALTH_CHECK_PASSWORD="${PGPOOL_POSTGRES_PASSWORD}"
fi

# Run Bitnami's setup (but not run.sh yet)
echo "[entrypoint] Running Bitnami setup..." >&2
/opt/bitnami/scripts/pgpool/entrypoint.sh echo "Setup complete"

# Now modify pgpool.conf directly to disable health checks and sr_check
PGPOOL_CONF="/opt/bitnami/pgpool/conf/pgpool.conf"
if [ -f "$PGPOOL_CONF" ]; then
  echo "[entrypoint] Modifying $PGPOOL_CONF to disable health checks and sr_check..." >&2

  # Disable health check by setting period to 0
  sed -i 's/^health_check_period = .*/health_check_period = 0/' "$PGPOOL_CONF"

  # Disable sr_check by setting period to 0
  sed -i 's/^sr_check_period = .*/sr_check_period = 0/' "$PGPOOL_CONF"

  echo "[entrypoint] Config modifications complete" >&2
  echo "[entrypoint] Health check period: $(grep '^health_check_period' $PGPOOL_CONF)" >&2
  echo "[entrypoint] SR check period: $(grep '^sr_check_period' $PGPOOL_CONF)" >&2
else
  echo "[entrypoint] WARNING: pgpool.conf not found at $PGPOOL_CONF" >&2
fi

# Find and modify pcp.conf to add admin user
echo "[entrypoint] Searching for pcp.conf..." >&2
PCP_CONF=""
for path in /opt/bitnami/pgpool/conf/pcp.conf /opt/bitnami/pgpool/etc/pcp.conf /etc/pgpool-II/pcp.conf; do
  if [ -f "$path" ]; then
    PCP_CONF="$path"
    echo "[entrypoint] Found pcp.conf at $PCP_CONF" >&2
    break
  fi
done

if [ -n "$PCP_CONF" ] && [ -n "${PGPOOL_ADMIN_USERNAME}" ] && [ -n "${PGPOOL_ADMIN_PASSWORD}" ]; then
  echo "[entrypoint] Adding admin user to pcp.conf..." >&2
  echo "[entrypoint] Current pcp.conf contents:" >&2
  cat "$PCP_CONF" >&2

  # Generate MD5 hash for PCP (format: md5<md5(password+username)>)
  HASH=$(echo -n "${PGPOOL_ADMIN_PASSWORD}${PGPOOL_ADMIN_USERNAME}" | md5sum | cut -d' ' -f1)
  echo "${PGPOOL_ADMIN_USERNAME}:md5${HASH}" >> "$PCP_CONF"

  echo "[entrypoint] Admin user added. Updated pcp.conf:" >&2
  cat "$PCP_CONF" >&2
else
  echo "[entrypoint] WARNING: Could not configure PCP admin user" >&2
  echo "[entrypoint]   PCP_CONF=$PCP_CONF" >&2
  echo "[entrypoint]   PGPOOL_ADMIN_USERNAME=$PGPOOL_ADMIN_USERNAME" >&2
fi

# Start patroni-watcher in background
echo "[entrypoint] Starting patroni-watcher..." >&2
python3 /usr/local/bin/patroni-watcher.py &
WATCHER_PID=$!
echo "[entrypoint] patroni-watcher started with PID $WATCHER_PID" >&2

# Trap to kill watcher on exit
cleanup() {
  echo "[entrypoint] Shutting down, killing watcher PID $WATCHER_PID..." >&2
  kill $WATCHER_PID 2>/dev/null || true
}
trap cleanup SIGTERM SIGINT EXIT

# Now start pgpool with modified config
echo "[entrypoint] Starting pgpool..." >&2
exec /opt/bitnami/scripts/pgpool/run.sh
EOF

RUN chmod +x /entrypoint-wrapper.sh

USER 1001

ENTRYPOINT ["/entrypoint-wrapper.sh"]
CMD ["/opt/bitnami/scripts/pgpool/run.sh"]

EXPOSE 5432 9999

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s \
  CMD pgpool -h localhost -p 5432 -c "SELECT 1" || exit 1

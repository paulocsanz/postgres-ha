#!/bin/sh
# etcd bootstrap wrapper with leader-based startup
#
# Problem: etcd nodes starting at different times fail to form cluster because
# all nodes waiting for each other on TCP creates a deadlock. etcd also has
# hard timeouts that corrupt local state if quorum isn't reached.
#
# Solution: Single-node bootstrap with dynamic member addition
# 1. Determine bootstrap leader (alphabetically first node name)
# 2. Leader bootstraps single-node cluster (instant quorum)
# 3. Other nodes wait for leader, add themselves, then join existing cluster

DATA_DIR=${ETCD_DATA_DIR:-/etcd-data}
MAX_RETRIES=${ETCD_MAX_RETRIES:-60}
RETRY_DELAY=${ETCD_RETRY_DELAY:-5}
BOOTSTRAP_COMPLETE_MARKER="$DATA_DIR/.bootstrap_complete"
PEER_WAIT_TIMEOUT=${ETCD_PEER_WAIT_TIMEOUT:-300}
PEER_CHECK_INTERVAL=${ETCD_PEER_CHECK_INTERVAL:-5}

log() {
  echo "[$(date -Iseconds)] ENTRYPOINT: $1"
}

check_cluster_health() {
  etcdctl endpoint health --endpoints=http://127.0.0.1:2379 >/dev/null 2>&1
}

# Get bootstrap leader (alphabetically first node name)
get_bootstrap_leader() {
  echo "$ETCD_INITIAL_CLUSTER" | tr ',' '\n' | cut -d'=' -f1 | sort | head -1
}

# Get leader's client endpoint (port 2379)
get_leader_endpoint() {
  leader=$1
  # Extract leader's URL from ETCD_INITIAL_CLUSTER and convert peer port to client port
  entry=$(echo "$ETCD_INITIAL_CLUSTER" | tr ',' '\n' | grep "^${leader}=")
  url=$(echo "$entry" | cut -d'=' -f2)
  # Convert http://host:2380 to http://host:2379
  echo "$url" | sed 's/:2380/:2379/'
}

# Get leader's peer host:port for TCP check
get_leader_peer_host() {
  leader=$1
  entry=$(echo "$ETCD_INITIAL_CLUSTER" | tr ',' '\n' | grep "^${leader}=")
  url=$(echo "$entry" | cut -d'=' -f2)
  echo "$url" | sed 's|.*://||'
}

# Get my peer URL from ETCD_INITIAL_CLUSTER
get_my_peer_url() {
  entry=$(echo "$ETCD_INITIAL_CLUSTER" | tr ',' '\n' | grep "^${ETCD_NAME}=")
  echo "$entry" | cut -d'=' -f2
}

# Wait for leader to be healthy and accepting connections
wait_for_leader() {
  leader=$1
  endpoint=$(get_leader_endpoint "$leader")
  host_port=$(get_leader_peer_host "$leader")
  host=$(echo "$host_port" | cut -d':' -f1)
  port=$(echo "$host_port" | cut -d':' -f2)

  log "Waiting for bootstrap leader $leader at $endpoint..."

  elapsed=0
  while [ $elapsed -lt $PEER_WAIT_TIMEOUT ]; do
    # First check TCP connectivity
    if command -v nc >/dev/null 2>&1; then
      if nc -z -w2 "$host" "$port" >/dev/null 2>&1; then
        # Then check if cluster is actually healthy
        if etcdctl endpoint health --endpoints="$endpoint" >/dev/null 2>&1; then
          log "Bootstrap leader $leader is healthy"
          return 0
        else
          log "Leader $leader reachable but not healthy yet..."
        fi
      else
        log "Leader $leader not reachable yet (${elapsed}s/${PEER_WAIT_TIMEOUT}s)"
      fi
    else
      # Fallback without nc
      if etcdctl endpoint health --endpoints="$endpoint" >/dev/null 2>&1; then
        log "Bootstrap leader $leader is healthy"
        return 0
      else
        log "Leader $leader not healthy yet (${elapsed}s/${PEER_WAIT_TIMEOUT}s)"
      fi
    fi

    sleep $PEER_CHECK_INTERVAL
    elapsed=$((elapsed + PEER_CHECK_INTERVAL))
  done

  log "ERROR: Timeout waiting for bootstrap leader $leader"
  return 1
}

# Add this node to an existing cluster
add_self_to_cluster() {
  leader=$1
  endpoint=$(get_leader_endpoint "$leader")
  my_peer_url=$(get_my_peer_url)

  log "Adding self ($ETCD_NAME) to cluster via $endpoint..."

  # Check if already a member (in case of restart)
  if etcdctl member list --endpoints="$endpoint" 2>/dev/null | grep -q "$ETCD_NAME"; then
    log "Already a member of the cluster"
    return 0
  fi

  # Add as new member
  if etcdctl member add "$ETCD_NAME" --peer-urls="$my_peer_url" --endpoints="$endpoint"; then
    log "Successfully added to cluster"
    return 0
  else
    log "Failed to add self to cluster"
    return 1
  fi
}

# Build current cluster membership for joining node
get_current_cluster() {
  leader=$1
  endpoint=$(get_leader_endpoint "$leader")
  my_peer_url=$(get_my_peer_url)

  # Get existing members
  existing=$(etcdctl member list --endpoints="$endpoint" -w simple 2>/dev/null | while read -r line; do
    # Format: id, status, name, peer_urls, client_urls
    name=$(echo "$line" | cut -d',' -f3 | tr -d ' ')
    peer_url=$(echo "$line" | cut -d',' -f4 | tr -d ' ')
    if [ -n "$name" ] && [ -n "$peer_url" ]; then
      echo "${name}=${peer_url}"
    fi
  done | tr '\n' ',' | sed 's/,$//')

  # Add ourselves if not in list
  if ! echo "$existing" | grep -q "$ETCD_NAME="; then
    if [ -n "$existing" ]; then
      existing="${existing},${ETCD_NAME}=${my_peer_url}"
    else
      existing="${ETCD_NAME}=${my_peer_url}"
    fi
  fi

  echo "$existing"
}

# Monitor etcd and mark bootstrap complete once healthy
monitor_and_mark_bootstrap() {
  while true; do
    sleep 5
    if check_cluster_health; then
      if [ ! -f "$BOOTSTRAP_COMPLETE_MARKER" ]; then
        echo "1" > "$BOOTSTRAP_COMPLETE_MARKER"
        log "Cluster healthy - bootstrap marked complete"
      fi
    fi
  done
}

# CRITICAL: Clean stale data on startup if bootstrap never completed
if [ -d "$DATA_DIR" ] && [ "$(ls -A "$DATA_DIR" 2>/dev/null)" ]; then
  if [ ! -f "$BOOTSTRAP_COMPLETE_MARKER" ]; then
    log "Found stale data from incomplete bootstrap - cleaning..."
    rm -rf "${DATA_DIR:?}"/*
    log "Data directory cleaned, starting fresh"
  else
    log "Found data with completed bootstrap marker - preserving"
  fi
fi

# Determine our role
BOOTSTRAP_LEADER=$(get_bootstrap_leader)
IS_LEADER=false
if [ "$ETCD_NAME" = "$BOOTSTRAP_LEADER" ]; then
  IS_LEADER=true
fi

log "Bootstrap leader is: $BOOTSTRAP_LEADER (I am $ETCD_NAME, is_leader=$IS_LEADER)"

attempt=1
while [ $attempt -le $MAX_RETRIES ]; do
  log "Starting etcd (attempt $attempt/$MAX_RETRIES)..."

  # Start health monitor in background
  monitor_and_mark_bootstrap &
  MONITOR_PID=$!

  if [ "$IS_LEADER" = "true" ]; then
    # Bootstrap leader: start single-node cluster if fresh, or normal start if already bootstrapped
    if [ ! -f "$BOOTSTRAP_COMPLETE_MARKER" ]; then
      MY_PEER_URL=$(get_my_peer_url)
      log "Bootstrapping as single-node cluster: ${ETCD_NAME}=${MY_PEER_URL}"

      # Override cluster config for single-node bootstrap
      export ETCD_INITIAL_CLUSTER="${ETCD_NAME}=${MY_PEER_URL}"
      export ETCD_INITIAL_CLUSTER_STATE="new"
    fi

    /usr/local/bin/etcd
    EXIT_CODE=$?
  else
    # Non-leader: wait for leader, join existing cluster
    if [ ! -f "$BOOTSTRAP_COMPLETE_MARKER" ]; then
      if ! wait_for_leader "$BOOTSTRAP_LEADER"; then
        log "Failed to reach bootstrap leader, retrying..."
        kill $MONITOR_PID 2>/dev/null || true
        attempt=$((attempt + 1))
        sleep $RETRY_DELAY
        continue
      fi

      if ! add_self_to_cluster "$BOOTSTRAP_LEADER"; then
        log "Failed to add self to cluster, retrying..."
        kill $MONITOR_PID 2>/dev/null || true
        attempt=$((attempt + 1))
        sleep $RETRY_DELAY
        continue
      fi

      # Get current cluster membership and join as existing
      CURRENT_CLUSTER=$(get_current_cluster "$BOOTSTRAP_LEADER")
      log "Joining existing cluster: $CURRENT_CLUSTER"

      export ETCD_INITIAL_CLUSTER="$CURRENT_CLUSTER"
      export ETCD_INITIAL_CLUSTER_STATE="existing"
    fi

    /usr/local/bin/etcd
    EXIT_CODE=$?
  fi

  # Stop monitor
  kill $MONITOR_PID 2>/dev/null || true

  # Exit code 0 means clean shutdown (e.g., SIGTERM)
  if [ $EXIT_CODE -eq 0 ]; then
    log "etcd exited cleanly"
    exit 0
  fi

  log "etcd exited with code $EXIT_CODE"

  # Only clean data if bootstrap never completed
  if [ ! -f "$BOOTSTRAP_COMPLETE_MARKER" ]; then
    if [ -d "$DATA_DIR" ] && [ "$(ls -A "$DATA_DIR" 2>/dev/null)" ]; then
      log "Bootstrap incomplete - cleaning data directory..."
      rm -rf "${DATA_DIR:?}"/*
    fi
  else
    log "Bootstrap was complete - preserving data directory"
  fi

  attempt=$((attempt + 1))
  if [ $attempt -le $MAX_RETRIES ]; then
    log "Retrying in ${RETRY_DELAY}s..."
    sleep $RETRY_DELAY
  fi
done

log "Failed to start etcd after $MAX_RETRIES attempts"
exit 1

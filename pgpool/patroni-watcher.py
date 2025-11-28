#!/usr/bin/env python3
import os
import sys
import time
import subprocess
import urllib.request
import json

PATRONI_HOSTS = [
    "postgres-1.railway.internal:8008",
    "postgres-2.railway.internal:8008",
    "postgres-3.railway.internal:8008",
]

BACKEND_MAP = {
    "postgres-1": 0,
    "postgres-2": 1,
    "postgres-3": 2,
}

CHECK_INTERVAL = int(os.environ.get("CHECK_INTERVAL_MS", "3000")) / 1000
RESYNC_INTERVAL = int(os.environ.get("RESYNC_INTERVAL_S", "30"))
PCP_RETRY_COUNT = int(os.environ.get("PCP_RETRY_COUNT", "3"))
PCP_RETRY_DELAY = float(os.environ.get("PCP_RETRY_DELAY_S", "1"))

last_leader_name = None
last_sync_time = 0


def log(msg):
    print(f"[watcher] {msg}", file=sys.stderr, flush=True)


def get_leader():
    for host in PATRONI_HOSTS:
        try:
            url = f"http://{host}/leader"
            req = urllib.request.Request(url, method="GET")
            with urllib.request.urlopen(req, timeout=2) as resp:
                if resp.status == 200:
                    data = json.loads(resp.read().decode())
                    return data.get("name")
        except Exception:
            continue
    return None


def get_cluster_state():
    for host in PATRONI_HOSTS:
        try:
            url = f"http://{host}/cluster"
            req = urllib.request.Request(url, method="GET")
            with urllib.request.urlopen(req, timeout=2) as resp:
                if resp.status == 200:
                    return json.loads(resp.read().decode())
        except Exception:
            continue
    return None


def run_pcp_command(cmd, retries=PCP_RETRY_COUNT):
    for attempt in range(retries):
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=5,
                env={**os.environ, "PCPPASSFILE": "/tmp/.pcppass"}
            )
            if result.returncode == 0:
                return True, result.stdout
            if attempt < retries - 1:
                log(f"PCP command failed (attempt {attempt + 1}/{retries}): {result.stderr.strip()}")
                time.sleep(PCP_RETRY_DELAY)
        except subprocess.TimeoutExpired:
            log(f"PCP command timed out (attempt {attempt + 1}/{retries})")
            if attempt < retries - 1:
                time.sleep(PCP_RETRY_DELAY)
        except Exception as e:
            log(f"PCP command error (attempt {attempt + 1}/{retries}): {e}")
            if attempt < retries - 1:
                time.sleep(PCP_RETRY_DELAY)
    return False, None


def get_backend_status(node_id):
    success, output = run_pcp_command([
        "pcp_node_info", "-h", "localhost", "-p", "9898", "-U", "admin", "-w", "-n", str(node_id)
    ])
    if success and output:
        parts = output.strip().split()
        if len(parts) >= 3:
            return parts[2]  # status field
    return None


def attach_node(node_id):
    log(f"Attaching node {node_id}")
    success, _ = run_pcp_command([
        "pcp_attach_node", "-h", "localhost", "-p", "9898", "-U", "admin", "-w", "-n", str(node_id)
    ])
    return success


def promote_node(node_id):
    log(f"Promoting node {node_id} to primary")
    success, _ = run_pcp_command([
        "pcp_promote_node", "-h", "localhost", "-p", "9898", "-U", "admin", "-w", "-n", str(node_id)
    ])
    return success


def sync_to_leader(leader_name, force=False):
    global last_leader_name, last_sync_time

    if leader_name not in BACKEND_MAP:
        log(f"Unknown leader: {leader_name}")
        return False

    leader_node_id = BACKEND_MAP[leader_name]
    all_success = True

    # Get current status of all backends
    for name, node_id in BACKEND_MAP.items():
        status = get_backend_status(node_id)
        if status is None:
            log(f"Could not get status for node {node_id} ({name})")
            all_success = False
            continue

        # Status codes: 1=up/attached, 2=up/primary, 3=down/detached
        is_attached = status in ("1", "2")
        is_leader = (node_id == leader_node_id)

        if is_leader:
            if not is_attached:
                if not attach_node(node_id):
                    all_success = False
                    continue
            if not promote_node(node_id):
                all_success = False
        else:
            # Ensure replicas are attached (for failover readiness)
            if not is_attached:
                if not attach_node(node_id):
                    all_success = False

    if all_success:
        last_leader_name = leader_name
        last_sync_time = time.time()
        log(f"Synced to leader: {leader_name}")
    else:
        log(f"Sync to leader {leader_name} had failures, will retry")

    return all_success


def main():
    global last_leader_name, last_sync_time

    log("Starting patroni watcher")
    log(f"Check interval: {CHECK_INTERVAL}s, Resync interval: {RESYNC_INTERVAL}s")

    # Wait for pgpool to be ready
    time.sleep(5)

    while True:
        try:
            leader = get_leader()
            now = time.time()
            needs_resync = (now - last_sync_time) >= RESYNC_INTERVAL

            if leader is None:
                log("Could not determine leader from any Patroni node")
            elif leader != last_leader_name:
                log(f"Leader changed: {last_leader_name} -> {leader}")
                sync_to_leader(leader)
            elif needs_resync:
                log(f"Periodic resync (last sync was {int(now - last_sync_time)}s ago)")
                sync_to_leader(leader, force=True)

        except Exception as e:
            log(f"Error in main loop: {e}")

        time.sleep(CHECK_INTERVAL)


if __name__ == "__main__":
    main()

#!/bin/bash
# post_bootstrap.sh - Patroni post-bootstrap script
#
# Runs ONCE after PostgreSQL initialization on the primary node.
# Note: Users are created by Patroni via bootstrap.users in patroni.yml
# This script only handles SSL setup and bootstrap marker.

set -e

echo "Post-bootstrap: starting..."

# Generate SSL certificates
echo "Post-bootstrap: generating SSL certificates..."
bash /docker-entrypoint-initdb.d/init-ssl.sh

# Mark bootstrap as complete - patroni-runner.sh checks for this marker
# to distinguish complete bootstrap from stale/failed data
touch /var/lib/postgresql/data/.patroni_bootstrap_complete

echo "Post-bootstrap completed"

# Multi-stage Dockerfile for PostgreSQL with optional Patroni HA
#
# Build variants:
#   slim (default) - Matches postgres-ssl, ~400MB
#   ha             - Adds Patroni for high availability, ~550MB
#
# Usage:
#   docker build --build-arg POSTGRES_VERSION=17 --build-arg BUILD_VARIANT=slim -t postgres:17 .
#   docker build --build-arg POSTGRES_VERSION=17 --build-arg BUILD_VARIANT=ha -t postgres:17-ha .

ARG POSTGRES_VERSION=17
ARG BUILD_VARIANT=slim

# ============================================================
# Rust builder stage: Compile all Rust binaries
# ============================================================
FROM rust:1.83-bookworm AS rust-builder

WORKDIR /build

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build release binaries
RUN cargo build --release

# ============================================================
# Base stage: Common to both slim and HA
# ============================================================
FROM postgres:${POSTGRES_VERSION} AS base

RUN apt-get update && apt-get install -y --no-install-recommends \
    openssl \
    sudo \
    && rm -rf /var/lib/apt/lists/*

# Allow postgres user to execute specific commands as root
RUN echo "postgres ALL=(root) NOPASSWD: /usr/bin/mkdir, /bin/chown, /bin/chmod, /usr/bin/openssl" > /etc/sudoers.d/postgres

# ============================================================
# Slim stage: Standalone PostgreSQL with SSL (matches postgres-ssl)
# ============================================================
FROM base AS slim

# Copy SSL init script and Rust wrapper
COPY --chmod=755 init-ssl.sh /docker-entrypoint-initdb.d/init-ssl.sh
COPY --from=rust-builder --chmod=755 /build/target/release/postgres-wrapper /usr/local/bin/postgres-wrapper

EXPOSE 5432

HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
    CMD pg_isready -U ${POSTGRES_USER:-postgres} || exit 1

ENTRYPOINT ["postgres-wrapper"]
CMD ["postgres", "-p", "5432", "-c", "listen_addresses=*"]

# ============================================================
# HA stage: Patroni for high availability
# ============================================================
FROM base AS ha

ARG PATRONI_VERSION=4.1.0

# Install HA dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 \
    python3-pip \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install setuptools first (needed for ydiff's legacy setup.py)
RUN pip3 install --no-cache-dir --break-system-packages setuptools

# Install Patroni and dependencies (etcd3 for v3 API)
# Note: python-etcd is required due to Patroni 4.0.5 import bug (GitHub #3337)
# The etcd3 module imports from etcd module even when using v3 API
# etcd3 is also installed separately for our stale-state cleanup script
RUN pip3 install --no-cache-dir --break-system-packages \
    psycopg2-binary \
    python-etcd==0.4.5 \
    etcd3 \
    patroni[etcd3]==${PATRONI_VERSION}

# Create required directories
RUN mkdir -p /home/postgres /var/run/postgresql \
    && chown -R postgres:postgres /home/postgres /var/run/postgresql

# Copy SSL init script and Rust binaries
COPY --chmod=755 init-ssl.sh /docker-entrypoint-initdb.d/init-ssl.sh
COPY --from=rust-builder --chmod=755 /build/target/release/postgres-wrapper /usr/local/bin/postgres-wrapper
COPY --from=rust-builder --chmod=755 /build/target/release/post-bootstrap /usr/local/bin/post-bootstrap
COPY --from=rust-builder --chmod=755 /build/target/release/patroni-runner /usr/local/bin/patroni-runner
COPY --from=rust-builder --chmod=755 /build/target/release/on-role-change /usr/local/bin/on-role-change

EXPOSE 5432 8008

# Aggressive health check for faster split-brain detection
HEALTHCHECK --interval=5s --timeout=3s --start-period=30s --retries=2 \
    CMD if [ "${PATRONI_ENABLED:-false}" = "true" ]; then \
            curl -sf http://localhost:8008/health || exit 1; \
        else \
            pg_isready -U ${POSTGRES_USER:-postgres} || exit 1; \
        fi

ENTRYPOINT ["postgres-wrapper"]
CMD ["postgres", "-p", "5432", "-c", "listen_addresses=*"]

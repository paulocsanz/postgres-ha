ARG ETCD_VERSION=3.6.6

# ============================================================
# Rust builder stage: Compile etcd-entrypoint binary (musl)
# ============================================================
FROM rust:1.83-alpine AS rust-builder

RUN apk add --no-cache musl-dev

WORKDIR /build

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build statically linked release binary
RUN cargo build --release

# ============================================================
# Final stage: etcd with Rust entrypoint
# ============================================================
FROM quay.io/coreos/etcd:v${ETCD_VERSION}

# Copy Rust binary (statically linked, no glibc needed)
COPY --from=rust-builder --chmod=755 /build/target/release/etcd-entrypoint /entrypoint

ENV ETCD_DATA_DIR=/var/lib/etcd
# Enable gRPC gateway for Patroni etcd3 client compatibility
ENV ETCD_ENABLE_GRPC_GATEWAY=true
# Retry settings (5 min total with defaults: 60 retries * 5s delay)
ENV ETCD_MAX_RETRIES=60
ENV ETCD_RETRY_DELAY=5
ENV ETCD_STABILIZE_TIME=30

EXPOSE 2379 2380

HEALTHCHECK --interval=10s --timeout=5s --start-period=60s --retries=3 \
  CMD etcdctl endpoint health --endpoints=http://127.0.0.1:2379 || exit 1

ENTRYPOINT ["/entrypoint"]

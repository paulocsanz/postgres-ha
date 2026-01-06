ARG HAPROXY_VERSION=3.2

# ============================================================
# Rust builder stage: Compile haproxy-entrypoint binary
# ============================================================
FROM rust:1.83-alpine AS rust-builder

RUN apk add --no-cache musl-dev

WORKDIR /build

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build release binary (statically linked with musl)
RUN cargo build --release

# ============================================================
# Final stage: HAProxy with Rust entrypoint
# ============================================================
FROM haproxy:${HAPROXY_VERSION}-alpine

USER root

# Install postgresql-client and bash for Railway's Database UI to work via SSH tunnel
# (bash is required because the UI runs: bash +H -c 'psql ...')
RUN apk add --no-cache postgresql-client bash

# Copy Rust binary
COPY --from=rust-builder --chmod=755 /build/target/release/haproxy-entrypoint /entrypoint

RUN chown -R haproxy:haproxy /usr/local/etc/haproxy

USER haproxy

EXPOSE 5432 5433 8404

HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://127.0.0.1:8404/stats || exit 1

ENTRYPOINT ["/entrypoint"]

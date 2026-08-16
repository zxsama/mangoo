# syntax=docker/dockerfile:1.7

# ---- Stage 1: Rust core builder ---------------------------------------------
FROM rust:1.85-slim AS core-builder
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p megle-core \
    && strip target/release/megle-core || true

# ---- Stage 2: Web UI builder -------------------------------------------------
FROM node:22-slim AS web-builder
WORKDIR /web

COPY package.json package-lock.json tsconfig.base.json ./
COPY apps/web ./apps/web
COPY packages/core-client ./packages/core-client
COPY contracts ./contracts

RUN npm ci --no-audit --no-fund
RUN npm --workspace @megle/web run build

# ---- Stage 3: Runtime --------------------------------------------------------
FROM debian:trixie-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

# Drop privileges: run Megle as a non-root system user. `/data` (the volume
# mountpoint for the SQLite DB, generated thumbnails, and plugin state) needs
# to be writable by this user. `/library` is mounted by the operator at run
# time, so its ownership is the operator's responsibility (the read-only
# default in compose keeps Megle from needing write access there).
RUN groupadd --system --gid 10001 megle \
    && useradd --system --uid 10001 --gid 10001 --home /home/megle --create-home megle \
    && mkdir -p /data && chown -R megle:megle /data

COPY --from=core-builder /src/target/release/megle-core /usr/local/bin/megle-core
COPY --from=web-builder  /web/apps/web/dist             /opt/megle/web

ENV MEGLE_HOST_DB_PATH=/data/host/host.sqlite \
    MEGLE_LIBRARIES_DIR=/data/host/libraries \
    MEGLE_PLUGINS_DIR=/data/plugins \
    MEGLE_WEB_DIR=/opt/megle/web \
    MEGLE_SERVE_WEB=1 \
    MEGLE_CORE_ADDR=0.0.0.0:47321 \
    RUST_LOG=info

EXPOSE 47321
VOLUME ["/data", "/library"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s \
    CMD wget -qO- http://127.0.0.1:47321/api/health >/dev/null 2>&1 || exit 1

USER megle

CMD ["/usr/local/bin/megle-core"]

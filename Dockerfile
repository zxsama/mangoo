# syntax=docker/dockerfile:1.7

# ---- Stage 1: Rust core builder ---------------------------------------------
FROM rust:1.88-slim AS core-builder
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo test --release -p megle-fake-provider --test process_conformance \
    && cargo test --release -p megle-onedrive-provider --features graph-emulator \
    && cargo build --release -p megle-fake-provider -p megle-onedrive-provider \
    && strip target/release/megle-fake-provider \
    && strip target/release/megle-onedrive-provider \
    && FAKE_HASH=$(sha256sum target/release/megle-fake-provider | cut -d' ' -f1) \
    && ONEDRIVE_HASH=$(sha256sum target/release/megle-onedrive-provider | cut -d' ' -f1) \
    && FAKE_BYTES=$(echo "$FAKE_HASH" | sed 's/../0x&,/g') \
    && ONEDRIVE_BYTES=$(echo "$ONEDRIVE_HASH" | sed 's/../0x&,/g') \
    && printf '// Generated during Docker build.\npub const GENERATED_FAKE_PROVIDER_SHA256: [u8; 32] = [%s];\npub const GENERATED_ONEDRIVE_PROVIDER_SHA256: [u8; 32] = [%s];\n' "$FAKE_BYTES" "$ONEDRIVE_BYTES" > crates/core/src/plugins/runtime/generated_trust.rs \
    && cargo build --release -p megle-core \
    && cargo test --release -p megle-core --test plugin_runtime_process \
    && strip target/release/megle-core

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
    && mkdir -p /data/plugin-state /data/secrets /opt/megle/plugins \
    && chown -R megle:megle /data \
    && chown -R root:root /opt/megle/plugins \
    && chmod 0555 /opt/megle/plugins

COPY --from=core-builder /src/target/release/megle-core /usr/local/bin/megle-core
COPY --from=core-builder /src/target/release/megle-fake-provider /opt/megle/plugins/com.megle.fake-sync/bin/megle-fake-provider
COPY bundled-plugins/com.megle.fake-sync/plugin.template.json /opt/megle/plugins/com.megle.fake-sync/plugin.template.json
COPY --from=core-builder /src/target/release/megle-onedrive-provider /opt/megle/plugins/com.megle.onedrive/bin/megle-onedrive-provider
COPY bundled-plugins/com.megle.onedrive/plugin.template.json /opt/megle/plugins/com.megle.onedrive/plugin.template.json
COPY --from=web-builder  /web/apps/web/dist             /opt/megle/web

RUN sha256sum /opt/megle/plugins/com.megle.fake-sync/bin/megle-fake-provider | cut -d' ' -f1 > /tmp/provider.sha \
    && sed "s/__SHA256__/$(cat /tmp/provider.sha)/g" /opt/megle/plugins/com.megle.fake-sync/plugin.template.json > /opt/megle/plugins/com.megle.fake-sync/plugin.json \
    && sha256sum /opt/megle/plugins/com.megle.onedrive/bin/megle-onedrive-provider | cut -d' ' -f1 > /tmp/onedrive.sha \
    && sed "s/__SHA256__/$(cat /tmp/onedrive.sha)/g" /opt/megle/plugins/com.megle.onedrive/plugin.template.json > /opt/megle/plugins/com.megle.onedrive/plugin.json \
    && rm /tmp/provider.sha /tmp/onedrive.sha /opt/megle/plugins/com.megle.fake-sync/plugin.template.json /opt/megle/plugins/com.megle.onedrive/plugin.template.json \
    && chown -R root:root /opt/megle/plugins && chmod -R a-w /opt/megle/plugins \
    && chmod 0555 /opt/megle/plugins/com.megle.fake-sync/bin/megle-fake-provider /opt/megle/plugins/com.megle.onedrive/bin/megle-onedrive-provider

ENV MEGLE_HOST_DB_PATH=/data/host/host.sqlite \
    MEGLE_LIBRARIES_DIR=/data/host/libraries \
    MEGLE_PLUGINS_DIR=/opt/megle/plugins \
    MEGLE_PLUGIN_STATE_DIR=/data/plugin-state \
    MEGLE_SECRETS_DIR=/data/secrets \
    MEGLE_SECRET_KEY_FILE=/run/secrets/megle_secret_key \
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

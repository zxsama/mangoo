# syntax=docker/dockerfile:1.7

# ---- Stage 1: Rust core builder ---------------------------------------------
FROM rust:1.88-slim AS core-builder
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bundled-plugins ./bundled-plugins
COPY contracts/plugins/fixtures ./contracts/plugins/fixtures

RUN cargo test --release -p mangoo-fake-provider --test process_conformance \
    && cargo test --release -p mangoo-ppm-decoder --test process_conformance \
    && cargo build --release -p mangoo-fake-provider -p mangoo-ppm-decoder \
    && strip target/release/mangoo-fake-provider target/release/mangoo-ppm-decoder \
    && mkdir -p target/release/bundled-plugins \
    && INVENTORY=target/release/bundled-plugins/.inventory.json \
    && printf '{"schemaVersion":1,"plugins":[' > "$INVENTORY" \
    && SEP='' \
    && for plugin in 'com.mangoo.fake-sync:mangoo-fake-provider:1.0.0' 'com.mangoo.ppm-decoder:mangoo-ppm-decoder:0.1.0'; do \
        PLUGIN_ID=${plugin%%:*}; PLUGIN_REST=${plugin#*:}; PLUGIN_BIN=${PLUGIN_REST%%:*}; PLUGIN_VERSION=${PLUGIN_REST#*:}; \
        PLUGIN_DIR=target/release/bundled-plugins/$PLUGIN_ID; \
        mkdir -p "$PLUGIN_DIR/bin" \
        && cp "target/release/$PLUGIN_BIN" "$PLUGIN_DIR/bin/$PLUGIN_BIN" \
        && PLUGIN_HASH=$(sha256sum "$PLUGIN_DIR/bin/$PLUGIN_BIN" | cut -d' ' -f1) \
        && sed "s/__SHA256__/$PLUGIN_HASH/g" "bundled-plugins/$PLUGIN_ID/plugin.template.json" > "$PLUGIN_DIR/plugin.json" \
        && MANIFEST_HASH=$(sha256sum "$PLUGIN_DIR/plugin.json" | cut -d' ' -f1) \
        && printf '%s{"id":"%s","version":"%s","platform":"linuxX64","entry":"bin/%s","sha256":"%s","manifestSha256":"%s"}' "$SEP" "$PLUGIN_ID" "$PLUGIN_VERSION" "$PLUGIN_BIN" "$PLUGIN_HASH" "$MANIFEST_HASH" >> "$INVENTORY" \
        && SEP=',' \
        || exit 1; \
    done \
    && for PLUGIN_ID in com.mangoo.image-viewer com.mangoo.notebook com.mangoo.todo-notes; do \
        PLUGIN_DIR=target/release/bundled-plugins/$PLUGIN_ID; \
        mkdir -p "$PLUGIN_DIR" && cp "bundled-plugins/$PLUGIN_ID/plugin.template.json" "$PLUGIN_DIR/plugin.json" \
        && MANIFEST_HASH=$(sha256sum "$PLUGIN_DIR/plugin.json" | cut -d' ' -f1) \
        && PLUGIN_VERSION=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$PLUGIN_DIR/plugin.json" | head -1) \
        && test -n "$PLUGIN_VERSION" \
        && printf ',{"id":"%s","version":"%s","platform":"linuxX64","engine":"host","manifestSha256":"%s"}' "$PLUGIN_ID" "$PLUGIN_VERSION" "$MANIFEST_HASH" >> "$INVENTORY" || exit 1; \
    done \
    && printf ']}\n' >> "$INVENTORY" \
    && cargo build --release -p mangoo-core \
    && cargo test --release -p mangoo-core --test plugin_runtime_process \
    && strip target/release/mangoo-core

# Release-only Linux verifier. The final runtime image remains minimal; this
# target retains Cargo/test artifacts so the exact archived tree can execute
# production sync bootstrap, convergence, restore, fault and physical-transfer
# probes inside Linux.
FROM core-builder AS sync-verifier
CMD ["cargo", "test", "--release", "-p", "mangoo-core"]

# ---- Stage 2: Web UI builder -------------------------------------------------
FROM node:22-slim AS web-builder
WORKDIR /web

COPY package.json package-lock.json tsconfig.base.json ./
COPY apps/web ./apps/web
COPY bundled-plugins ./bundled-plugins
COPY packages/core-client ./packages/core-client
COPY packages/notification-contract ./packages/notification-contract
COPY packages/plugin-sdk ./packages/plugin-sdk
COPY contracts ./contracts

RUN npm ci --no-audit --no-fund
RUN npm --workspace @mangoo/web run build

# ---- Stage 3: Runtime --------------------------------------------------------
FROM debian:trixie-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

# Drop privileges: run Mangoo as a non-root system user. `/data` (the volume
# mountpoint for the SQLite DB, generated thumbnails, and plugin state) needs
# to be writable by this user. `/library` is mounted by the operator at run
# time, so its ownership is the operator's responsibility (the read-only
# default in compose keeps Mangoo from needing write access there).
RUN groupadd --system --gid 10001 mangoo \
    && useradd --system --uid 10001 --gid 10001 --home /home/mangoo --create-home mangoo \
    && mkdir -p /data/plugin-state /data/secrets /opt/mangoo/plugins \
    && chown -R mangoo:mangoo /data \
    && chown -R root:root /opt/mangoo/plugins \
    && chmod 0555 /opt/mangoo/plugins

COPY --from=core-builder /src/target/release/mangoo-core /usr/local/bin/mangoo-core
COPY --from=core-builder /src/target/release/bundled-plugins /opt/mangoo/plugins
COPY --from=web-builder  /web/apps/web/dist             /opt/mangoo/web

RUN chown -R root:root /opt/mangoo/plugins && chmod -R a-w /opt/mangoo/plugins \
    && chmod 0555 /opt/mangoo/plugins/com.mangoo.fake-sync/bin/mangoo-fake-provider \
        /opt/mangoo/plugins/com.mangoo.ppm-decoder/bin/mangoo-ppm-decoder

ENV MANGOO_HOST_DB_PATH=/data/host/host.sqlite \
    MANGOO_LIBRARIES_DIR=/data/host/libraries \
    MANGOO_PLUGINS_DIR=/opt/mangoo/plugins \
    MANGOO_PLUGIN_STATE_DIR=/data/plugin-state \
    MANGOO_SECRETS_DIR=/data/secrets \
    MANGOO_SECRET_KEY_FILE=/run/secrets/mangoo_secret_key \
    MANGOO_WEB_DIR=/opt/mangoo/web \
    MANGOO_SERVE_WEB=1 \
    MANGOO_CORE_ADDR=0.0.0.0:47321 \
    RUST_LOG=info

EXPOSE 47321
VOLUME ["/data", "/library"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s \
    CMD wget -qO- http://127.0.0.1:47321/api/health >/dev/null 2>&1 || exit 1

USER mangoo

CMD ["/usr/local/bin/mangoo-core"]

# Megle Deployment (Docker)

This guide covers running Megle as a headless service in Docker. Core serves
both the HTTP API and the built React UI, so a single container is enough to
browse a mounted library from any browser on the same network.

> Phase 9 deployment is **single-user**. Multi-tenancy, user accounts, and
> HTTPS termination are explicitly out of scope. Run the container behind a
> reverse proxy (Caddy, nginx, Traefik) when you need TLS or per-user access.

The runtime container drops privileges and runs Megle as a non-root system
user (`megle`, uid/gid `10001`). The named `megle-data` volume is `chown`ed
to that user during the image build so the SQLite database, generated
thumbnails, and plugin state are writable. If you bind-mount `/library` from
the host with write access (no `:ro`), make sure the host directory is
readable (and writable, when needed) by uid `10001` or use a permissive
mode.

## Quickstart

```bash
# from the repo root
docker compose up --build
```

Open `http://localhost:47321` in a browser. You should see the same UI the
desktop shell loads. Add a root that points at `/library` (the path inside
the container, not on the host) to start scanning.

## Mounting your library

Megle never imports media; it indexes the directories you mount. Point the
`MEGLE_LIBRARY_PATH` env var at the host directory you want exposed, and the
container will see it at `/library` (read-only by default):

```bash
MEGLE_LIBRARY_PATH=/srv/photos docker compose up
```

Inside Megle, add a root with the path `/library` (or any subdirectory of it,
e.g. `/library/2024`).

If you need write access for file operations (rename / move / recycle), drop
the `:ro` suffix in `compose.yaml`:

```yaml
- ${MEGLE_LIBRARY_PATH:-./library}:/library
```

## Authentication

Megle supports two opt-in auth mechanisms:

- **HTTP Basic auth** (`MEGLE_BASIC_AUTH=user:pass`) — front-of-house
  protection for the browser UI and API. Applied to every request except
  `/api/health` so Docker healthchecks keep working without credentials.
  Returns `401 WWW-Authenticate: Basic realm="Megle"` on failure. Use this
  when the container is reachable from anything other than your loopback.
- **Session token** (`MEGLE_SESSION_TOKEN=<token>`) — the existing desktop
  mechanism. When set, every request must carry `X-Megle-Session: <token>`.
  Leave it **unset** in browser deployments where Basic auth is the only
  gate; the static UI bundle does not yet inject a session token from the
  page.

> **Security: terminate TLS in a reverse proxy.** The Megle container speaks
> plain HTTP. HTTP Basic auth credentials are sent base64-encoded in every
> request and are trivially recoverable on the wire. Do **not** expose the
> container directly to an untrusted network. Put it behind a reverse proxy
> (Caddy, nginx, Traefik) that terminates TLS, or restrict it to a private
> network you fully control. The same caveat applies to `MEGLE_SESSION_TOKEN`
> over plain HTTP.

Recommended browser-deploy posture: set `MEGLE_BASIC_AUTH`, leave
`MEGLE_SESSION_TOKEN` unset.

```bash
MEGLE_BASIC_AUTH=alice:s3cret docker compose up
```

For the strictest configuration (e.g. a private LAN where you also run the
desktop client), set both variables.

## Persisting data

The container writes its SQLite database, generated thumbnails, and plugin
state into `/data`, backed by the named volume `megle-data`. The volume
survives `docker compose down`; remove it explicitly with
`docker volume rm megle-megle-data` if you want a clean slate.

Provider runtime state lives in `/data/plugin-state`; encrypted credential
records live separately in `/data/secrets`. Bundled executable providers are
only loaded from the root-owned, read-only `/opt/megle/plugins` image path.
Files placed under `/data` are never treated as executable plugins.

The encrypted records require an independently mounted 32-byte master key at
`/run/secrets/megle_secret_key`. Megle never generates this key in `/data`.
If it is absent or has unsafe permissions, local browsing remains available
and secret-dependent providers report `configuration-required`.

Create the key once, keep it outside the data volume, and restrict access:

```bash
mkdir -p secrets
openssl rand 32 > secrets/megle_secret_key
chmod 600 secrets/megle_secret_key
docker compose up --build
```

`/library` is the bind mount for your media. Nothing is written there unless
you remove the `:ro` flag.

## Environment variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `MEGLE_CORE_ADDR` | Bind address. | `0.0.0.0:47321` |
| `MEGLE_HOST_DB_PATH` | Host registry SQLite path. | `/data/host/host.sqlite` |
| `MEGLE_LIBRARIES_DIR` | Local catalog/cache storage owned by the Host. | `/data/host/libraries` |
| `MEGLE_PLUGINS_DIR` | Read-only bundled provider root. | `/opt/megle/plugins` |
| `MEGLE_PLUGIN_STATE_DIR` | Writable non-secret Provider runtime state. | `/data/plugin-state` |
| `MEGLE_SECRETS_DIR` | Encrypted Provider credential records. | `/data/secrets` |
| `MEGLE_SECRET_KEY_FILE` | Read-only 32-byte master-key file. | `/run/secrets/megle_secret_key` |
| `MEGLE_PUBLIC_URL` | Public HTTPS base used for Docker OAuth callbacks; no userinfo/query/fragment. | _(unset)_ |
| `MEGLE_ONEDRIVE_CLIENT_ID` | Microsoft Entra public-client application ID for the bundled OneDrive Provider. It is configuration, not a secret. | _(unset; Provider reports configuration-required)_ |
| `MEGLE_ONEDRIVE_AUTHORITY` | Optional Microsoft identity authority override. | `https://login.microsoftonline.com/common/` |
| `MEGLE_SERVE_WEB` | Serve the bundled UI when `1`. | `1` |
| `MEGLE_WEB_DIR` | Directory containing `index.html` + `assets/`. | `/opt/megle/web` |
| `MEGLE_BASIC_AUTH` | Browser/API Basic auth. Compose uses a development credential that must be overridden for production. | `megle:change-me` (Compose) |
| `MEGLE_SESSION_TOKEN` | Optional desktop session token. | _(unset)_ |
| `MEGLE_ALLOWED_ORIGIN` | Optional dev CORS origin (exact match). | _(unset)_ |
| `RUST_LOG` | Tracing filter. | `info` |

Override any of them via `compose.yaml` or the `-e` flag on `docker run`.

## Health checks

The image ships with a `HEALTHCHECK` that polls `/api/health`. The endpoint
is always exempt from Basic auth so the container reports healthy without
credentials. To exercise it manually:

```bash
curl -f http://localhost:47321/api/health
```

## Updating

The container caches Rust and npm dependencies between builds. To pick up
new code:

```bash
docker compose build --pull
docker compose up -d
```

The named `megle-data` volume is preserved across rebuilds.

## Limitations

- **Browser UI session token is not injected.** When `MEGLE_BASIC_AUTH` is
  the front-of-house gate, leave `MEGLE_SESSION_TOKEN` unset; the static UI
  cannot currently send `X-Megle-Session` from the browser. Tracking work
  for cookie-based session-token wiring is in the master plan.
- **No HTTPS in the image.** Terminate TLS in a reverse proxy.
- **OneDrive OAuth requires an exact public callback.** Set `MEGLE_PUBLIC_URL`
  to the external HTTPS origin registered for the public client, and set
  `MEGLE_ONEDRIVE_CLIENT_ID`; tokens remain in the mounted encrypted Secret
  Store, never in Compose environment values.
- **Single user.** All requests share one library, one set of tags, and one
  task queue.
- **Windows-only file ops still apply.** Linux containers can mount Windows
  shares, but advanced recycle-bin operations remain Windows-specific. The
  OneDrive sync Provider itself is bundled for Linux and runs as a supervised
  process from read-only `/opt/megle/plugins`.

## Running without compose

```bash
docker build -t megle:dev .
docker run --rm -p 47321:47321 \
  -v megle-data:/data \
  -v "$PWD/secrets/megle_secret_key:/run/secrets/megle_secret_key:ro" \
  -v "$PWD/library:/library:ro" \
  -e MEGLE_BASIC_AUTH=alice:s3cret \
  megle:dev
```

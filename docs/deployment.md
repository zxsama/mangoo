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
| `MEGLE_ONEDRIVE_CLIENT_ID` | Optional read-only public-client Application ID supplied by the deployment operator. It is configuration, not a secret. | _(unset; configure through the OneDrive setup wizard)_ |
| `MEGLE_ONEDRIVE_AUTH_MODE` | OneDrive authorization completion mode. Docker must use the image default. | `manual-response-uri` |
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

## Synchronization release qualification

Run `node tools/checks/sync-release.mjs --renderer-evidence <clean-renderer-evidence.json>`
from a clean commit before publishing an image.
The command builds from a hashed `git archive`, verifies the Windows gate, then
requires an actual Docker/Linux build/runtime row plus a two-container volume
restart proof of the Host Registry, active Library Catalog, sync capture state,
and Provider state. Separate credential-gated Personal and Business OneDrive
canaries exercise Delta, immutable put, Range download, and upload sessions.
The gate also keeps Product OAuth evidence separate from Graph transport tokens:
Personal, Business, Windows Desktop loopback, and Docker manual response-URI
authorization each have their own required evidence row. Missing Docker access,
operator-owned Entra application registrations, real accounts, or those evidence
files is a blocking result, never an emulator success. See
`docs/sync-operations.md` for staged rollout and forward-disable rollback.

## OneDrive application registration and authorization

Each user or deployment operator creates their own Microsoft Entra **public
client** application. Megle release artifacts contain no publisher-owned Client
ID. Choose the audience that supports accounts in any organizational directory
and personal Microsoft accounts, add delegated Microsoft Graph permission
`Files.ReadWrite.AppFolder`, and enable public-client flows. Register the native
redirect form used by the target platform:

- Windows Desktop: add `http://127.0.0.1/plugin-auth/callback` through the
  application manifest; the Provider sends the same IPv4 host/path with an
  ephemeral runtime port.
- Docker/Web: `https://login.microsoftonline.com/common/oauth2/nativeclient`.

Open the OneDrive plugin and choose **配置 OneDrive**. The setup wizard
walks through application creation, platform settings, the Application (client)
ID, Microsoft sign-in, and final verification. It accepts only the public Client
ID and never requests a Directory ID, Object ID, password, token, or Client
Secret. Operators may instead set `MEGLE_ONEDRIVE_CLIENT_ID`; that value has
priority and is read-only in the UI. With neither an environment value nor a
Host-stored Client ID, OneDrive remains safely `configuration-required` while
local browsing continues normally.

Docker follows the same manual handoff used by `abraunegg/onedrive`:

1. Complete the OneDrive setup wizard, then choose **登录 Microsoft 账号**.
2. Open or copy the displayed Microsoft authorization URL and finish sign-in in
   any browser.
3. When the browser reaches the native-client page, copy the complete address
   from the address bar and paste it into Megle's **登录返回地址** field.
4. Submit once. Megle validates the exact scheme, host, path, fields, flow,
   state, expiry, and PKCE exchange, then immediately clears the transient URI.

There is no inbound OAuth callback, `MEGLE_PUBLIC_URL`, confidential-client
registration, or client secret in this flow. The response URI contains a
short-lived authorization code and state: do not put it in Compose, logs,
screenshots, shell history, tickets, or diagnostics.

## Limitations

- **Browser UI session token is not injected.** When `MEGLE_BASIC_AUTH` is
  the front-of-house gate, leave `MEGLE_SESSION_TOKEN` unset; the static UI
  cannot currently send `X-Megle-Session` from the browser. Tracking work
  for cookie-based session-token wiring is in the master plan.
- **No HTTPS in the image.** Terminate TLS in a reverse proxy.
- **OneDrive authorization is manual in Docker.** Keep
  `MEGLE_ONEDRIVE_AUTH_MODE=manual-response-uri`; no public callback is exposed.
  Tokens remain in the mounted encrypted Secret Store and never belong in
  Compose environment values.
- **Single-user Host, multiple Libraries.** One deployed Host owns its Library
  registry and user session. Each managed Library has its own Catalog, roots,
  tags and sync binding; the active Library is switched through Core without
  restarting the process.
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

# Mangoo Deployment

## Windows packaged local launch

From the repository root, run `start-desktop-prod.bat` (optionally passing a
folder to add on first launch). The launcher incrementally builds the release
Core, bundled Providers, Web renderer, and Desktop shell, then creates and
starts `.data/desktop-release/win-unpacked/Mangoo.exe`.

After startup, the batch/Node build process exits. `Mangoo.exe` owns the Core
sidecar, Core serves the packaged renderer on its allocated loopback origin,
and no Vite preview or fixed port `4173` is required. Use
`npm run build:desktop-package` to rebuild the unpacked product without
launching it. This local directory package is unsigned and is not an installer.

Default artifacts include only the fake conformance Provider. To package and
launch with an independently built plugin, use
`node tools/dev/run-prod.mjs --external-plugin <artifact-root>` so its exact
trust record is generated before Core rebuilds. See the
[extraction and integration note](onedrive-extraction.md).

## Windows development launch

Use `start-desktop.cmd` (or `npm run dev`) for the desktop development build,
and `start-web.cmd` for the Web development server. Both prefer loopback port
5173 and automatically select an available port if Windows reserves it or it
is occupied. The console prints the selected address. Set `MANGOO_WEB_URL` to
use an explicit address; an unavailable explicit port produces an error rather
than changing the requested address.

For isolated desktop verification, set `MANGOO_ELECTRON_USER_DATA_DIR`,
`MANGOO_HOST_DB_PATH`, and `MANGOO_LIBRARIES_DIR` to temporary locations. Both
desktop launchers honor the optional Electron profile directory.

## Docker

This guide covers running Mangoo as a headless service in Docker. Core serves
both the HTTP API and the built React UI, so a single container is enough to
browse a mounted library from any browser on the same network.

> Phase 9 deployment is **single-user**. Multi-tenancy, user accounts, and
> HTTPS termination are explicitly out of scope. Run the container behind a
> reverse proxy (Caddy, nginx, Traefik) when you need TLS or per-user access.

The runtime container drops privileges and runs Mangoo as a non-root system
user (`mangoo`, uid/gid `10001`). The named `mangoo-data` volume is `chown`ed
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

Mangoo never imports media; it indexes the directories you mount. Point the
`MANGOO_LIBRARY_PATH` env var at the host directory you want exposed, and the
container will see it at `/library` (read-only by default):

```bash
MANGOO_LIBRARY_PATH=/srv/photos docker compose up
```

Inside Mangoo, add a root with the path `/library` (or any subdirectory of it,
e.g. `/library/2024`).

If you need write access for file operations (rename / move / recycle), drop
the `:ro` suffix in `compose.yaml`:

```yaml
- ${MANGOO_LIBRARY_PATH:-./library}:/library
```

## Authentication

Mangoo supports two opt-in auth mechanisms:

- **HTTP Basic auth** (`MANGOO_BASIC_AUTH=user:pass`) — front-of-house
  protection for the browser UI and API. Applied to every request except
  `/api/health` so Docker healthchecks keep working without credentials.
  Returns `401 WWW-Authenticate: Basic realm="Mangoo"` on failure. Use this
  when the container is reachable from anything other than your loopback.
- **Session token** (`MANGOO_SESSION_TOKEN=<token>`) — the existing desktop
  mechanism. When set, every request must carry `X-Mangoo-Session: <token>`.
  Leave it **unset** in browser deployments where Basic auth is the only
  gate; the static UI bundle does not yet inject a session token from the
  page.

> **Security: terminate TLS in a reverse proxy.** The Mangoo container speaks
> plain HTTP. HTTP Basic auth credentials are sent base64-encoded in every
> request and are trivially recoverable on the wire. Do **not** expose the
> container directly to an untrusted network. Put it behind a reverse proxy
> (Caddy, nginx, Traefik) that terminates TLS, or restrict it to a private
> network you fully control. The same caveat applies to `MANGOO_SESSION_TOKEN`
> over plain HTTP.

Recommended browser-deploy posture: set `MANGOO_BASIC_AUTH`, leave
`MANGOO_SESSION_TOKEN` unset.

```bash
MANGOO_BASIC_AUTH=alice:s3cret docker compose up
```

For the strictest configuration (e.g. a private LAN where you also run the
desktop client), set both variables.

## Persisting data

The container writes its SQLite database, generated thumbnails, and plugin
state into `/data`, backed by the named volume `mangoo-data`. The volume
survives `docker compose down`; remove it explicitly with
`docker volume rm mangoo-mangoo-data` if you want a clean slate.

Provider runtime state lives in `/data/plugin-state`; encrypted credential
records live separately in `/data/secrets`. Bundled executable providers are
only loaded from the root-owned, read-only `/opt/mangoo/plugins` image path.
Files placed under `/data` are never treated as executable plugins.

The encrypted records require an independently mounted 32-byte master key at
`/run/secrets/mangoo_secret_key`. Mangoo never generates this key in `/data`.
If it is absent or has unsafe permissions, local browsing remains available
and secret-dependent providers report `configuration-required`.

Create the key once, keep it outside the data volume, and restrict access:

```bash
mkdir -p secrets
openssl rand 32 > secrets/mangoo_secret_key
chmod 600 secrets/mangoo_secret_key
docker compose up --build
```

`/library` is the bind mount for your media. Nothing is written there unless
you remove the `:ro` flag.

## Environment variables

| Variable | Purpose | Default |
| --- | --- | --- |
| `MANGOO_CORE_ADDR` | Bind address. | `0.0.0.0:47321` |
| `MANGOO_HOST_DB_PATH` | Host registry SQLite path. | `/data/host/host.sqlite` |
| `MANGOO_LIBRARIES_DIR` | Local catalog/cache storage owned by the Host. | `/data/host/libraries` |
| `MANGOO_PLUGINS_DIR` | Read-only bundled provider root. | `/opt/mangoo/plugins` |
| `MANGOO_PLUGIN_STATE_DIR` | Writable non-secret Provider runtime state. | `/data/plugin-state` |
| `MANGOO_SECRETS_DIR` | Encrypted Provider credential records. | `/data/secrets` |
| `MANGOO_SECRET_KEY_FILE` | Read-only 32-byte master-key file. | `/run/secrets/mangoo_secret_key` |
| `MANGOO_SERVE_WEB` | Serve the bundled UI when `1`. | `1` |
| `MANGOO_WEB_DIR` | Directory containing `index.html` + `assets/`. | `/opt/mangoo/web` |
| `MANGOO_BASIC_AUTH` | Browser/API Basic auth. Compose uses a development credential that must be overridden for production. | `mangoo:change-me` (Compose) |
| `MANGOO_SESSION_TOKEN` | Optional desktop session token. | _(unset)_ |
| `MANGOO_ALLOWED_ORIGIN` | Optional dev CORS origin (exact match). | _(unset)_ |
| `MANGOO_PARENT_PID` | Packaged Desktop PID watched by Core so an unexpected Electron exit also shuts down Core and its Provider jobs. Set automatically by `Mangoo.exe`; do not set manually. | _(unset)_ |
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

The named `mangoo-data` volume is preserved across rebuilds.

## Synchronization release qualification

Run `node tools/checks/sync-release.mjs --renderer-evidence <clean-renderer-evidence.json>`
from a clean commit before publishing an image.
The command builds from a hashed `git archive`, verifies the Windows gate, then
requires an actual Docker/Linux build/runtime row plus a two-container volume
restart proof of the Host Registry, active Library Catalog, sync capture state,
and Provider state. Missing Docker access or required Host evidence keeps the
release blocked. The Host fault matrix contains 16 generic cases.

OneDrive transport, emulator and Personal/Business product OAuth qualification
belongs to the independent plugin repository. Its
[authorization and release guide](../../Mangoo_OneDriveSync/docs/authorization-and-release.md)
also owns the application registration, plugin environment values and manual
Docker authorization instructions. These requirements apply when that plugin
is explicitly included; the default Mangoo image does not include it. See
[sync operations](sync-operations.md) for staged rollout and forward-disable
rollback.

## Limitations

- **Browser UI session token is not injected.** When `MANGOO_BASIC_AUTH` is
  the front-of-house gate, leave `MANGOO_SESSION_TOKEN` unset; the static UI
  cannot currently send `X-Mangoo-Session` from the browser. Tracking work
  for cookie-based session-token wiring is in the master plan.
- **No HTTPS in the image.** Terminate TLS in a reverse proxy.
- **Provider authorization is declared by each plugin.** Optional providers
  must document their Docker completion mode. Authorization records remain in
  the mounted encrypted Secret Store.
- **Single-user Host, multiple Libraries.** One deployed Host owns its Library
  registry and user session. Each managed Library has its own Catalog, roots,
  tags and sync binding; the active Library is switched through Core without
  restarting the process.
- **Windows-only file ops still apply.** Linux containers can mount Windows
  shares, but advanced recycle-bin operations remain Windows-specific. Any
  explicitly included Provider needs a Linux artifact and compiled trust; it
  runs as a supervised process from read-only `/opt/mangoo/plugins`.

## Running without compose

```bash
docker build -t mangoo:dev .
docker run --rm -p 47321:47321 \
  -v mangoo-data:/data \
  -v "$PWD/secrets/mangoo_secret_key:/run/secrets/mangoo_secret_key:ro" \
  -v "$PWD/library:/library:ro" \
  -e MANGOO_BASIC_AUTH=alice:s3cret \
  mangoo:dev
```

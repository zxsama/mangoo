# Mangoo Web / Docker Plan

> **For agentic workers:** Implement task-by-task. Steps use `- [ ]` checkboxes.

**Goal:** Run Mangoo as a headless Core service in a Docker container plus the existing React UI served as static files. Same product UX, different deployment shape.

**Architecture:**
- Core builds as a release binary, packaged in a Docker image (Debian-slim base + ffmpeg).
- The same `apps/web` build artifact (vite `dist/`) is served by Core's HTTP server when `MANGOO_SERVE_WEB=1`.
- Authentication: existing `MANGOO_SESSION_TOKEN` mechanism, plus an opt-in HTTP basic-auth wrapper for browser access.
- Roots are mounted into the container; the user picks which host paths to expose.

**Tech Stack:** Rust (existing), tower-http `services::ServeDir`, multi-stage Dockerfile, GitHub-style `compose.yaml`.

**Scope cuts (deferred):**
- HTTPS termination (use reverse proxy in production).
- Multi-tenant or user accounts.
- Cloud / S3-backed storage.

---

## File Structure

**Modify (backend):**
- `D:/Mangoo/crates/core/src/api/mod.rs` (add `nest_service` for static UI files when env var set)
- `D:/Mangoo/crates/core/src/main.rs` (read `MANGOO_SERVE_WEB`, `MANGOO_BASIC_AUTH`)
- `D:/Mangoo/crates/core/Cargo.toml` (`tower-http` already present; ensure `fs` feature)
- `D:/Mangoo/contracts/core-api/openapi.yaml` (no API changes; document the new env vars in `info.description`)

**Create:**
- `D:/Mangoo/Dockerfile` (multi-stage: builder + runtime)
- `D:/Mangoo/compose.yaml` (sample with mounted roots, healthcheck)
- `D:/Mangoo/.dockerignore`
- `D:/Mangoo/docs/deployment.md` (user-facing deploy doc)

**Modify (frontend):**
- `D:/Mangoo/apps/web/src/core/client.ts` (when `import.meta.env.PROD`, default `baseUrl` to `/api` and `sessionToken` to undefined; let Core's static-serve mode hand cookies/auth instead — sketch the path)
- `D:/Mangoo/apps/web/vite.config.ts` (set `base: '/'` and ensure `dist/` is the deployable folder)

---

## Tasks

### Task 1 — Static UI serving

- [ ] Add `tower-http` `fs` feature; mount `axum::Router::new().nest_service("/", ServeDir::new(web_dir).fallback_method(...).precompressed_gzip())` when `MANGOO_SERVE_WEB=1` AND the dir exists.
- [ ] Index fallback: any non-API path that doesn't match a file falls back to `index.html` for SPA routing.
- [ ] Cache headers: hashed asset filenames are immutable; `index.html` is `Cache-Control: no-cache`.

### Task 2 — Optional HTTP Basic auth wrapper

- [ ] When `MANGOO_BASIC_AUTH=user:pass` is set, wrap the entire app with a `tower::Service` that requires Basic auth on every request EXCEPT `/api/health`. Reject with 401 + `WWW-Authenticate: Basic realm="Mangoo"`.
- [ ] When unset, no Basic auth (loopback-only is the default; in container the user is responsible for restricting network exposure).

### Task 3 — Frontend deploy mode

- [ ] In `apps/web/src/core/desktop.ts`, when no electron bridge is present (browser context), the existing fallback flow needs a `MANGOO_API_PATH` (`/api`) and read session token from a cookie. For Phase 9 keep it simple: rely on Basic auth + same-origin requests; do NOT set `X-Mangoo-Session` from the browser; Core's session-token middleware accepts requests when the env var is unset.
- [ ] The Core `MANGOO_SESSION_TOKEN` env var becomes optional in serve-web mode; `Basic auth` is the front-of-house auth and `X-Mangoo-Session` is the desktop's mechanism.

### Task 4 — Dockerfile + compose

- [ ] Multi-stage Dockerfile:
  - Stage 1 (`rust:1.85-slim` or matching MSRV): `cargo build --release -p mangoo-core`.
  - Stage 2 (`debian:trixie-slim`): install `ffmpeg`, copy binary, copy `apps/web/dist`, set `MANGOO_DB_PATH=/data/mangoo.sqlite`, `MANGOO_SERVE_WEB=1`, expose `47321`.
- [ ] `compose.yaml`: one service `mangoo`, two volumes (`/data` for DB+thumbnails, `/library` mount-point for media), env for `MANGOO_BASIC_AUTH`.
- [ ] `.dockerignore` excludes `target/`, `node_modules/`, `dist/`, `.git/`.

### Task 5 — Docs

- [ ] `docs/deployment.md` with quickstart (`docker compose up`, mount your library, browse to `http://localhost:47321`).
- [ ] Note that Phase 9 deployment is single-user; multi-tenant is future work.

### Task 6 — Verification + commit

- [ ] `cargo build --release -p mangoo-core` (sanity-check)
- [ ] `npm test`
- [ ] Build the Docker image locally if Docker is available; otherwise just lint the Dockerfile via `docker buildx debug` or `hadolint` if installed; if neither, rely on syntax inspection.
- [ ] Update `CLAUDE.md` snippet, master plan checkboxes.
- [ ] Commit `feat: web/docker deployment (phase 9)`.

---

## Acceptance

- A user runs `docker compose up`, navigates to `http://localhost:47321`, sees the same React UI logged in via Basic auth, can add a root mounted at `/library`, browse, scan, search, rate, tag, file-op — same UX as desktop.
- The desktop Electron app keeps working unchanged.

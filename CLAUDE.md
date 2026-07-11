# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Megle is a Windows-first local media browser that indexes existing folders without importing originals. It is contract-first: every cross-process boundary (HTTP API, plugin manifest, SQLite schema) is defined before implementation so the Electron shell, React UI, Rust Core, and tests stay aligned.

The stack: Electron desktop shell, React + TypeScript + Vite UI, Rust + Axum Core service, SQLite WAL (with planned FTS5).

## Common Commands

Run from the repo root.

```text
npm test                  # full verification: structure, contracts, schema, typecheck, rust
npm run dev               # vite dev server + build desktop + spawn electron (dev harness)
npm run check:structure   # required dirs/files, package scripts, openapi paths, plugin caps
npm run check:core-api    # validates contracts/core-api/openapi.yaml shape
npm run check:core-client # @megle/core-client contract-vs-types check + typecheck
npm run check:desktop     # desktop adapter check + typecheck
npm run check:web         # web client check + typecheck
npm run check:schema      # python: applies migrations to a temp sqlite, verifies tables/indexes
npm run check:rust        # cargo fmt --check + cargo test --workspace (skips if cargo missing)
```

Workspace-scoped commands:

```text
npm --workspace @megle/web run build|dev|typecheck|preview
npm --workspace @megle/desktop run build|typecheck
npm --workspace @megle/core-client run check|typecheck
cargo test -p megle-core <test_name>     # run a single Rust test
cargo run  -p megle-core                 # requires MEGLE_SESSION_TOKEN env var
```

`check:schema` requires Python 3. `check:rust` falls back to `~/.cargo/bin` if cargo is not on PATH and prints `SKIP` rather than failing when the toolchain is unavailable. To run `cargo` directly (e.g. `cargo test -p megle-core <test>`) when it is not on the shell PATH, prepend it: `export PATH="$HOME/.cargo/bin:$PATH"` (MSVC toolchain on Windows; install rustfmt once with `rustup component add rustfmt`).

## High-Level Architecture

Three processes, one contract:

```text
Electron main (apps/desktop)
  spawns megle-core.exe via cargo run, holds session token + bind addr
  opens BrowserWindow pointing at the vite dev server or built bundle
    |
React UI (apps/web)
  uses @megle/core-client over HTTP only; never touches the filesystem
    |
Rust Core (crates/core)
  axum router + sqlite + scan/thumbnails/fsops/plugins/watch modules
```

The desktop shell (`apps/desktop/src/main.ts`, `core-process.ts`, `core-session.ts`) generates a session token, allocates a bind address, sets `MEGLE_SESSION_TOKEN` / `MEGLE_CORE_ADDR` / `MEGLE_DB_PATH` / `MEGLE_ALLOWED_ORIGIN`, then spawns Core. `tools/dev/run-dev.mjs` is the dev orchestrator: it starts Vite (forced to `127.0.0.1:5173 --strictPort`), builds the desktop TS, then launches Electron against `apps/desktop/dist/main.js`.

### Security Boundary

- All Core API requests carry `X-Megle-Session: <token>`. Desktop owns the token lifecycle and passes it to both Core (env var) and the renderer (preload bridge).
- Dev CORS is opt-in through `MEGLE_ALLOWED_ORIGIN` and must remain **exact origin only**. Do not introduce wildcard origins or disable Electron web security.
- Electron picks a dynamic loopback port before spawning Core. Explicit `MEGLE_CORE_ADDR` / `MEGLE_CORE_URL` overrides are honored but must agree on host/port. Local URL bind derivation supports `localhost` normalization and IP literals only — arbitrary hostnames are rejected.
- `MEGLE_CORE_EXTERNAL=1` runs against an externally launched Core; in that mode `MEGLE_CORE_URL` and `MEGLE_SESSION_TOKEN` must both be set explicitly. Desktop will not generate a token for external Core mode.
- Token is currently exposed to renderer; future hardening will proxy Core requests through preload/main. Don't design new code around direct renderer-to-Core.

### Contract-First Boundaries

- `contracts/core-api/openapi.yaml` is the single source of truth for HTTP endpoints, DTO names, and status codes. UI, Core, and tests all flow from it.
- `packages/core-client` (`@megle/core-client`) holds a hand-maintained `generated-contract.ts` aligned to the OpenAPI by `scripts/check-contract.mjs`. Treat it as a generator placeholder: edit when the contract changes, never the other way around.
- `contracts/plugins/manifest.schema.json` defines plugin manifest fields and capabilities (`decoder`, `metadata`, `action`, `import-provider`).
- SQLite schema lives in `crates/core/migrations/NNNN_*.sql` (currently `0001`–`0017`; `0014` is applied in code, not as a file). A new migration must be (1) created as the SQL file, (2) declared in `crates/core/src/db/migrations.rs`, (3) gated in `Database::apply_migrations` (`crates/core/src/db/mod.rs`), and (4) asserted in `tools/checks/validate_sqlite_schema.py` (`MIGRATIONS` plus `REQUIRED_TABLES`/`REQUIRED_INDEXES`).
- Some structural checks grep Rust source for implementation markers and **specific test names** (e.g. `tools/checks/validate-core-api.mjs` pins viewport-window markers in `db/mod.rs`). Renaming a guarded function/test or deleting a marked code pattern fails the check — update the check alongside the rename.

When adding an API path, plugin capability, table, or index, update the contract and the corresponding check before wiring code.

### Rust Core Layout (Phase 1)

`crates/core/src` keeps `api`, `db`, `roots`, `scan`, `thumbnails`, `fsops`, `plugins`, `watch`, and `tasks` as **modules inside one crate**. Do not pre-split them into separate workspace crates — `validate-structure.mjs` actively rejects `crates/indexer` and `crates/thumbnails`. Crate-splitting comes only after module boundaries stabilize.

`main.rs` reads `MEGLE_DB_PATH`, `MEGLE_CORE_ADDR`, `MEGLE_SESSION_TOKEN` (required), and `MEGLE_ALLOWED_ORIGIN`, opens the database, applies migrations, and serves `api::router_with_config` on the bind address.

### Web UI Layout

`apps/web/src` is split into `app/` (App shell), `core/` (HTTP client, types, query hooks), `design/` (tokens), and `features/` (`library`, `media-grid`, `preview`, `tasks`). The UI talks to Core via `@megle/core-client`; it must not import Node filesystem APIs or call raw `fetch`. Stack: TanStack Query for server state, TanStack Virtual for the grid, Zustand for UI state.

`apps/web/src/core/client.ts` and `types.ts` are thin runtime config + re-export wrappers over `@megle/core-client`. UI code consumes hooks (`useLibraryData`) or the wrapped client — it must not declare duplicate Core DTOs, call raw `fetch`, or read `thumbnailCacheKey` directly. Web boundary checks (`check:web`) currently use lightweight source regexes; if they start producing false positives or negatives, escalate to AST parsing rather than weakening the rules.

### Approved UI Direction

UI must follow `docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md`: Eagle-like information architecture, frameless Electron chrome, layered liquid glass design system, dense dark content stage. One design language across all screens, menus, dialogs, inspectors, settings, plugins, and task views. Liquid glass lives on chrome and control layers only — the media grid, preview canvas, and other heavy content surfaces stay dark and stable. Implementation stack: Radix UI primitives + Tailwind CSS + Lucide. Don't grow `apps/web/src/styles.css` into the permanent design system; introduce shared `design-tokens` and `ui` package boundaries instead. First implementation milestone is a reusable app shell covering Library, Settings, Plugins, and Tasks before per-feature polish.

### Unified Pointer-Highlight System

All interactive affordances (tree rows, tiles, glass buttons, sliders, list rows, settings nav, the column resizer) share ONE hover highlight: a single screen-space circle centred on the cursor. There is exactly one driver and it lives in `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`. Do not add per-element `:hover` borders/backgrounds or a second glow subsystem — that is the anti-pattern this system replaced.

- **The circle is genuinely one circle across elements.** `LiquidGlassLayer` runs a single global `pointermove → rAF → updateInteractiveAffordancePointers` pass. For each lit target it writes the cursor position in that element's **local px** (`--interactive-pointer-x/y`, can be negative/past the box) plus an on/off gate `--interactive-pointer-opacity`. Because every element uses the same cursor point + the same absolute-px radius, neighbouring rows' gradients line up into one continuous circle. The falloff lives entirely in the CSS radial gradient (`::before` fill + `::after` stroke on `[data-interactive-pointer-target="true"]` in `styles.css`), pow-curve, decaying from the centre — never a per-element uniform opacity (that flattens the circle into a vertical band).
- **Params are in `apps/web/src/features/settings/interfaceStyle.ts`** and adjustable in Settings → Appearance → 交互高光: `pointerSensingDistance` (detect/gate radius, base 80px), `pointerResponseRadius` (visible glow radius ≤ sensing), `pointerFalloff` (pow exponent, default 2.2), `haloBrightness`, `edgeHighlightBrightness`. They emit `--glass-pointer-*` CSS vars; `interfaceStyleToCssVariables` builds the gradient stop lists. Preferences sync across the main and settings OS windows via `BroadcastChannel` (`core/preferenceSync.ts`).
- **Performance is O(near-cursor), not O(all affordances).** The driver keeps a `litAffordanceTargets` Set and only writes style to targets entering/staying/leaving the ignite radius; the hundreds of off-screen virtualized rows are never touched. Rects are cached (`cachedBoundingRect`) and invalidated on scroll (capture phase), resize, and — critically — DOM mutations via a `MutationObserver` on `<body>` (folder expand/collapse grows the sidebar's own scroll container without resizing `<body>`, so a ResizeObserver alone misses it); a layout shift re-anchors the glow to the stationary cursor after a two-frame settle.
- **Opt out** of the glow with `data-pointer-glow="off"` on an element (or ancestor) — honoured in `interactiveAffordanceTargets`. Used for text-like heading buttons and the titlebar search where a rectangular hover ring reads as noise. Selection/active state keeps a distinct background fill (no border ring); the retired `--glass-pointer-x/y/opacity` surface-glow plane was fully deleted.
- **Corner anti-aliasing is a known limitation.** The `::after` stroke uses the CSS `mask-composite` gradient-border trick, which aliases at rounded corners under Electron's realtime compositor at dpr=1 (a documented Chromium behaviour — verified against Blink coordinate-space docs). Real supersampling at unchanged size is impossible on a 1:1/100% display (device-scale-factor and page-zoom are one multiplier and cancel). SVG stroke / Houdini Paint API were prototyped as cleaner alternatives but the improvement was marginal, so the mask-composite stroke stands. **CDP `Page.captureScreenshot` does NOT reflect real on-display corner AA** (it looks clean in captures, jagged on the physical screen) — verify AA-sensitive UI on the real window with the user, not from screenshots.

### Data Model Highlights

The schema separates `roots` / `folders` / `files` / `media` / `user_metadata` / `tags` / `file_tags` / `thumbs` / `thumb_blobs` / `file_operations` so multi-million-file libraries do not duplicate full paths in one wide table. `thumbs` tracks thumbnail *state* (`pending`/`queued`/`ready`/`failed`/`skipped_small`); `thumb_blobs` stores the generated WebP *bytes* as a SQLite BLOB keyed by `(file_id, profile)` — thumbnails live in the DB, not sidecar files (a legacy on-disk cache is migrated in once via `import_legacy_thumbnail_cache`). Queries must use keyset pagination, never large offsets. See `docs/architecture.md` for the full table list and required indexes.

### Thumbnail and Preview Rules

- Thumbnail profiles: `tiny` (96px), `grid` (320 short side), `retina` (640, post-MVP), `preview` (1600).
- Originals smaller than 320px short side record `skipped_small` and the UI uses the original directly, except for non-displayable formats (RAW/PSD/HEIC) which still need WebP.
- All generated thumbnails (including video posters) are WebP.
- Preview switching never blocks the UI on original decoding: show cached preview first, prefetch neighbors, decode on a background queue with interactive preemption.
- Thumbnails are stored as WebP BLOBs in `thumb_blobs` and served by `GET /api/media/{fileId}/thumbnail/blob` (1-year immutable; the client fingerprints the URL by version). A blob row exists only when state is `ready` and the file is active — every reset/disable/delete drops it in the same transaction, so a present blob is always serveable.
- Generation downscales via `DynamicImage::thumbnail_exact` (fast single-pass area-average) then WebP-encodes. There is no reduced-resolution JPEG decode in the pinned `image`/`zune-jpeg`, so do not assume one.

### Core Performance Model

These conventions keep the UI fluid on million-file libraries; preserve them when touching Core hot paths.

- **DB connections.** API write/mutating routes share one `Arc<Mutex<Database>>` (`AppState.database`). The scan worker, the watcher, and read handlers each use *separate* SQLite connections, so reads and background work do not serialize behind the write mutex. Read routes (`run_read_database`) reuse a per-blocking-thread cached connection instead of opening one per request.
- **SQLite is tuned for regenerable data.** File-backed connections run WAL + `synchronous=NORMAL` + 64 MiB `cache_size` + 256 MiB `mmap_size` + `temp_store=MEMORY` (`configure_file_database_wal`, `db/mod.rs`). NORMAL is safe because every table is rebuildable by rescanning — keep it.
- **Thumbnail scheduling is viewport-priority.** Task priorities are `bulk(-10) < background(0) < ahead(20) < visible(30) < selected(40)`, run on Tokio blocking threads gated by per-bucket semaphores (selected=1, foreground=12, background=2). The UI POSTs `/api/thumbnails/priority-scope` (selected/visible/ahead file ids) as it scrolls; Core re-prioritizes only tasks entering that scope or already elevated above background — never the whole root.
- **No per-row work on hot paths.** Batch enqueues commit once (not per file); `list_media` computes `COUNT(*)` only on the first page (later offset windows reuse the client's retained total); the blob route does a single query. Prefer one transaction over a loop of commits, and avoid `COUNT(*)`/full-table-scan/`list_tasks()`-style whole-table loads on per-scroll or per-tile paths.

### File Operations

Megle maps real directories, so file ops are real:
- Default delete goes to the Windows recycle bin; permanent delete needs a second confirmation.
- Same-volume moves use rename; cross-volume uses copy+verify+delete.
- Every operation writes a row to `file_operations` for tracking.

## Anti-Patterns to Avoid

These are enforced by checks or called out in `docs/project-structure.md`:

- A separate `server/` product for Web/Docker — Web/Docker must reuse the same Core and `apps/web`.
- Electron-only feature modules that bypass the Core HTTP API.
- UI code importing `node:fs` or similar to access media.
- DTOs duplicated between desktop, web, and Core instead of flowing from the OpenAPI contract.
- SQLite schema changes without a migration + schema-check update.
- Long-running scan, decode, or file ops on the Electron main or renderer thread.
- Loading full result sets into the frontend; one unsegmented scroll container for large grids.
- Generated thumbnails or DB sidecars written inside user media folders.
- Pre-splitting `crates/media`, `crates/indexer`, `crates/thumbnails`, `crates/fsops`, `crates/plugins` before the `crates/core` modules stabilize.
- Committing benchmark output: `bench-results/`, `tools/bench/*/results/`, generated `node_modules` under `tools/bench/*` are ignored and rejected by structural checks.

## Docs to Read for Bigger Changes

- `docs/architecture.md` — full process model, data model, indexing, thumbnail, and file-op rules.
- `docs/project-structure.md` — directory ownership and what not to create yet.
- `docs/testing-strategy.md` — current test layers and the gates required before feature work.
- `docs/final-solution.md` — locked product rules (Windows first, no managed library, recycle-bin default, plugin-ready).
- `docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md` — approved UI design spec.
- `docs/superpowers/plans/2026-05-16-megle-ui-foundation.md` — UI foundation implementation plan.
- `docs/superpowers/plans/2026-05-16-megle-complete-product-plan.md` — master execution order spanning UI foundation through release hardening.

## Current Implementation State (carried forward from Codex memory)

Phase 1 skeleton with several routes already wired. When extending, build on what exists rather than rewriting:

- `POST /api/roots` persists a `root_scan` task, enqueues it on the in-process Core background worker, and returns `202 { accepted, taskId, rootId, scan: null }`. The scan worker uses a separate SQLite connection so traversal does not hold the API route's `Arc<Mutex<Database>>`.
- `GET /api/roots` returns rows including `rootFolderId` and `lastScanAt`.
- `GET /api/folders/{folderId}/children` and `GET /api/media/{fileId}` return typed records and 404 on missing IDs (not placeholder 200s).
- `GET /api/tasks` returns persisted task rows; `POST /api/tasks/scan` queues another root scan for an existing root.
- Worker startup performs durable recovery: stale `running` root scans reset to `pending`, pending scans run in scheduler order, failed/succeeded tasks do not auto-retry. Enqueue send failure marks the created task `failed` before returning the API error.
- Rust tests cover route display-name fallback, root upsert/listing, DB-backed media page/thumbnail state, scanning a temp root with image/video files (skipping non-media), token auth (missing/wrong/correct), and exact dev CORS preflight.
- `npm test` currently passes end-to-end including `cargo fmt --all --check` and `cargo test --workspace`.
- `npm audit` reports one high-severity issue. **Do not** run `npm audit fix --force` without reviewing package impact.

### Phase Summary (1–10)

Phases 1–10 of the master plan are complete. Each phase ships both Core and UI work inside the same shell, not back-end first.

1. **Phase 1 — App skeleton.** Electron + Vite + Axum + SQLite wired with session-token auth, exact dev CORS, durable scan-task queue, and routes/migrations under one Cargo crate.
2. **Phase 2 — Real directory browsing.** Add root, scan, populate folders/files/media, browse the real folder tree and grid, open the preview shell.
3. **Phase 3 — Thumbnails and preview.** Tiny/grid/preview pipeline, viewport-priority scheduling, neighbor prefetch, stable dark grid surface, no-layout-shift tile loading.
4. **Phase 4 — Task center and watcher.** Background scan/thumbnail queue, persistent `notify` watcher with overflow recovery, task drawer + Task Center page with cancel/retry, recovery on Core restart.
5. **Phase 5 — Metadata, search, organizing.** Tags, ratings, favorites, notes, full-text search, filter chips, sort-by-rating, inspector metadata editing.
6. **Phase 6 — Real file operations.** Rename, move, recycle/permanent delete with validation, conflict handling, atomic DB+filesystem transactions, recent-ops panel.
7. **Phase 7 — Advanced media.** Real image decoder + resizer and FFmpeg sidecar for video posters, populate `width/height/duration/codec`, graceful failure for unsupported formats.
8. **Phase 8 — Plugin manager.** Manifest discovery, enable/disable, capabilities/permissions display, plugin detail inspector. No runtime yet — registration only.
9. **Phase 9 — Web / Docker.** Headless Core mode, Basic auth, mounted roots, HTTP asset delivery, the same React app shell served as static files.
10. **Phase 10 — Release hardening.** Onboarding hero, empty states, Settings page with diagnostics, keyboard shortcuts (F2 / Delete / Shift+Delete / Ctrl+F / Esc), frameless desktop chrome with restored window state, release checklist at `docs/release-checklist.md`.

### Real GUI Integration (2026-05-17)

End-to-end real-photo testing against `C:/Users/84460/Pictures/normal` (31 multi-MB illustrations) shipped three additional fixes that anyone running `npm run dev` against a real directory needs:

- `apps/desktop/src/preload.cjs` (CommonJS, hand-maintained) replaces the old `preload.ts`. Electron's sandboxed preload runtime requires CJS, but the workspace root is `"type": "module"`, so a `.ts → .js` build was loaded as ESM and crashed with "Cannot use import statement outside a module" — the renderer never received `window.megleDesktop`. The `.cjs` is short enough that a static file beats forcing a separate tsconfig.
- `apps/web/src/core/mediaResources.ts::isFreshThumbnailForMediaRecord` no longer requires `mediaRecord.thumbnailCacheKey`. `/api/media` listing intentionally omits per-row thumbnail metadata for performance, so the freshness check used to reject every `ready` thumbnail and tiles stayed in `loading` forever. Now it trusts `ThumbnailResponse.fileId` and the response state.
- The new `GET /api/media/{fileId}/thumbnail/blob` endpoint streams the cached WebP bytes (`image/webp` + 1-year immutable cache), and `apps/web/src/features/media-grid/MediaGrid.tsx::ReadyThumbnail` + `apps/web/src/features/preview/PreviewPanel.tsx::ReadyPreviewImage` fetch the blob through `@megle/core-client::getThumbnailBlob` and render `<img>` elements via `URL.createObjectURL`. Phase 1 only rendered placeholder text in tiles — the cache files existed on disk but never reached the screen.

### Dev Ergonomics

Three optional env vars / scripts make local debugging cheap:

- `MEGLE_AUTO_ADD_ROOT="<path>"` — Electron auto-adds the path as a root after Core is healthy. Lets `npm run dev` come up with data already loaded.
- `MEGLE_REMOTE_DEBUG=1` — `tools/dev/run-dev.mjs` passes `--remote-debugging-port=9500` to Electron so you can attach Chrome DevTools or use the next script. (Default is 9500, not 9222: on Windows 9222 is inside a reserved excluded TCP port range — `netsh interface ipv4 show excludedportrange protocol=tcp` — so Electron can't bind it and CDP silently fails with "Cannot start http server for devtools". Override with `MEGLE_REMOTE_DEBUG_PORT`, but keep it outside the excluded ranges.)
- `tools/dev/cdp-inspect.mjs <ws-url>` — connects to the renderer over CDP and prints `hasBridge`, `coreUrl`, token presence, tile counts (`ready` / `loading` / `failed`), `<img>` count, and recent console errors. Use it instead of guessing whether the UI rendered. Wire-up: `WS=$(curl -sS http://127.0.0.1:9500/json | python -c "import json,sys; print([p for p in json.load(sys.stdin) if p['type']=='page'][0]['webSocketDebuggerUrl'])") && node tools/dev/cdp-inspect.mjs "$WS"`.

Remaining scan work explicitly deferred: transactional scan batching, chunked commits, worker shutdown, progress counters, cancellation/retry semantics, clearer rescan failure/staleness behavior. The Phase 1 dynamic-port probe still has a small probe-and-close bind race before Core starts. Desktop dev still uses `cargo run -p megle-core`; a packaged Core sidecar replaces this for production.

### Desktop Chrome Notes (carried forward)

- **Electron is pinned to `43.1.0`** (`apps/desktop/package.json`). It was upgraded from `34.3.0` specifically to fix a Win11 bug where drag-maximize (Aero Snap / OS maximize path) dropped the acrylic backdrop to solid black. Do not downgrade without re-testing drag-maximize acrylic. The titlebar uses native `-webkit-app-region: drag` (frameless window + Aero Snap + double-click maximize); interactive titlebar children opt out with `no-drag` / `data-no-drag`.
- **CDP screenshots don't match the physical display for anti-aliasing.** `Page.captureScreenshot` (even `fromSurface: true`) renders through a high-quality offscreen path and looks smoother than Electron's realtime GPU compositor. For AA-sensitive work (rounded-corner strokes, sub-pixel glows) you cannot self-verify from screenshots — drive the real window and have the user confirm. Screenshots ARE reliable for layout, state, counts, and whether an element rendered at all.
- **Persisted zoom gotcha.** `webContents.setZoomFactor` persists per-origin in Electron's `Preferences` (userData → `partition.per_host_zoom_levels`) and survives restarts even after the code that set it is reverted. If the UI comes up unexpectedly scaled, clear that key (app stopped) rather than hunting for a code cause.

## Next Execution Order

1. Execute the integrated complete-product plan at `docs/superpowers/plans/2026-05-16-megle-complete-product-plan.md`, starting with the UI foundation track.
2. After UI foundation, write and execute the Phase 2 real-directory-browsing child plan.
3. Harden scanning with transactional batching, recovery, progress, and failure semantics.
4. Add a smoke path for `npm run dev` once GUI launch behavior is ready to verify manually.
5. Harden the Desktop/Web security boundary by proxying Core requests through preload/main instead of exposing the token to the renderer.

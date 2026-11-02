# Mangoo Plugin Manager Plan

> **For agentic workers:** Implement task-by-task. Steps use `- [ ]` checkboxes.

**Goal:** Make plugins a first-class product area: scan a plugin folder, register manifests, enable/disable, surface permissions, runtime logs, and per-plugin settings, all inside the existing app shell.

**Architecture:** A focused Phase 8 slice that DOES NOT yet execute plugin code (no Wasmtime/Extism, no process plugins). Instead it lays the management surface: a registry, manifest validation, persisted enable state, and a Plugins page. Phase 8.5 / 9 will add the runtime once the manager is stable.

**Tech Stack:** Rust + serde + jsonschema for manifest validation; React.

**Scope cuts (deferred):**
- Plugin runtime / sandboxing (Phase 9+).
- Plugin marketplace, install-from-URL.
- Code signing.
- Live reload.

---

## File Structure

**Modify (backend):**

- `D:/Mangoo/contracts/core-api/openapi.yaml`
- `D:/Mangoo/contracts/plugins/manifest.schema.json` (already exists; extend if Phase 8 needs new fields)
- `D:/Mangoo/packages/core-client/src/generated-contract.ts`, `client.ts`, `index.ts`
- `D:/Mangoo/packages/core-client/scripts/check-contract.mjs`
- `D:/Mangoo/crates/core/src/plugins/mod.rs` (currently almost empty)
- `D:/Mangoo/crates/core/src/db/mod.rs` (the `plugins` table already exists; add CRUD helpers)
- `D:/Mangoo/crates/core/src/api/routes.rs` and `mod.rs`
- `D:/Mangoo/crates/core/src/main.rs` (kick off plugin discovery at startup)
- `D:/Mangoo/tools/checks/validate-core-api.mjs`

**Create (backend):**

- `D:/Mangoo/crates/core/src/plugins/manifest.rs` (or inline) — manifest parsing + JSON-schema validation.
- A small fixtures dir under `crates/core/src/plugins/test_fixtures/` for invalid/valid manifests in tests.

**Modify (frontend):**

- `D:/Mangoo/apps/web/src/app/App.tsx` (replace Plugins placeholder with PluginsView)
- `D:/Mangoo/apps/web/src/core/useLibraryData.ts` (or split off a `usePluginsData.ts`)
- `D:/Mangoo/apps/web/src/styles.css`

**Create (frontend):**

- `D:/Mangoo/apps/web/src/features/plugins/PluginsView.tsx` (list, enable/disable, error surface, permissions)
- `D:/Mangoo/apps/web/src/features/plugins/PluginCard.tsx`
- `D:/Mangoo/apps/web/src/features/plugins/PluginDetail.tsx` (right-side inspector)

---

## Contract surface (frozen)

Add OpenAPI paths:

- `GET /plugins` → `{ items: PluginRecord[] }`. Paginated would be over-engineered for now; flat list capped at 500.
- `POST /plugins/discover` → `202 { discovered: number, errors: PluginDiscoveryError[] }`. Triggers a re-scan of the plugin folder; reads manifests; upserts records.
- `POST /plugins/{pluginId}/enable` → `200 PluginRecord` / `404` / `409` (manifest invalid; cannot enable).
- `POST /plugins/{pluginId}/disable` → `200 PluginRecord` / `404`.
- `GET /plugins/{pluginId}` → `200 PluginRecord` / `404` (single plugin with permissions, capabilities, runtime status, last error).
- `DELETE /plugins/{pluginId}` → `200 { deleted: true }` / `404` (removes the row but does NOT delete files; the plugin folder is untouched).

DTOs:

```ts
export type PluginCapability = "decoder" | "metadata" | "action" | "import-provider";
export type PluginStatus = "registered" | "invalid" | "enabled" | "disabled";

export interface PluginRecord {
  id: string;
  name: string;
  version: string;
  description: string | null;
  enabled: boolean;
  status: PluginStatus;
  capabilities: PluginCapability[];
  permissions: string[];
  manifestPath: string;
  installedAt: number;
  updatedAt: number;
  lastError: string | null;
}

export interface PluginListResponse { items: PluginRecord[]; }

export interface PluginDiscoveryError {
  manifestPath: string;
  message: string;
}

export interface PluginDiscoveryResponse {
  discovered: number;
  errors: PluginDiscoveryError[];
}
```

The `plugins` SQLite table already exists; extend with columns if needed via migration `0011_plugins_extended.sql`:

```
description TEXT,
status TEXT NOT NULL DEFAULT 'registered',
capabilities_json TEXT NOT NULL DEFAULT '[]',
permissions_json TEXT NOT NULL DEFAULT '[]',
last_error TEXT
```

The plugin folder is `<MANGOO_PLUGINS_DIR>` (env var; defaults to `./plugins/` next to the DB file). Each plugin lives in its own subfolder containing `plugin.json` (matching `contracts/plugins/manifest.schema.json`).

---

## Tasks

### Task 1 — Manifest parsing & validation

- [ ] Add `jsonschema = "0.18"` (or already-present version) — actually skip if already present; check `Cargo.lock`. If not, prefer ad-hoc validation against the schema since manifest fields are small and deterministic.
- [ ] `plugins::manifest::parse(path) -> Result<ManifestRecord, ManifestError>`: read file, parse JSON, validate against schema, return typed `ManifestRecord` plus any validation errors as `ManifestError` variants (`MissingField`, `InvalidCapability`, `InvalidVersion`, etc).

### Task 2 — DB helpers

- [ ] migration 0011 adds the new columns.
- [ ] `db::list_plugins`, `db::get_plugin(id)`, `db::upsert_plugin(record)`, `db::set_plugin_enabled(id, bool)`, `db::set_plugin_status(id, status, last_error)`, `db::delete_plugin(id)`.

### Task 3 — Discovery & API

- [ ] `plugins::discover(plugins_dir) -> (Vec<ManifestRecord>, Vec<DiscoveryError>)`. Walk first-level subfolders, read `plugin.json`, validate. UPSERT each.
- [ ] Wire into `main.rs` as a startup call (best-effort; log on failure).
- [ ] Axum handlers + route constants + validate-core-api.mjs.

### Task 4 — Frontend

- [ ] `usePluginsData()` hook (separate file): `plugins: PluginRecord[]`, `selectedPluginId`, `discover()`, `enable(id)`, `disable(id)`, `remove(id)`.
- [ ] `PluginsView`: list of `PluginCard` on the left, `PluginDetail` on the right when a plugin is selected. "Re-scan" button triggers `discover`.
- [ ] `PluginCard`: name, version, status pill (registered/invalid/enabled/disabled), enable toggle disabled when status==invalid, lastError tooltip.
- [ ] `PluginDetail`: capabilities chips, permissions list, manifest path (copy button), full lastError.
- [ ] App.tsx replaces the Plugins placeholder with `<PluginsView />`.

### Task 5 — Verification + commit

- [ ] `npm test`.
- [ ] Update `CLAUDE.md` snippet, master plan checkboxes.
- [ ] Commit `feat: plugin manager (phase 8)`.

---

## Acceptance

- A user drops `plugins/sample-plugin/plugin.json` (valid manifest) under `MANGOO_PLUGINS_DIR`, clicks Re-scan, the plugin appears registered in the Plugins page, can be toggled enabled/disabled, and the row persists across Core restarts.
- An invalid manifest shows up with `status=invalid` + lastError, cannot be enabled.
- Removing a plugin row leaves the on-disk folder alone.

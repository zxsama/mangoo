# Mangoo Metadata, Search, And Organizing Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use `- [ ]` checkboxes for tracking.

**Goal:** Add tags, ratings, favorites, notes, search, filter chips, and sort-by-rating so a user can browse, search, organize, and refilter the same library inside one product flow.

**Architecture:** Reuse existing `roots/folders/files/media/user_metadata/tags/file_tags/media_fts` SQLite schema, extend Core API with metadata + search endpoints, expose them through `@mangoo/core-client`, and add UI surfaces inside the existing app shell (toolbar search, filter chips, inspector edit).

**Tech Stack:** Rust + Axum + rusqlite + SQLite FTS5; React + TypeScript + TanStack Query + Zustand + Lucide.

**Scope cuts (deferred):**
- Saved views (out — release hardening).
- Color-by-tag in grid (out — advanced media phase).
- Bulk-edit toolbar for many selected items (out — file ops phase will reuse selection model; for now we support per-item edit and per-selection bulk-tag in the inspector).

---

## File Structure

**Modify (backend):**

- `D:/Mangoo/contracts/core-api/openapi.yaml` — add metadata + tag + search paths and DTOs.
- `D:/Mangoo/packages/core-client/src/generated-contract.ts` — add the new TypeScript shapes.
- `D:/Mangoo/crates/core/src/db/mod.rs` — DB helpers for user_metadata, tags, file_tags, FTS sync, search query.
- `D:/Mangoo/crates/core/src/api/routes.rs` and `mod.rs` — new route constants, handlers, axum wiring.
- `D:/Mangoo/tools/checks/validate-core-api.mjs` — assert new paths/DTO names/sort enums are aligned.

**Create (backend):**

- `D:/Mangoo/crates/core/migrations/0010_metadata_indexes.sql` — only if a missing hot-path index is found while implementing search; otherwise skip.

**Modify (frontend):**

- `D:/Mangoo/apps/web/src/core/useLibraryData.ts` — query/filter/sort state, search debouncing, mutation helpers.
- `D:/Mangoo/apps/web/src/app/App.tsx` — wire new toolbar search, filter chips, sort menu into the Library shell.
- `D:/Mangoo/apps/web/src/styles.css` — classes for chip row, rating stars, tag pills.

**Create (frontend):**

- `D:/Mangoo/apps/web/src/features/library/SearchBar.tsx` — toolbar search.
- `D:/Mangoo/apps/web/src/features/library/FilterChips.tsx` — kind/rating/favorite/tag chips.
- `D:/Mangoo/apps/web/src/features/library/SortMenu.tsx` — sort-by selector.
- `D:/Mangoo/apps/web/src/features/preview/InspectorMetadata.tsx` — rating stars, favorite toggle, tag editor, note edit (used inside existing preview/inspector area).
- `D:/Mangoo/apps/web/src/features/library/TagChip.tsx` — small reusable chip.

---

## Contract Surface (frozen for both agents)

Add to `contracts/core-api/openapi.yaml` paths:

- `GET  /tags` → `TagListResponse` `{ items: TagRecord[] }`
- `POST /tags` body `{ name, color? }` → `201 TagRecord` / `409` (duplicate) / `400` (invalid name).
- `DELETE /tags/{tagId}` → `200 { deleted: true }` / `404`.
- `PUT  /media/{fileId}/metadata` body `UserMetadataUpdate` → `200 UserMetadataRecord`. Partial update semantics: undefined keys are not touched; explicit `null` clears `rating`/`note`.
- `GET  /media/{fileId}/metadata` → `200 UserMetadataRecord` (always present; default record returned for files with no row).
- `PUT  /media/{fileId}/tags` body `{ tagIds: number[] }` → `200 { fileId, tagIds: number[] }` (replaces full set).
- `POST /media/{fileId}/tags` body `{ tagId }` → `200 { fileId, tagIds }` / `404`.
- `DELETE /media/{fileId}/tags/{tagId}` → `200 { fileId, tagIds }` / `404`.
- `GET  /search` query: `q?` (string, FTS), `rootId?`, `folderId?`, `kind?`, `minRating?`, `favorite?` (bool), `tagId?` (repeatable; ALL must match), `sort=mtime_desc|mtime_asc|name_asc|name_desc|rating_desc|rating_asc`, `limit`, `cursor` → `MediaListResponse` (same shape as `/media`, items also carry `rating`, `favorite`, `tagIds`).

Extend existing `MediaRecord` (`packages/core-client/src/generated-contract.ts` and OpenAPI schema):

```ts
export interface MediaRecord {
  // existing fields…
  rating?: number | null;       // 0-5 inclusive
  favorite?: boolean;            // default false
  note?: string | null;
  tagIds?: number[];             // omitted in /media; populated in /search and /media/{id}
}
```

New shapes:

```ts
export interface TagRecord { id: number; name: string; color: string | null; }
export interface TagListResponse { items: TagRecord[]; }

export interface UserMetadataRecord {
  fileId: number;
  rating: number | null;
  favorite: boolean;
  note: string | null;
  tagIds: number[];
  updatedAt: number;
}

export interface UserMetadataUpdate {
  rating?: number | null;       // 0..5 or null to clear
  favorite?: boolean;
  note?: string | null;         // null clears
}

export interface SearchParams {
  q?: string;
  rootId?: number;
  folderId?: number;
  kind?: "image" | "video" | "other";
  minRating?: number;            // 1..5
  favorite?: boolean;
  tagIds?: number[];             // AND across tags
  sort?: "mtime_desc" | "mtime_asc" | "name_asc" | "name_desc" | "rating_desc" | "rating_asc";
  limit?: number;
  cursor?: string;
}
```

**Both agents must adhere to these names and shapes verbatim.** If something is impossible, STOP and report.

---

## Task 1: Backend — schema check + DB helpers + FTS sync

- [ ] Add DB helpers in `crates/core/src/db/mod.rs`:
  - `list_tags()`, `create_tag(name, color)`, `delete_tag(id)` (cascades via FK).
  - `get_user_metadata(file_id)`, `upsert_user_metadata_partial(file_id, patch)` (writes `updated_at = unixepoch()`).
  - `set_file_tags(file_id, tag_ids)` (replace all), `add_file_tag(file_id, tag_id)`, `remove_file_tag(file_id, tag_id)`, `list_file_tag_ids(file_id)`.
  - `sync_media_fts_for_file(file_id)` — rebuild that file's FTS row from `files.name`, `user_metadata.note`, and joined tag names.
  - `search_media_page(query)` — keyset paginated query joining `files`, `media`, `user_metadata`, and applying FTS5 `MATCH` only when `q` is non-empty.
- [ ] Wire FTS sync after every mutation that touches name/note/tags. Files inserted by scan/watcher already exist; ensure scan upserts also write a baseline FTS row (name only).
- [ ] Add Rust unit tests for: tag CRUD + duplicate name rejection, partial metadata update, set_file_tags replacement semantics, FTS search hit on note/tag/name, search filter combinations (kind+rating+tag AND, favorite, sort-by-rating with NULL ratings sorted last).

## Task 2: Backend — API routes and contract

- [ ] Edit `contracts/core-api/openapi.yaml` to add the paths and DTOs from "Contract Surface" above. Add `rating`, `favorite`, `note`, `tagIds` to `MediaRecord`.
- [ ] Update `validate-core-api.mjs` to require these new path fragments, sort enum values (`rating_desc`, `rating_asc`), and DTO names.
- [ ] Update `packages/core-client/src/generated-contract.ts` to add the new types.
- [ ] Implement axum handlers and add them to `crates/core/src/api/routes.rs` route constants and `mod.rs` router. Reuse session/CORS middleware. Validate inputs (rating 0..=5, name non-empty, max name length 64, tag color hex regex if provided).
- [ ] Add Rust API integration tests for each new path: success + 400 + 404 + 409 cases.
- [ ] Run: `cargo test -p mangoo-core`, `npm run check:core-api`, `npm run check:core-client`, `npm run check:schema`.

## Task 3: Frontend — search bar, filter chips, sort menu

- [ ] Extend `apps/web/src/core/useLibraryData.ts` to expose `searchState` (`q`, `kind`, `minRating`, `favorite`, `tagIds`, `sort`) plus `setQ` (debounced 250ms), `setFilter`, `setSort`, and `searchResults` driven by `/search`. When the search state is empty, fall back to existing `/media` listing so the user sees the full library.
- [ ] Add `tags` query (`/tags`) and `tagsById` map.
- [ ] Build `SearchBar` (lucide search icon + input + clear button), `FilterChips` (kind, ⭐ rating, favorite, tag dropdown), `SortMenu` (4 existing + 2 new sort modes).
- [ ] Mount them inside `App.tsx`'s Library tab toolbar. Keep existing folder tree + grid working.
- [ ] No raw `fetch`. All calls go through `@mangoo/core-client` via the wrapped client in `apps/web/src/core/client.ts`.
- [ ] Verify: `npm run check:web`, `npm run check:core-client`, `npm --workspace @mangoo/web run typecheck`, `npm --workspace @mangoo/web run build`.

## Task 4: Frontend — inspector metadata editor

- [ ] Create `InspectorMetadata.tsx` rendering rating stars (0–5 clickable, click on currently-set value clears), favorite heart toggle, note textarea (autosave on blur, 500-char soft limit with counter), and tag editor (search-as-you-type, create-on-Enter when no match, click chip to remove).
- [ ] All mutations go through `@mangoo/core-client` and use TanStack Query optimistic updates with rollback on error.
- [ ] When a media item is selected (existing preview state), `InspectorMetadata` shows under preview/info area without changing the existing layout.
- [ ] Verify: same web checks as Task 3.

## Task 5: Wire-up + final verification

- [ ] Update `CLAUDE.md`'s "Current Implementation State" snippet.
- [ ] Update `docs/superpowers/plans/2026-05-16-mangoo-complete-product-plan.md` Task 5 checkboxes.
- [ ] Run `npm test` and confirm clean.
- [ ] Commit with `feat: add metadata search and organizing (phase 5)`.

---

## Acceptance

- A user adds a real root, browses real folders, marks 3 favorites, rates 5 items, applies a tag to 10 items, searches for a substring matching name + note + tag, sees only matching items, and can clear filters back to the full library — all inside the same shell.
- All Rust + Node verification passes.
- No regression in Phase 4 watcher + task center.

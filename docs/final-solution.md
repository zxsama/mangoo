# Megle Final Solution

Updated: 2026-05-16

## Final Direction

Build Megle as a Windows-first local media browser and manager that indexes existing folders without importing originals into a private library.

Final architecture:

```text
Electron Desktop Shell
  |
React/TypeScript Web UI
  |
Local Core API
  |
Rust Core Service
  |
SQLite + Thumbnail Cache + Plugin Workers
```

This is not a clone of any existing product and does not depend on reverse engineering. It is an independent implementation optimized for very large local media libraries.

## Non-Negotiable Product Rules

- Windows first.
- Only index existing directories.
- Do not copy originals into a managed library.
- Left folder tree maps real directories.
- Tags, ratings, notes, favorites live in Megle database.
- Real file rename, move, and delete are allowed.
- Default delete goes to recycle bin.
- Plugin system is prepared from the start.
- Browser extension is a later import-provider plugin.
- Web/Docker is a future deployment mode, not a different product.

## Recommended Stack

Use this stack for the first implementation:

```text
Desktop:       Electron
UI:            React + TypeScript + Vite
Components:    Radix UI + Tailwind CSS + Lucide
Virtual grid:  TanStack Virtual
Server state:  TanStack Query
UI state:      Zustand
Core:          Rust + Tokio + Axum
Database:      SQLite WAL + FTS5 + rusqlite
Filesystem:    walkdir/jwalk + notify + windows-rs
File ops:      windows-rs + trash-rs
Images:        libvips primary, image/fast_image_resize fallback
Videos:        FFmpeg sidecar
Metadata:      exif-rs first, ExifTool sidecar optional
Search:        SQLite FTS5 first, Tantivy later
Plugins:       process plugins first, Wasmtime/Extism later
```

## Why This Route

The main performance risk is not the UI framework. It is accidental full-load behavior:

- loading all files into frontend memory
- rendering too many DOM nodes
- generating all thumbnails before browsing
- hashing every file during scan
- decoding original images on the UI path
- treating watcher events as reliable truth

This architecture prevents those problems by moving all heavy work to a Core Service and making every large operation paged, queued, cached, and cancellable.

## Process Model

```text
Megle.exe
  |
  |-- Electron main
  |     - window lifecycle
  |     - tray/menu/dialogs
  |     - starts Core
  |     - owns local session token
  |
  |-- React renderer
  |     - virtual grid
  |     - previewer
  |     - folder tree
  |     - metadata panel
  |     - settings/plugins/tasks
  |
  |-- megle-core.exe
        - HTTP/named pipe API
        - SQLite
        - scan queue
        - thumbnail queue
        - preview prefetch
        - file watcher
        - real file ops
        - plugin host
```

The Core API should be shaped like a local server API from day one. Future Docker/Web mode can reuse the same API with authentication and mounted root permissions.

## Data Model

Use normalized path storage:

- `roots`
- `folders`
- `files`
- `media`
- `user_metadata`
- `tags`
- `file_tags`
- `thumbs`
- `tasks`
- `file_operations`
- `plugins`

Important rules:

- Store full root path once.
- Store folders hierarchically.
- Store file names separately from folders.
- Use stable numeric ids internally.
- Store derived media metadata separately from file identity.
- Keep user metadata separate from source file metadata.

Key query rules:

- keyset pagination only
- indexed folder queries
- indexed filters
- FTS5 for text search
- no large offset paging
- hot browse queries must be driven by the final sort index
- avoid filtering through a joined table and then sorting a large result set

Example:

- Good: scan `files(mtime DESC, id DESC)` and verify media kind per candidate.
- Risky: scan `media(kind)` first, join files, then sort all matching rows.

## Thumbnail Strategy

Use these thumbnail profiles:

- `tiny`: 96px
- `grid`: shortest side 320px
- `retina`: shortest side 640px, optional after MVP
- `preview`: 1600px

Grid thumbnail rule:

- If source shortest side is below 320px, do not generate a standalone grid thumbnail.
- If source shortest side is 320px or larger, generate a WebP grid thumbnail with shortest side 320px.
- Generated thumbnail cache files use `.webp` / `image/webp`.
- Video poster thumbnails also use WebP.
- Store a database state such as `skipped_small` for files that intentionally have no generated thumbnail.
- `skipped_small` applies only when the original source format is directly displayable by the UI. Unsupported source formats still need generated WebP thumbnails.

Cache phase:

1. MVP: hash-sharded file cache.
2. Later: thumbnail pack files if NTFS small-file pressure becomes measurable.

Generation priority:

1. current preview item
2. current viewport missing thumbnails
3. selected item neighbors
4. opened folder background fill
5. whole-library background fill

Do not block browsing on thumbnail completion.

## Preview And Switching

Switching images must update selection immediately.

Flow:

1. User presses left/right.
2. UI changes selected id.
3. UI displays cached preview or lower-resolution thumbnail.
4. Core cancels stale interactive work.
5. Core prioritizes current item and neighbor prefetch.
6. Full decode or video stream loads asynchronously.

Target:

- selection state update under 50ms
- cached preview visible under 150ms
- no UI thread decode
- rapid key-repeat should coalesce stale intermediate work
- only the newest selected item remains interactive priority

For huge images, add tile pyramid/deep zoom later. Do not decode giant originals directly for every zoom interaction.

## Scanning Strategy

Initial scan:

1. Add root.
2. Enumerate folders/files quickly.
3. Insert basic rows in transactions.
4. UI becomes browsable.
5. Background jobs extract metadata and thumbnails.

Do not:

- generate all thumbnails before browsing
- hash every file in the first pass
- read all EXIF/video metadata synchronously
- decode image/video content in the traversal loop
- create any cache files inside the user's media folders

Incremental scan:

- notify watcher receives events.
- events enqueue verification.
- verification uses stat/local rescan.
- overflow or sleep/resume triggers broader rescan.
- later explore NTFS USN Journal for huge Windows roots.

## File Operations

Because Megle operates on real directories, every file operation must be transactional from the user's point of view.

Rename:

- validate target
- fail if target already exists unless overwrite is explicitly requested
- execute filesystem rename
- update database
- log operation

Move:

- same volume: rename/move
- cross volume: copy + verify + delete-to-recycle-bin or safe cleanup
- fail on target conflict unless overwrite is explicitly requested
- show progress
- log every item

Delete:

- default recycle bin
- permanent delete is advanced mode
- database marks deleted after filesystem success

All operations write `file_operations` records.
Failed operations write logs but do not mutate file rows.

## Plugin Strategy

Implement plugin scaffolding early, but keep plugins out of the hot path unless needed.

First plugin model:

- process-based
- manifest
- permissions
- enabled/disabled state
- logs
- timeouts
- memory/process isolation

Plugin types:

- decoder
- metadata
- action
- import provider

Browser extension later becomes an import provider:

```text
Browser extension -> Megle Native Host/Core -> user-selected real folder -> watcher/indexer
```

WASM plugins through Wasmtime/Extism are optional later, mainly for logic plugins rather than native media decoders.

## UI Direction

Layout:

- left real-folder tree
- center media grid
- right metadata panel
- top toolbar with filters/search/sort
- bottom or side task drawer
- modal quick preview

Interaction:

- Space/Enter preview
- arrow keys switch
- grid zoom slider
- multi-select
- context menu
- drag to move/copy with confirmation
- task progress for scans and file operations

Visual:

- restrained acrylic/frosted glass on chrome, not media grid
- dense asset-management UI
- stable dimensions for grid tiles
- no layout shift from thumbnail loading
- performance mode to reduce blur/shadows

Large result-set rule:

- Do not represent a 5M-item all-library view as one unsegmented DOM scroll container.
- Chromium scroll height limits appear around `33.5M px`.
- Use segmented result windows, folder-first browsing, and keyset-paged query chunks for very large views.

## Web/Docker Path

The future Web/Docker version should reuse:

- React UI
- Core API
- SQLite schema
- thumbnail cache
- plugin model

Docker differences:

- roots are mounted volumes
- must have auth
- root permissions can be read-only/read-write
- no Windows recycle bin
- video and thumbnails served through HTTP/range streaming

Do not fork a separate Web product.

## Implementation Phases

### Phase 0: Performance Gates

Status: completed on 2026-05-16.

1. SQLite 1M/5M rows.
2. Concurrent foreground reads and background writes.
3. Paged API query and serialization.
4. TanStack Virtual grid with paged API.
5. Thumbnail generation queue.
6. Preview switching and prefetch.
7. Real file operation consistency.

Phase 0 raw JSON reports are archived under `docs/performance-results/raw/2026-05-16/`.

### Phase 1: App Skeleton

- Electron app.
- React shell.
- Rust Core sidecar.
- local API token.
- root add/remove.
- SQLite schema.

### Phase 2: Real Directory Browsing

- scan roots
- lazy folder tree
- media grid
- basic preview
- basic metadata

### Phase 3: Thumbnail Pipeline

- tiny/grid/retina/preview profiles
- viewport priority
- cache lookup API
- background fill

### Phase 4: Watcher And Incremental Indexing

- root watcher
- add/delete/rename/move event handling
- watcher overflow local rescan
- background task panel

### Phase 5: Metadata And Search

- tags
- ratings
- favorites
- notes
- filters
- FTS5 search

### Phase 6: File Operations

- rename
- move
- delete to recycle bin
- operation log
- error recovery

### Phase 7: Video And Advanced Formats

- FFmpeg poster frames
- video metadata
- video preview
- decoder plugins for long-tail formats

### Phase 8: Plugins

- manifest
- plugin manager UI
- decoder/action/import-provider interfaces
- browser extension later

### Phase 9: Web/Docker

- headless Core mode
- auth
- mounted roots
- static Web UI serving
- Dockerfile

## Final Decision

Proceed with a custom component-based implementation:

```text
Electron UI shell + Rust Core + SQLite + libvips/FFmpeg + plugin-ready architecture
```

This has higher initial cost than modifying an existing app, but it is the most controlled path for:

- millions of files
- real directory mapping
- fast preview switching
- true Windows file operations
- future Web/Docker reuse
- clear licensing

## Immediate Next Step

Start Phase 1 with the smallest working app skeleton that preserves the proven performance constraints.

Create:

- Monorepo structure for `apps/desktop`, `apps/web`, and Core crates/services.
- Core API contract for roots, folders, media pages, thumbnails, preview, and file operations.
- SQLite schema v0 and migrations.
- Real-directory root add/scan path.
- Segmented/windowed React grid path; do not model 5M items as one DOM scroll range.
- Thumbnail queue skeleton using WebP and the 320px shortest-side `grid` rule.

Keep the UI minimal until the scan, page query, thumbnail, and preview paths are integrated end to end.

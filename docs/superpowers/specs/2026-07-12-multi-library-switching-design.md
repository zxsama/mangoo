# Multi-Library Switching, Loading Progress, and Library Management — Design

> Status: approved design (brainstorming). Next step: `writing-plans`.
> Date: 2026-07-12

## Goal

Turn Megle's single fixed database into multiple switchable **libraries** (one library = one `.sqlite` file, each containing many root folders), show a centred loading card during app startup and library switching, and move library/root management into a dedicated switcher popup + Settings.

## Confirmed Product Decisions

1. **One active library at a time.** Switching a library = gracefully restarting the Core process pointed at the new `.sqlite` file; the previous library's Core (scans, thumbnail generation) is fully stopped. No background keep-alive of inactive libraries.
2. **A library is a single `.sqlite` file.** A library contains many root folders (`roots` rows inside that DB — the existing mechanism, unchanged). Root management is a within-library operation; library switching is the top-level operation.
3. **Library registry lives in `userData/libraries.json`** (Electron main process, alongside `window-state.json`), decoupled from any DB. It records the library list + the active library id.
4. **Sidebar top = current-library name + a "switch library" entry only** (opens the switcher popup). The switcher popup's per-library `...` menu gains "add folder" / "remove folder" (active library only; greyed out for inactive libraries). Full library management lives in Settings.
5. **Loading UI:** on cold start and on library switch, the window chrome (titlebar) + acrylic/glass background are preserved; the content area shows a centred glass loading card with an **indeterminate spinner + phase text** (not a real percentage bar). Scan/thumbnail progress stays in the Task Center. Switching may blank the content and reset UI state (expanded tree, scroll) — only the chrome + glass background must persist.
6. **Switch reconnect = reuse port + token (soft reload).** The restarted Core reuses the same bind address and session token, so the renderer's `coreUrl`/`token` are unchanged and no `window.reload()` is needed — the renderer just re-initializes its data. No white flash.
7. **Create/open auto-switches.** Creating a new library (or opening an existing `.sqlite` via "open other library") adds it to the registry and immediately switches to it.
8. **Inactive-library root buttons are disabled.** "Add folder" / "remove folder" only ever act on the active library; for inactive rows the buttons are greyed out and non-clickable.

## Architecture

### Library lifecycle

```
App start
  → main reads userData/libraries.json
  → active library id → its dbPath → start Core (MEGLE_DB_PATH = dbPath)
  → (no libraries at all → "no library" onboarding: create / open)

Switch library (user clicks a row in the switcher popup)
  → renderer IPC: megle:switch-library(libraryId)
  → main: push phase "starting-core" → restartCoreForLibrary(newDbPath)
           → stop old Core (existing tree-kill) → wait for port release
           → build session reusing SAME bindAddress + SAME sessionToken, new dbPath
           → startCoreProcess → waitForCoreHealth  (push phase "opening-library")
           → update registry activeLibraryId + lastOpenedAt
  → main IPC → renderer: megle:library-ready
  → renderer: useLibraryData re-initializes (phase "loading-media") → listRoots/loadMedia
           → phase "ready" → loading card fades out
```

Because port + token are reused, the renderer's injected `coreUrl`/`sessionToken` (baked into the window at creation) stay valid across the restart.

### Core (Rust) — zero change

Core already starts from `MEGLE_DB_PATH`, opens the DB, and runs `apply_migrations()`. `apply_migrations` runs `INITIAL_MIGRATION` (all base tables) unconditionally then numbered migrations gated by `migration_applied(n)` (`crates/core/src/db/mod.rs:623`), so a **brand-new empty `.sqlite` is fully migrated on first open**. Multi-library therefore needs no Core changes: creating a library = main process picks a new file path and starts Core there; Core builds the schema. No new API paths, tables, or indexes → OpenAPI/schema checks unaffected.

### Electron main process — main work

- **New `apps/desktop/src/library-registry.ts`** — read/write `userData/libraries.json`; CRUD libraries; set active; validate `dbPath` existence (`available` flag). Pure logic, unit-testable.
- **`main.ts` Core lifecycle** — the write-once `coreSession`/`coreReadyPromise` become restartable. Add `restartCoreForLibrary(dbPath)`: stop old Core (existing `stopCoreProcessTree`), wait for the bind port to be re-bindable (guards the known probe-and-close race), build a session reusing the same `bindAddress` + `sessionToken` with the new `dbPath`, `startCoreProcess`, `waitForCoreHealth`, push load phases over IPC.
- **New IPC handlers** — `listLibraries`, `getActiveLibraryId`, `switchLibrary`, `createLibrary`, `openLibrary`, `removeLibraryFromList`, `renameLibrary`, `pickLibraryFile`, `pickNewLibraryPath`, plus the `megle:library-load-phase` event push.
- **`preload.cjs`** — expose the new bridge methods.
- **Cold start** — read registry → active dbPath → existing startup flow (off-screen → shell-ready → reveal, now revealing to a loading card). No libraries → "no library" onboarding.

### Renderer non-UI

- **`useLibraryData`** — add a "re-initialize" entry point (reuse `loadLibrary`) triggered by the `megle:library-ready` IPC so a switch clears and reloads roots/media.
- **`apps/web/src/core/desktop.ts`** — extend `MegleDesktopBridge` type + wrapper with the new methods and the phase subscription.

## Frozen Interfaces (UI depends on these — freeze first to unblock parallel UI work)

### Bridge methods (`window.megleDesktop` / `MegleDesktopBridge`)

```ts
listLibraries(): Promise<LibrarySummary[]>
getActiveLibraryId(): Promise<string | null>
switchLibrary(libraryId: string): Promise<{ ok: boolean; error?: string }>
createLibrary(input: { name: string; dbPath: string }): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }>
openLibrary(dbPath: string): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }>
removeLibraryFromList(libraryId: string): Promise<{ ok: boolean }>   // list-only removal; never deletes the file
renameLibrary(libraryId: string, name: string): Promise<{ ok: boolean }>
pickFolder(): Promise<string | null>                 // EXISTS today
pickLibraryFile(): Promise<string | null>            // NEW: pick an existing .sqlite ("open other library")
pickNewLibraryPath(defaultName?: string): Promise<string | null>  // NEW: pick save location for a new .sqlite
onLibraryPhase(cb: (phase: LibraryLoadPhase) => void): () => void  // subscribe; returns unsubscribe
```

`createLibrary` and `openLibrary` auto-switch on success (they internally trigger the switch flow and resolve after the registry is updated; the phase events drive the loading card).

### Types (`apps/web/src/core/libraryTypes.ts`, mirrored in preload/main)

```ts
interface LibrarySummary {
  id: string;
  name: string;
  dbPath: string;
  available: boolean;   // dbPath exists on disk (false → warning icon, greyed, like the Eagle "Test" state)
  active: boolean;
}

type LibraryLoadPhase =
  | "starting-core"
  | "opening-library"
  | "loading-media"
  | "ready"
  | { kind: "error"; message: string };
```

### Root management — reuse existing `@megle/core-client`

`addRoot` / `removeRoot` / `enqueueScan` / `listRoots` already exist. The switcher popup's "add folder" / "remove folder" call these **only for the active library**; inactive rows keep the buttons greyed out. No new root interface.

## UI Components (parallelizable — build against the frozen interfaces with a mock bridge)

- **UI-1 — `LibrarySidebar` top refactor.** Remove the `root-form` (path input, Add, choose-folder, rescan). Replace with a current-library-name label + a switch-library entry (opens `LibrarySwitcher`). Keep the folder tree and a whole-library refresh button. Root add/remove now lives in the switcher popup + Settings.
- **UI-2 — `LibrarySwitcher` popup** (glass popup, mirrors the Eagle reference). Search box; library rows = icon + name + dbPath (muted) + active checkmark + `...` menu (add folder / remove folder [active only, else greyed] / rename / remove from list); unavailable libraries show a warning icon + greyed row; clicking a non-active row switches. Footer: "创建资源库…" (pick location + name → create empty DB → auto-switch), "打开其它资源库…" (pick `.sqlite` → add to list → auto-switch).
- **UI-3 — `LibraryLoadingOverlay` + App gating.** Centred glass card with indeterminate spinner + phase text ("正在启动核心…" / "正在打开库…" / "正在加载媒体…"); covers the content area only (chrome + glass background stay). App gates on an app-level `libraryPhase` (from `onLibraryPhase` + `useLibraryData.loading`): not-ready → AppShell + overlay; ready → normal content. No-library state → onboarding (reuse OnboardingHero shape, CTA = create/open library). Error phase → error card with retry / switch-library actions (not an infinite spinner).
- **UI-4 — Settings "资源库" section.** Full library list + CRUD + per-library root management (active library). Superset of the switcher; lower priority.

## Error Handling

- Core start failure / corrupt DB / missing path → the loading card becomes an error card (message + "重试" / "切换其它库"); `LibraryLoadPhase` carries `{ kind: "error", message }`.
- Missing library file (registry entry whose `dbPath` no longer exists) → `available: false`, greyed in the switcher, cannot be switched to; offer remove-from-list.
- Port not released after stopping old Core → main polls for re-bindability before starting the new Core (bounded retries; surfaces an error phase on timeout).

## Testing

- **`library-registry.ts`** — unit tests (JSON read/write, CRUD, active-id, path validation).
- **Core** — zero change expected; optionally add a Rust test that opens a brand-new empty file, migrates, and `list_roots()` returns empty, to lock the "empty DB → schema built" guarantee.
- **UI** — each UI subtask against a mock bridge; guarded by `npm run check:web` / `check:ui-design`.
- **Bridge boundary checks** — new methods likely trip `check:desktop` / `check:web` boundary regexes; update those checks alongside the new methods.
- **Real-window (CDP) verification** — cold start shows the loading card; switching shows the card, preserves chrome + glass, then loads the new library's data; create/open auto-switch.
- **`npm test` green** as the completion gate.

## Execution Order

1. **Freeze interfaces** — `desktop.ts` bridge types + `libraryTypes.ts` + preload stubs. Unblocks parallel UI.
2. **Main-process backend** — `library-registry.ts` + restartable Core + IPC handlers. Non-UI; parallel with UI.
3. **UI-1…UI-4** — depend on (1); parallelizable via mock bridge.
4. **Integration + real-window verification + `npm test`.**

## Non-Goals (YAGNI)

- Background keep-alive / concurrent Core processes for inactive libraries.
- Real percentage progress for startup/switch (indeterminate + phase text only).
- Deleting library `.sqlite` files from disk via the UI (list-only removal).
- Cross-library operations (moving media between libraries).
- Writing roots into an inactive library without switching to it.

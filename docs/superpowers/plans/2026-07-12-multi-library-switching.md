# Multi-Library Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Megle switch between multiple libraries (one library = one `.sqlite` file with many root folders), show a centred loading card during startup and switching, and manage libraries/roots from a switcher popup + Settings.

**Architecture:** One active library = one Core process bound to one `.sqlite`. Switching = the Electron main process gracefully restarts Core pointed at the new file, **reusing the same bind port + session token** so the renderer never reloads. A `userData/libraries.json` registry (main process) records the library list + active id. Core is unchanged (a fresh `.sqlite` self-migrates on first open). UI is built against a frozen bridge interface so the switcher/loading-card/sidebar/settings work can proceed in parallel.

**Tech Stack:** Electron main (Node/TS) · React + TypeScript UI (`apps/web`) · Rust/Axum Core (unchanged) · SQLite · existing `@megle/core-client` for roots.

## Global Constraints

- One active library at a time; no background keep-alive of inactive libraries (spec Decision 1).
- A library is a single `.sqlite` file; roots live inside it (spec Decision 2).
- Registry file: `userData/libraries.json` (spec Decision 3).
- Sidebar top = current-library name + switch entry only; root add/remove in the switcher popup (active library) + Settings (spec Decision 4).
- Loading card = **indeterminate spinner + phase text**, NOT a percentage bar; window chrome + acrylic/glass background must persist during load/switch (spec Decision 5).
- Switch reconnect reuses the same `bindAddress` + `sessionToken`; **no `window.reload()`** (spec Decision 6).
- Create-library and open-library **auto-switch** on success (spec Decision 7).
- Inactive-library "add folder"/"remove folder" buttons are greyed out and non-clickable (spec Decision 8).
- Core (`crates/core`) gets **zero changes** unless Task 9's optional test is added. No new OpenAPI paths / tables / indexes.
- UI code must not use raw `fetch`, duplicate Core DTOs, or import Node fs — go through `@megle/core-client` / the desktop bridge. Guard with `npm run check:web` / `check:ui-design` / `check:desktop`.
- Every task ends by running the narrowest check for its area; broad/integration tasks run `npm test`.
- Never weaken a validation script to pass; update the check in the same task that changes the guarded pattern.

## File Structure

- `apps/desktop/src/library-registry.ts` **(new)** — read/write `userData/libraries.json`, CRUD libraries, active-id, path validation. Pure logic.
- `apps/desktop/src/main.ts` **(modify)** — restartable Core lifecycle (`restartCoreForLibrary`), library IPC handlers, phase push, cold-start from registry.
- `apps/desktop/src/preload.cjs` **(modify)** — expose new bridge methods + phase subscription.
- `apps/desktop/src/core-session.ts` **(modify)** — allow building a session with an explicit dbPath + reused bind/token.
- `apps/web/src/core/libraryTypes.ts` **(new)** — `LibrarySummary`, `LibraryLoadPhase`.
- `apps/web/src/core/desktop.ts` **(modify)** — extend `MegleDesktopBridge` + wrapper helpers.
- `apps/web/src/features/library/LibrarySwitcher.tsx` **(new)** — switcher popup.
- `apps/web/src/features/library/LibraryLoadingOverlay.tsx` **(new)** — centred loading/error card.
- `apps/web/src/features/library/LibrarySidebar.tsx` **(modify)** — top refactor.
- `apps/web/src/app/App.tsx` **(modify)** — app-level `libraryPhase` gating + overlay.
- `apps/web/src/core/useLibraryData.ts` **(modify)** — re-initialize on `library-ready`.
- `apps/web/src/features/settings/settingsPanels.tsx` **(modify)** — "资源库" section (Task 12, lower priority).
- `tools/checks/validate-desktop-core.mjs`, `validate-web-client.mjs` **(modify)** — extend boundary pins for new bridge methods.

---

## Task 1: Library registry module

**Files:**
- Create: `apps/desktop/src/library-registry.ts`
- Test: `apps/desktop/src/library-registry.test.ts`

**Interfaces:**
- Produces:
  - `interface LibraryEntry { id: string; name: string; dbPath: string; lastOpenedAt: number }`
  - `interface LibraryRegistry { activeLibraryId: string | null; libraries: LibraryEntry[] }`
  - `function loadRegistry(userDataDir: string): LibraryRegistry` (missing/corrupt file → `{ activeLibraryId: null, libraries: [] }`)
  - `function saveRegistry(userDataDir: string, registry: LibraryRegistry): void`
  - `function addLibrary(registry: LibraryRegistry, input: { name: string; dbPath: string; now: number }): { registry: LibraryRegistry; entry: LibraryEntry }` (generates `id = "lib_" + 8 hex`; if `dbPath` already present, returns the existing entry without duplicating)
  - `function removeLibrary(registry: LibraryRegistry, id: string): LibraryRegistry` (also clears `activeLibraryId` if it was the removed one)
  - `function setActive(registry: LibraryRegistry, id: string, now: number): LibraryRegistry` (sets `activeLibraryId` + bumps that entry's `lastOpenedAt`)
  - `function renameLibrary(registry: LibraryRegistry, id: string, name: string): LibraryRegistry`
  - `function registryFilePath(userDataDir: string): string` → `path.join(userDataDir, "libraries.json")`

- [ ] **Step 1: Write the failing test**

```ts
// apps/desktop/src/library-registry.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import os from "node:os";
import fs from "node:fs";
import path from "node:path";
import {
  loadRegistry, saveRegistry, addLibrary, removeLibrary, setActive, renameLibrary
} from "./library-registry.ts";

function tmpDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "megle-reg-"));
}

test("loadRegistry returns empty registry when file is missing", () => {
  const dir = tmpDir();
  assert.deepEqual(loadRegistry(dir), { activeLibraryId: null, libraries: [] });
});

test("loadRegistry recovers from corrupt JSON", () => {
  const dir = tmpDir();
  fs.writeFileSync(path.join(dir, "libraries.json"), "{ not json");
  assert.deepEqual(loadRegistry(dir), { activeLibraryId: null, libraries: [] });
});

test("addLibrary assigns an id and persists via save/load round-trip", () => {
  const dir = tmpDir();
  let reg = loadRegistry(dir);
  const { registry, entry } = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1000 });
  assert.match(entry.id, /^lib_[0-9a-f]{8}$/);
  assert.equal(entry.name, "A");
  assert.equal(registry.libraries.length, 1);
  saveRegistry(dir, registry);
  assert.deepEqual(loadRegistry(dir), registry);
});

test("addLibrary is idempotent by dbPath", () => {
  let reg = { activeLibraryId: null as string | null, libraries: [] };
  reg = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 }).registry;
  const second = addLibrary(reg, { name: "A again", dbPath: "C:/a.sqlite", now: 2 });
  assert.equal(second.registry.libraries.length, 1);
  assert.equal(second.entry.name, "A"); // existing entry returned, not duplicated
});

test("setActive sets id and bumps lastOpenedAt", () => {
  let reg = { activeLibraryId: null as string | null, libraries: [] };
  const added = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 });
  reg = setActive(added.registry, added.entry.id, 5000);
  assert.equal(reg.activeLibraryId, added.entry.id);
  assert.equal(reg.libraries[0].lastOpenedAt, 5000);
});

test("removeLibrary clears active id when removing the active library", () => {
  let reg = { activeLibraryId: null as string | null, libraries: [] };
  const added = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 });
  reg = setActive(added.registry, added.entry.id, 2);
  reg = removeLibrary(reg, added.entry.id);
  assert.equal(reg.activeLibraryId, null);
  assert.equal(reg.libraries.length, 0);
});

test("renameLibrary changes the name only", () => {
  let reg = { activeLibraryId: null as string | null, libraries: [] };
  const added = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 });
  reg = renameLibrary(added.registry, added.entry.id, "B");
  assert.equal(reg.libraries[0].name, "B");
  assert.equal(reg.libraries[0].dbPath, "C:/a.sqlite");
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --test --experimental-strip-types apps/desktop/src/library-registry.test.ts`
Expected: FAIL — cannot find module `./library-registry.ts`.

- [ ] **Step 3: Write minimal implementation**

```ts
// apps/desktop/src/library-registry.ts
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

export interface LibraryEntry {
  id: string;
  name: string;
  dbPath: string;
  lastOpenedAt: number;
}

export interface LibraryRegistry {
  activeLibraryId: string | null;
  libraries: LibraryEntry[];
}

const EMPTY: LibraryRegistry = { activeLibraryId: null, libraries: [] };

export function registryFilePath(userDataDir: string): string {
  return path.join(userDataDir, "libraries.json");
}

export function loadRegistry(userDataDir: string): LibraryRegistry {
  try {
    const raw = fs.readFileSync(registryFilePath(userDataDir), "utf8");
    const parsed = JSON.parse(raw) as Partial<LibraryRegistry>;
    const libraries = Array.isArray(parsed.libraries)
      ? parsed.libraries.filter(
          (l): l is LibraryEntry =>
            !!l && typeof l.id === "string" && typeof l.name === "string" && typeof l.dbPath === "string"
        )
      : [];
    const activeLibraryId =
      typeof parsed.activeLibraryId === "string" &&
      libraries.some((l) => l.id === parsed.activeLibraryId)
        ? parsed.activeLibraryId
        : null;
    return { activeLibraryId, libraries };
  } catch {
    return { ...EMPTY };
  }
}

export function saveRegistry(userDataDir: string, registry: LibraryRegistry): void {
  fs.writeFileSync(registryFilePath(userDataDir), JSON.stringify(registry), "utf8");
}

export function addLibrary(
  registry: LibraryRegistry,
  input: { name: string; dbPath: string; now: number }
): { registry: LibraryRegistry; entry: LibraryEntry } {
  const existing = registry.libraries.find((l) => l.dbPath === input.dbPath);
  if (existing) {
    return { registry, entry: existing };
  }
  const entry: LibraryEntry = {
    id: `lib_${crypto.randomBytes(4).toString("hex")}`,
    name: input.name,
    dbPath: input.dbPath,
    lastOpenedAt: input.now
  };
  return {
    registry: { ...registry, libraries: [...registry.libraries, entry] },
    entry
  };
}

export function removeLibrary(registry: LibraryRegistry, id: string): LibraryRegistry {
  return {
    activeLibraryId: registry.activeLibraryId === id ? null : registry.activeLibraryId,
    libraries: registry.libraries.filter((l) => l.id !== id)
  };
}

export function setActive(registry: LibraryRegistry, id: string, now: number): LibraryRegistry {
  return {
    activeLibraryId: id,
    libraries: registry.libraries.map((l) => (l.id === id ? { ...l, lastOpenedAt: now } : l))
  };
}

export function renameLibrary(registry: LibraryRegistry, id: string, name: string): LibraryRegistry {
  return {
    ...registry,
    libraries: registry.libraries.map((l) => (l.id === id ? { ...l, name } : l))
  };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --test --experimental-strip-types apps/desktop/src/library-registry.test.ts`
Expected: PASS — 7 tests.

- [ ] **Step 5: Typecheck + commit**

Run: `npm --workspace @megle/desktop run typecheck`
Expected: no output (clean).

```bash
git add apps/desktop/src/library-registry.ts apps/desktop/src/library-registry.test.ts
git commit -m "feat(desktop): library registry (userData/libraries.json CRUD)"
```

---

## Task 2: Frozen renderer interfaces (types + bridge shape)

Unblocks all UI tasks. No behavior yet — types + wrapper helpers + preload stubs so UI can build against a real (initially inert) surface.

**Files:**
- Create: `apps/web/src/core/libraryTypes.ts`
- Modify: `apps/web/src/core/desktop.ts` (add to `MegleDesktopBridge` + helper wrappers)
- Modify: `apps/desktop/src/preload.cjs` (expose new methods)

**Interfaces:**
- Produces (consumed by Tasks 5–12):
  - `libraryTypes.ts`: `LibrarySummary`, `LibraryLoadPhase` (verbatim from spec).
  - `desktop.ts` helpers: `getLibraryBridge()`, `switchLibrary`, `listLibraries`, `createLibrary`, `openLibrary`, `removeLibraryFromList`, `renameLibrary`, `getActiveLibraryId`, `pickLibraryFile`, `pickNewLibraryPath`, `subscribeLibraryPhase`.

- [ ] **Step 1: Create the types file**

```ts
// apps/web/src/core/libraryTypes.ts
export interface LibrarySummary {
  id: string;
  name: string;
  dbPath: string;
  available: boolean;
  active: boolean;
}

export type LibraryLoadPhase =
  | "starting-core"
  | "opening-library"
  | "loading-media"
  | "ready"
  | { kind: "error"; message: string };

export function isErrorPhase(
  phase: LibraryLoadPhase
): phase is { kind: "error"; message: string } {
  return typeof phase === "object" && phase.kind === "error";
}
```

- [ ] **Step 2: Extend the bridge interface + wrappers in `desktop.ts`**

Add to the `MegleDesktopBridge` interface (after `shell?: DesktopShellActions;`):

```ts
  listLibraries?: () => Promise<LibrarySummary[]>;
  getActiveLibraryId?: () => Promise<string | null>;
  switchLibrary?: (libraryId: string) => Promise<{ ok: boolean; error?: string }>;
  createLibrary?: (input: { name: string; dbPath: string }) => Promise<{ ok: boolean; library?: LibrarySummary; error?: string }>;
  openLibrary?: (dbPath: string) => Promise<{ ok: boolean; library?: LibrarySummary; error?: string }>;
  removeLibraryFromList?: (libraryId: string) => Promise<{ ok: boolean }>;
  renameLibrary?: (libraryId: string, name: string) => Promise<{ ok: boolean }>;
  pickLibraryFile?: () => Promise<string | null>;
  pickNewLibraryPath?: (defaultName?: string) => Promise<string | null>;
  onLibraryPhase?: (cb: (phase: LibraryLoadPhase) => void) => () => void;
```

Add `import type { LibrarySummary, LibraryLoadPhase } from "./libraryTypes";` at the top, and these wrappers at the bottom of `desktop.ts`:

```ts
export async function listLibraries(): Promise<LibrarySummary[]> {
  return (await getDesktopBridge()?.listLibraries?.()) ?? [];
}
export async function getActiveLibraryId(): Promise<string | null> {
  return (await getDesktopBridge()?.getActiveLibraryId?.()) ?? null;
}
export async function switchLibrary(libraryId: string): Promise<{ ok: boolean; error?: string }> {
  return (await getDesktopBridge()?.switchLibrary?.(libraryId)) ?? { ok: false, error: "no desktop bridge" };
}
export async function createLibrary(
  input: { name: string; dbPath: string }
): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }> {
  return (await getDesktopBridge()?.createLibrary?.(input)) ?? { ok: false, error: "no desktop bridge" };
}
export async function openLibrary(
  dbPath: string
): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }> {
  return (await getDesktopBridge()?.openLibrary?.(dbPath)) ?? { ok: false, error: "no desktop bridge" };
}
export async function removeLibraryFromList(libraryId: string): Promise<{ ok: boolean }> {
  return (await getDesktopBridge()?.removeLibraryFromList?.(libraryId)) ?? { ok: false };
}
export async function renameLibraryEntry(libraryId: string, name: string): Promise<{ ok: boolean }> {
  return (await getDesktopBridge()?.renameLibrary?.(libraryId, name)) ?? { ok: false };
}
export async function pickLibraryFile(): Promise<string | null> {
  return (await getDesktopBridge()?.pickLibraryFile?.()) ?? null;
}
export async function pickNewLibraryPath(defaultName?: string): Promise<string | null> {
  return (await getDesktopBridge()?.pickNewLibraryPath?.(defaultName)) ?? null;
}
export function subscribeLibraryPhase(cb: (phase: LibraryLoadPhase) => void): () => void {
  return getDesktopBridge()?.onLibraryPhase?.(cb) ?? (() => undefined);
}
```

- [ ] **Step 3: Add preload stubs in `preload.cjs`**

Inside the `exposeInMainWorld("megleDesktop", { ... })` object (after the `shell: {...}` block), add:

```js
  listLibraries: () => ipcRenderer.invoke("megle:list-libraries"),
  getActiveLibraryId: () => ipcRenderer.invoke("megle:get-active-library-id"),
  switchLibrary: (libraryId) => ipcRenderer.invoke("megle:switch-library", libraryId),
  createLibrary: (input) => ipcRenderer.invoke("megle:create-library", input),
  openLibrary: (dbPath) => ipcRenderer.invoke("megle:open-library", dbPath),
  removeLibraryFromList: (libraryId) => ipcRenderer.invoke("megle:remove-library-from-list", libraryId),
  renameLibrary: (libraryId, name) => ipcRenderer.invoke("megle:rename-library", libraryId, name),
  pickLibraryFile: () => ipcRenderer.invoke("megle:pick-library-file"),
  pickNewLibraryPath: (defaultName) => ipcRenderer.invoke("megle:pick-new-library-path", defaultName),
  onLibraryPhase: (cb) => {
    const listener = (_event, phase) => cb(phase);
    ipcRenderer.on("megle:library-load-phase", listener);
    return () => ipcRenderer.removeListener("megle:library-load-phase", listener);
  },
```

- [ ] **Step 4: Typecheck web + web check**

Run: `npm --workspace @megle/web run typecheck`
Expected: clean.
Run: `node tools/checks/validate-web-client.mjs`
Expected: `PASS: web client boundaries` (if it fails on a new-symbol pin, that is Task 4's job — note it and continue only if the failure is unrelated).

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/core/libraryTypes.ts apps/web/src/core/desktop.ts apps/desktop/src/preload.cjs
git commit -m "feat(web): freeze library bridge interface + types + preload stubs"
```

---

## Task 3: Restartable Core session (core-session.ts)

**Files:**
- Modify: `apps/desktop/src/core-session.ts`
- Test: `apps/desktop/src/core-session.test.ts`

**Interfaces:**
- Consumes: `CoreSession` (existing: `{ baseUrl, bindAddress, dbPath, sessionToken, allowedOrigin }`).
- Produces: `function sessionForDbPath(base: CoreSession, dbPath: string): CoreSession` — clones `base` but swaps `dbPath`, keeping the same `bindAddress`/`sessionToken`/`baseUrl`/`allowedOrigin`. This is how a switch reuses port + token.

- [ ] **Step 1: Write the failing test**

```ts
// apps/desktop/src/core-session.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { sessionForDbPath } from "./core-session.ts";

test("sessionForDbPath swaps only dbPath, reusing bind + token", () => {
  const base = {
    baseUrl: "http://127.0.0.1:47000/api",
    bindAddress: "127.0.0.1:47000",
    dbPath: "C:/old.sqlite",
    sessionToken: "tok123",
    allowedOrigin: "http://127.0.0.1:5173"
  };
  const next = sessionForDbPath(base, "C:/new.sqlite");
  assert.equal(next.dbPath, "C:/new.sqlite");
  assert.equal(next.bindAddress, base.bindAddress);
  assert.equal(next.sessionToken, base.sessionToken);
  assert.equal(next.baseUrl, base.baseUrl);
  assert.equal(next.allowedOrigin, base.allowedOrigin);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --test --experimental-strip-types apps/desktop/src/core-session.test.ts`
Expected: FAIL — `sessionForDbPath` is not exported.

- [ ] **Step 3: Add the function to `core-session.ts`**

Append (after `createCoreSession`):

```ts
// Build a session for a different database file that reuses the same bind
// address + session token, so a Core restart does not require the renderer to
// re-read its coreUrl/token (spec Decision 6).
export function sessionForDbPath(base: CoreSession, dbPath: string): CoreSession {
  return { ...base, dbPath };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --test --experimental-strip-types apps/desktop/src/core-session.test.ts`
Expected: PASS.

- [ ] **Step 5: Typecheck + commit**

Run: `npm --workspace @megle/desktop run typecheck`
Expected: clean.

```bash
git add apps/desktop/src/core-session.ts apps/desktop/src/core-session.test.ts
git commit -m "feat(desktop): sessionForDbPath to restart Core reusing port+token"
```

---

## Task 4: Main-process library IPC + restartable Core + phase push

The largest task. Wires the registry, restart routine, IPC handlers, phase events, and cold-start-from-registry into `main.ts`. Also updates the desktop boundary check.

**Files:**
- Modify: `apps/desktop/src/main.ts`
- Modify: `tools/checks/validate-desktop-core.mjs` (extend pinned IPC/bridge lists)

**Interfaces:**
- Consumes: `library-registry.ts` (Task 1), `sessionForDbPath` (Task 3), existing `startCoreProcess`/`waitForCoreHealth`/`createCoreSession`/`stopCoreProcessTree`, existing module state (`coreProcess`, `coreSession`, `coreReadyPromise`).
- Produces (consumed by preload IPC channels from Task 2): the handlers for `megle:list-libraries`, `megle:get-active-library-id`, `megle:switch-library`, `megle:create-library`, `megle:open-library`, `megle:remove-library-from-list`, `megle:rename-library`, `megle:pick-library-file`, `megle:pick-new-library-path`, and the `megle:library-load-phase` push. Phase strings match `LibraryLoadPhase`.

- [ ] **Step 1: Add imports + registry helpers near the top of `main.ts`**

After the existing imports add:

```ts
import * as libraryRegistry from "./library-registry.js";
import { sessionForDbPath } from "./core-session.js";
```

Add module state + helpers (near the other module-level `let` declarations):

```ts
let cachedRegistry: libraryRegistry.LibraryRegistry | null = null;

function userDataDir(): string {
  return app.getPath("userData");
}
function getRegistry(): libraryRegistry.LibraryRegistry {
  if (!cachedRegistry) cachedRegistry = libraryRegistry.loadRegistry(userDataDir());
  return cachedRegistry;
}
function persistRegistry(next: libraryRegistry.LibraryRegistry): void {
  cachedRegistry = next;
  libraryRegistry.saveRegistry(userDataDir(), next);
}
function pushLibraryPhase(phase: unknown): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send("megle:library-load-phase", phase);
  }
}
function toSummaries(reg: libraryRegistry.LibraryRegistry): Array<{
  id: string; name: string; dbPath: string; available: boolean; active: boolean;
}> {
  return reg.libraries.map((l) => ({
    id: l.id,
    name: l.name,
    dbPath: l.dbPath,
    available: fsSync.existsSync(l.dbPath),
    active: l.id === reg.activeLibraryId
  }));
}
```

Add `import fsSync from "node:fs";` at the top (the file currently imports `promises as fsp`; add the sync default import for `existsSync`).

- [ ] **Step 2: Add the Core restart routine**

Add this function (near `startCoreSession`):

```ts
// Gracefully restart Core pointed at a new database file, reusing the same
// bind address + session token so the renderer keeps its injected coreUrl/token
// (spec Decision 6). Pushes load phases to the renderer as it goes.
async function restartCoreForLibrary(dbPath: string): Promise<void> {
  pushLibraryPhase("starting-core");
  const base = coreSession ?? (await createCoreSession());
  // Stop the current Core and wait for the port to free up.
  coreProcess?.stop();
  coreProcess = null;
  await waitForPortFree(base.bindAddress);

  const session = sessionForDbPath(base, dbPath);
  const handle = startCoreProcess(session);
  pushLibraryPhase("opening-library");
  await waitForCoreHealth(session.baseUrl, session.sessionToken);
  coreProcess = handle;
  coreSession = session;
}

// Poll until the host:port can be bound again (Core's listener has closed),
// guarding the known probe-and-close race. Bounded; throws on timeout.
async function waitForPortFree(bindAddress: string): Promise<void> {
  const [host, portStr] = bindAddress.split(":");
  const port = Number(portStr);
  const deadline = Date.now() + 5000;
  // eslint-disable-next-line no-constant-condition
  while (true) {
    const free = await new Promise<boolean>((resolve) => {
      const probe = net.createServer();
      probe.once("error", () => resolve(false));
      probe.listen(port, host, () => probe.close(() => resolve(true)));
    });
    if (free) return;
    if (Date.now() > deadline) throw new Error(`Port ${bindAddress} did not free up`);
    await new Promise((r) => setTimeout(r, 80));
  }
}
```

Add `import net from "node:net";` at the top if not already present.

- [ ] **Step 3: Make cold-start read the registry**

Find `createCoreSession()` usage in `startCoreSession` (line ~440). Change the cold-start to derive the DB path from the active library. Replace the `const session = await createCoreSession();` line inside the loop with:

```ts
      const session = await createColdStartSession();
```

And add:

```ts
// On cold start, point Core at the active library from the registry (if any),
// otherwise fall back to createCoreSession()'s default dbPath (first-run / dev).
async function createColdStartSession(): Promise<CoreSession> {
  const base = await createCoreSession();
  const reg = getRegistry();
  const active = reg.libraries.find((l) => l.id === reg.activeLibraryId);
  if (active && fsSync.existsSync(active.dbPath)) {
    return sessionForDbPath(base, active.dbPath);
  }
  // No active library yet: register the default dbPath as the first library so
  // the switcher has something to show, and make it active.
  const added = libraryRegistry.addLibrary(reg, {
    name: "Default",
    dbPath: base.dbPath,
    now: Date.now()
  });
  persistRegistry(libraryRegistry.setActive(added.registry, added.entry.id, Date.now()));
  return base;
}
```

- [ ] **Step 4: Add the IPC handlers**

Add near the other `ipcMain.handle(...)` blocks:

```ts
ipcMain.handle("megle:list-libraries", () => toSummaries(getRegistry()));
ipcMain.handle("megle:get-active-library-id", () => getRegistry().activeLibraryId);

ipcMain.handle("megle:switch-library", async (_e, libraryId: string) => {
  const reg = getRegistry();
  const target = reg.libraries.find((l) => l.id === libraryId);
  if (!target) return { ok: false, error: "library not found" };
  if (!fsSync.existsSync(target.dbPath)) return { ok: false, error: "library file missing" };
  try {
    await restartCoreForLibrary(target.dbPath);
    persistRegistry(libraryRegistry.setActive(reg, libraryId, Date.now()));
    pushLibraryPhase("loading-media"); // renderer takes over from here
    return { ok: true };
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:create-library", async (_e, input: { name: string; dbPath: string }) => {
  const reg = getRegistry();
  try {
    // A brand-new empty file: Core builds the schema on first open.
    if (!fsSync.existsSync(input.dbPath)) {
      fsSync.writeFileSync(input.dbPath, ""); // touch; Core opens + migrates
    }
    const added = libraryRegistry.addLibrary(reg, { name: input.name, dbPath: input.dbPath, now: Date.now() });
    await restartCoreForLibrary(added.entry.dbPath);
    persistRegistry(libraryRegistry.setActive(added.registry, added.entry.id, Date.now()));
    pushLibraryPhase("loading-media");
    const summary = toSummaries(getRegistry()).find((s) => s.id === added.entry.id);
    return { ok: true, library: summary };
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:open-library", async (_e, dbPath: string) => {
  const reg = getRegistry();
  if (!fsSync.existsSync(dbPath)) return { ok: false, error: "file not found" };
  try {
    const name = path.basename(dbPath).replace(/\.sqlite$/i, "");
    const added = libraryRegistry.addLibrary(reg, { name, dbPath, now: Date.now() });
    await restartCoreForLibrary(added.entry.dbPath);
    persistRegistry(libraryRegistry.setActive(added.registry, added.entry.id, Date.now()));
    pushLibraryPhase("loading-media");
    const summary = toSummaries(getRegistry()).find((s) => s.id === added.entry.id);
    return { ok: true, library: summary };
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:remove-library-from-list", (_e, libraryId: string) => {
  persistRegistry(libraryRegistry.removeLibrary(getRegistry(), libraryId));
  return { ok: true };
});

ipcMain.handle("megle:rename-library", (_e, libraryId: string, name: string) => {
  persistRegistry(libraryRegistry.renameLibrary(getRegistry(), libraryId, name));
  return { ok: true };
});

ipcMain.handle("megle:pick-library-file", async () => {
  const result = mainWindow
    ? await dialog.showOpenDialog(mainWindow, { properties: ["openFile"], filters: [{ name: "Megle Library", extensions: ["sqlite"] }] })
    : await dialog.showOpenDialog({ properties: ["openFile"], filters: [{ name: "Megle Library", extensions: ["sqlite"] }] });
  return result.canceled ? null : (result.filePaths[0] ?? null);
});

ipcMain.handle("megle:pick-new-library-path", async (_e, defaultName?: string) => {
  const result = mainWindow
    ? await dialog.showSaveDialog(mainWindow, { defaultPath: `${defaultName ?? "library"}.sqlite`, filters: [{ name: "Megle Library", extensions: ["sqlite"] }] })
    : await dialog.showSaveDialog({ defaultPath: `${defaultName ?? "library"}.sqlite`, filters: [{ name: "Megle Library", extensions: ["sqlite"] }] });
  return result.canceled ? null : (result.filePath ?? null);
});
```

- [ ] **Step 5: Update the desktop boundary check**

Open `tools/checks/validate-desktop-core.mjs`. Find the list of pinned IPC channel names or bridge methods (grep for `megle:pick-folder` or `ipcMain.handle`). Add the new channel names to whatever allowlist/required-list the check enforces so the new handlers pass. If the check has no such list (only structural), skip — but run it to confirm.

Run: `node tools/checks/validate-desktop-core.mjs`
Expected: `PASS: desktop core orchestration boundaries`. If it fails naming a new channel, add that channel to the pinned list in the check and re-run.

- [ ] **Step 6: Typecheck + commit**

Run: `npm --workspace @megle/desktop run typecheck`
Expected: clean.

```bash
git add apps/desktop/src/main.ts tools/checks/validate-desktop-core.mjs
git commit -m "feat(desktop): library IPC + restartable Core + load-phase push + cold-start from registry"
```

---

## Task 5: useLibraryData re-initialize on library-ready

**Files:**
- Modify: `apps/web/src/core/useLibraryData.ts`

**Interfaces:**
- Consumes: `subscribeLibraryPhase` (Task 2), existing `loadLibrary` (internal to the hook).
- Produces: on `"loading-media"` phase, the hook re-runs `loadLibrary` from scratch (fresh roots/media) — so a switch reloads the new library without a page reload.

- [ ] **Step 1: Add the subscription effect**

In `useLibraryData.ts`, import at top: `import { subscribeLibraryPhase } from "./desktop";`. Find the mount `useEffect` that calls `loadLibrary` once (around line 1813). Add a second effect after it:

```ts
useEffect(() => {
  const unsubscribe = subscribeLibraryPhase((phase) => {
    if (phase === "loading-media") {
      // A library switch completed on the main process; reload from the new DB.
      void loadLibrary({ rootId: null, folderId: null });
    }
  });
  return unsubscribe;
  // loadLibrary is stable (useCallback); safe to depend on it.
}, [loadLibrary]);
```

(If `loadLibrary` is not a `useCallback`, wrap it or depend on `[]` with an eslint-disable — match the file's existing pattern for the mount effect.)

- [ ] **Step 2: Typecheck**

Run: `npm --workspace @megle/web run typecheck`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add apps/web/src/core/useLibraryData.ts
git commit -m "feat(web): reload library data when a switch reaches loading-media"
```

---

## Task 6 (PARALLEL-UI): LibraryLoadingOverlay component

Depends only on Task 2 interfaces. Build with a mock bridge if backend isn't ready.

**Files:**
- Create: `apps/web/src/features/library/LibraryLoadingOverlay.tsx`
- Modify: `apps/web/src/styles.css` (overlay styles)

**Interfaces:**
- Consumes: `LibraryLoadPhase`, `isErrorPhase` (Task 2).
- Produces: `<LibraryLoadingOverlay phase={LibraryLoadPhase} onRetry={() => void} onSwitchLibrary={() => void} />` — renders a centred glass card. Non-error → spinner + phase text; error → message + "重试"/"切换其它库" buttons. Renders `null` when `phase === "ready"`.

- [ ] **Step 1: Create the component**

```tsx
// apps/web/src/features/library/LibraryLoadingOverlay.tsx
import { Loader2 } from "lucide-react";
import { LiquidGlassSurface } from "../../design/liquid-glass";
import { isErrorPhase, type LibraryLoadPhase } from "../../core/libraryTypes";

const PHASE_TEXT: Record<string, string> = {
  "starting-core": "正在启动核心…",
  "opening-library": "正在打开库…",
  "loading-media": "正在加载媒体…"
};

export function LibraryLoadingOverlay({
  phase,
  onRetry,
  onSwitchLibrary
}: {
  phase: LibraryLoadPhase;
  onRetry: () => void;
  onSwitchLibrary: () => void;
}) {
  if (phase === "ready") return null;
  const error = isErrorPhase(phase);
  return (
    <div className="library-loading-overlay" aria-live="polite" aria-busy={!error}>
      <LiquidGlassSurface as="div" className="library-loading-card" tone="elevated">
        {error ? (
          <>
            <div className="library-loading-error-text">{phase.message}</div>
            <div className="library-loading-actions">
              <button type="button" onClick={onRetry}>重试</button>
              <button type="button" onClick={onSwitchLibrary}>切换其它库</button>
            </div>
          </>
        ) : (
          <>
            <Loader2 className="library-loading-spinner" aria-hidden="true" size={22} />
            <div className="library-loading-text">{PHASE_TEXT[phase] ?? "加载中…"}</div>
          </>
        )}
      </LiquidGlassSurface>
    </div>
  );
}
```

- [ ] **Step 2: Add styles to `styles.css`**

```css
.library-loading-overlay {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  z-index: 5;
  pointer-events: none;
}
.library-loading-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 28px 40px;
  pointer-events: auto;
}
.library-loading-spinner { animation: library-loading-spin 900ms linear infinite; }
@keyframes library-loading-spin { to { transform: rotate(360deg); } }
.library-loading-text,
.library-loading-error-text { font-size: 13px; color: var(--text-soft); }
.library-loading-actions { display: flex; gap: 10px; margin-top: 4px; }
```

- [ ] **Step 3: Typecheck + web check**

Run: `npm --workspace @megle/web run typecheck` → clean.
Run: `node tools/checks/validate-web-client.mjs` → PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/web/src/features/library/LibraryLoadingOverlay.tsx apps/web/src/styles.css
git commit -m "feat(web): LibraryLoadingOverlay (spinner/phase card + error card)"
```

---

## Task 7 (PARALLEL-UI): App-level phase gating + overlay mount

**Files:**
- Modify: `apps/web/src/app/App.tsx`

**Interfaces:**
- Consumes: `subscribeLibraryPhase`, `switchLibrary` (Task 2); `<LibraryLoadingOverlay />` (Task 6); existing `library.loading` from `useLibraryData`.
- Produces: an app-level `libraryPhase` state; the center content area is overlaid by `LibraryLoadingOverlay` until phase is `"ready"` AND `!library.loading`.

- [ ] **Step 1: Add phase state + subscription in `App.tsx`**

Add near the other `useState`s:

```tsx
const [libraryPhase, setLibraryPhase] = useState<LibraryLoadPhase>("starting-core");
```

Import `{ subscribeLibraryPhase } from "../core/desktop"`, `{ type LibraryLoadPhase } from "../core/libraryTypes"`. Add an effect:

```tsx
useEffect(() => subscribeLibraryPhase(setLibraryPhase), []);
```

Move to `"ready"` once the renderer has data: add

```tsx
useEffect(() => {
  if (!library.loading && library.roots.length >= 0) {
    // data settled (roots resolved, even if empty) → overlay can clear.
    setLibraryPhase((p) => (p === "loading-media" ? "ready" : p));
  }
}, [library.loading, library.roots.length]);
```

- [ ] **Step 2: Mount the overlay over the center pane**

Add the switcher-open state here (Task 11 reuses it):

```tsx
const [switcherOpen, setSwitcherOpen] = useState(false);
```

In `renderCenterPane()`, wrap the returned content so the overlay sits on top. Simplest: in the top-level JSX where `centerPane={renderCenterPane()}` is passed to `AppShell`, change to a fragment:

```tsx
centerPane={
  <>
    {renderCenterPane()}
    <LibraryLoadingOverlay
      phase={library.loading && libraryPhase === "ready" ? "loading-media" : libraryPhase}
      onRetry={() => window.location.reload()}
      onSwitchLibrary={() => setSwitcherOpen(true)}
    />
  </>
}
```

(Import `LibraryLoadingOverlay`. `switcherOpen`/`setSwitcherOpen` are consumed by Task 11 to render `<LibrarySwitcher open={switcherOpen} .../>`.)

- [ ] **Step 3: Typecheck + web + ui-design checks**

Run: `npm --workspace @megle/web run typecheck` → clean.
Run: `node tools/checks/validate-web-client.mjs` → PASS.
Run: `node tools/checks/validate-ui-design.mjs` → PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/web/src/app/App.tsx
git commit -m "feat(web): app-level library-phase gating with loading overlay"
```

---

## Task 8 (PARALLEL-UI): LibrarySwitcher popup

**Files:**
- Create: `apps/web/src/features/library/LibrarySwitcher.tsx`
- Modify: `apps/web/src/styles.css`

**Interfaces:**
- Consumes: `listLibraries`, `switchLibrary`, `createLibrary`, `openLibrary`, `removeLibraryFromList`, `renameLibraryEntry`, `pickLibraryFile`, `pickNewLibraryPath` (Task 2); `LibrarySummary` (Task 2); existing `library.addRoot` / `library.removeRoot?` and `pickNativeFolder` for the active-library folder buttons.
- Produces: `<LibrarySwitcher open={boolean} onClose={() => void} library={LibraryState} />` — the popup. Clicking a non-active row calls `switchLibrary`; footer buttons create/open; per-row `...` menu: add folder / remove folder (active only, else disabled) / rename / remove from list.

- [ ] **Step 1: Create the component**

```tsx
// apps/web/src/features/library/LibrarySwitcher.tsx
import { useEffect, useState } from "react";
import { Check, FolderPlus, FolderMinus, Search, X } from "lucide-react";
import { LiquidGlassSurface } from "../../design/liquid-glass";
import {
  listLibraries, switchLibrary, createLibrary, openLibrary,
  removeLibraryFromList, pickLibraryFile, pickNewLibraryPath
} from "../../core/desktop";
import { pickNativeFolder } from "../../core/desktop";
import type { LibrarySummary } from "../../core/libraryTypes";
import type { LibraryState } from "../../core/useLibraryData";

export function LibrarySwitcher({
  open, onClose, library
}: { open: boolean; onClose: () => void; library: LibraryState }) {
  const [libs, setLibs] = useState<LibrarySummary[]>([]);
  const [query, setQuery] = useState("");

  const reload = () => void listLibraries().then(setLibs);
  useEffect(() => { if (open) reload(); }, [open]);
  if (!open) return null;

  const filtered = libs.filter((l) =>
    l.name.toLowerCase().includes(query.toLowerCase()) ||
    l.dbPath.toLowerCase().includes(query.toLowerCase())
  );

  async function onSwitch(l: LibrarySummary) {
    if (l.active || !l.available) return;
    onClose();
    await switchLibrary(l.id);
  }
  async function onCreate() {
    const p = await pickNewLibraryPath("library");
    if (!p) return;
    const name = p.split(/[\\/]/).pop()?.replace(/\.sqlite$/i, "") ?? "library";
    onClose();
    await createLibrary({ name, dbPath: p });
  }
  async function onOpen() {
    const p = await pickLibraryFile();
    if (!p) return;
    onClose();
    await openLibrary(p);
  }
  async function onAddFolder() {
    const folder = await pickNativeFolder();
    if (folder) await library.addRoot(folder);
    reload();
  }

  return (
    <div className="library-switcher-backdrop" onClick={onClose}>
      <LiquidGlassSurface
        as="div"
        className="library-switcher"
        tone="elevated"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="library-switcher-search">
          <Search size={14} aria-hidden="true" />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索…" />
          <button type="button" aria-label="Close" onClick={onClose}><X size={14} /></button>
        </div>
        <ul className="library-switcher-list">
          {filtered.map((l) => (
            <li key={l.id} className={`library-switcher-row${l.active ? " active" : ""}${l.available ? "" : " unavailable"}`}>
              <button type="button" className="library-switcher-main" onClick={() => onSwitch(l)}>
                <span className="library-switcher-name">{l.name}</span>
                <span className="library-switcher-path">{l.dbPath}</span>
              </button>
              {l.active ? <Check size={14} className="library-switcher-check" aria-hidden="true" /> : null}
              <div className="library-switcher-actions">
                <button type="button" aria-label="Add folder" disabled={!l.active} onClick={onAddFolder}>
                  <FolderPlus size={14} />
                </button>
                <button type="button" aria-label="Remove folder" disabled={!l.active} onClick={() => { /* opens a root-picker submenu; wired in integration */ }}>
                  <FolderMinus size={14} />
                </button>
              </div>
            </li>
          ))}
        </ul>
        <div className="library-switcher-footer">
          <button type="button" onClick={onCreate}>创建资源库…</button>
          <button type="button" onClick={onOpen}>打开其它资源库…</button>
        </div>
      </LiquidGlassSurface>
    </div>
  );
}
```

Note: "remove folder" opens a submenu to pick which root to remove (there can be many). For this task leave the click a no-op placeholder that the integration task (Task 11) wires to a root-list submenu calling `library.removeRoot(rootId)`; the button's disabled state (active-only) is the deliverable here.

- [ ] **Step 2: Add styles to `styles.css`**

```css
.library-switcher-backdrop {
  position: fixed; inset: 0; z-index: 40;
  display: grid; place-items: start center; padding-top: 64px;
}
.library-switcher { width: 460px; max-height: 70vh; display: flex; flex-direction: column; overflow: hidden; }
.library-switcher-search { display: flex; align-items: center; gap: 8px; padding: 10px 12px; }
.library-switcher-search input { flex: 1; background: transparent; border: 0; outline: 0; color: inherit; }
.library-switcher-list { list-style: none; margin: 0; padding: 4px; overflow-y: auto; }
.library-switcher-row { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: var(--radius-control); }
.library-switcher-row.unavailable { opacity: 0.5; }
.library-switcher-main { flex: 1; display: flex; flex-direction: column; align-items: flex-start; background: transparent; border: 0; cursor: pointer; color: inherit; }
.library-switcher-name { font-size: 13px; }
.library-switcher-path { font-size: 11px; color: var(--text-muted); }
.library-switcher-actions { display: flex; gap: 4px; }
.library-switcher-actions button:disabled { opacity: 0.35; cursor: default; }
.library-switcher-footer { display: flex; flex-direction: column; padding: 8px; gap: 4px; border-top: 1px solid var(--line); }
.library-switcher-footer button { text-align: left; background: transparent; border: 0; color: inherit; padding: 8px; border-radius: var(--radius-control); cursor: pointer; }
```

- [ ] **Step 3: Typecheck + checks**

Run: `npm --workspace @megle/web run typecheck` → clean.
Run: `node tools/checks/validate-web-client.mjs` → PASS.
Run: `node tools/checks/validate-ui-design.mjs` → PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/web/src/features/library/LibrarySwitcher.tsx apps/web/src/styles.css
git commit -m "feat(web): LibrarySwitcher popup (list, switch, create/open, folder buttons)"
```

---

## Task 9 (PARALLEL-UI): LibrarySidebar top refactor

**Files:**
- Modify: `apps/web/src/features/library/LibrarySidebar.tsx`
- Modify: `apps/web/src/styles.css`

**Interfaces:**
- Consumes: existing `library` prop; a new `onOpenSwitcher: () => void` prop (wired in Task 11); `library.activeLibraryName` — if not present, show `library.roots.length` roots subtitle as today plus a switch button. For now use a passed `activeLibraryName?: string` prop defaulting to `"Library"`.
- Produces: sidebar top = library-name label + switch button; the `root-form` (path input, Add, choose-folder, rescan) is removed. Folder tree + whole-library refresh button stay.

- [ ] **Step 1: Remove the `root-form` block and add the switch header**

In `LibrarySidebar.tsx`, delete the entire `<form className="root-form">…</form>` (lines ~89-124) and the now-unused local `rootPath` state / `submitRoot` / `chooseFolder` / `rescanSelectedRoot` handlers if they become dead. Replace the heading area (lines ~73-87) with:

```tsx
<header className="library-sidebar-heading">
  <button
    type="button"
    className="library-switch-button"
    data-pointer-glow="off"
    onClick={onOpenSwitcher}
  >
    <span className="library-switch-name">{activeLibraryName ?? "Library"}</span>
    <ChevronsUpDown size={14} aria-hidden="true" />
  </button>
  <button
    type="button"
    className="library-refresh-button"
    aria-label="Refresh library"
    onClick={() => void library.refresh()}
  >
    <RefreshCw size={14} aria-hidden="true" />
  </button>
</header>
```

Add `ChevronsUpDown` to the `lucide-react` import; add `onOpenSwitcher: () => void;` and `activeLibraryName?: string;` to the component's props interface. Keep `RefreshCw` import.

- [ ] **Step 2: Add styles**

```css
.library-sidebar-heading { display: flex; align-items: center; gap: 8px; padding: 8px 4px; }
.library-switch-button { flex: 1; display: flex; align-items: center; justify-content: space-between; gap: 8px; background: transparent; border: 0; color: inherit; cursor: pointer; padding: 6px 8px; border-radius: var(--radius-control); }
.library-switch-name { font-size: 14px; font-weight: 650; }
```

- [ ] **Step 3: Typecheck + checks**

Run: `npm --workspace @megle/web run typecheck` → clean.
Run: `node tools/checks/validate-web-client.mjs` → PASS.
Run: `node tools/checks/validate-ui-design.mjs` → PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/web/src/features/library/LibrarySidebar.tsx apps/web/src/styles.css
git commit -m "feat(web): sidebar top = library switch entry (remove root-form)"
```

---

## Task 10: Optional Core empty-DB migration test (guarantee)

Locks the "fresh `.sqlite` self-migrates" guarantee the whole feature relies on. Core code is unchanged; this only adds a test.

**Files:**
- Modify: `crates/core/src/db/mod.rs` (add a `#[test]` in the existing `mod tests`)

- [ ] **Step 1: Write the test**

```rust
#[test]
fn brand_new_empty_file_migrates_and_lists_no_roots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.sqlite");
    let db = Database::open(&path).unwrap();
    db.apply_migrations().unwrap();
    let roots = db.list_roots().unwrap();
    assert!(roots.is_empty());
}
```

(Use the same tempfile/test helpers already present in `db/mod.rs` tests; if `tempfile` isn't a dev-dependency, use the existing pattern other DB tests use to make a temp path.)

- [ ] **Step 2: Run the test**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core brand_new_empty_file_migrates`
Expected: PASS.

- [ ] **Step 3: fmt + commit**

Run: `cargo fmt --all`
```bash
git add crates/core/src/db/mod.rs
git commit -m "test(core): brand-new empty sqlite migrates and lists no roots"
```

---

## Task 11: Integration — wire switcher into App, remove-folder submenu, real end-to-end

**Files:**
- Modify: `apps/web/src/app/App.tsx` (mount `LibrarySwitcher`, pass `onOpenSwitcher`, active name)
- Modify: `apps/web/src/features/library/LibrarySwitcher.tsx` (remove-folder submenu)

**Interfaces:**
- Consumes: Tasks 5–9 outputs.
- Produces: working switch/create/open from the real UI; sidebar opens the switcher; remove-folder picks among the active library's roots.

- [ ] **Step 1: Mount the switcher + wire the sidebar**

In `App.tsx`: `switcherOpen`/`setSwitcherOpen` already exist from Task 7. Pass `onOpenSwitcher={() => setSwitcherOpen(true)}` and `activeLibraryName={activeLibraryName}` to `LibrarySidebar`. Render `<LibrarySwitcher open={switcherOpen} onClose={() => setSwitcherOpen(false)} library={library} />` in the overlays slot. Derive `activeLibraryName` by calling `listLibraries()` on mount + on phase `"ready"` and storing the active one's name (small `useState` + effect).

- [ ] **Step 2: Remove-folder submenu**

In `LibrarySwitcher.tsx`, replace the remove-folder no-op with a small inline menu listing `library.roots` (name + a remove action calling `library.removeRoot(root.id)` if that exists; if `removeRoot` is not yet wired in `useLibraryData`, add a thin wrapper that calls `client.removeRoot(rootId)` then `library.refresh()`). Only render for the active library.

- [ ] **Step 3: Typecheck + full web/ui checks**

Run: `npm --workspace @megle/web run typecheck` → clean.
Run: `node tools/checks/validate-web-client.mjs` and `node tools/checks/validate-ui-design.mjs` → PASS.

- [ ] **Step 4: Real-window verification (CDP)**

Launch the dev harness (`MEGLE_REMOTE_DEBUG=1 MEGLE_AUTO_ADD_ROOT=... npm run dev`), then with the user (screenshots don't reflect everything): open the switcher, create a new library (auto-switches, loading card shows, new empty library loads), switch back (loading card, chrome + glass persist, old library reloads), add a folder to the active library. Confirm no `window.reload()` white flash.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/app/App.tsx apps/web/src/features/library/LibrarySwitcher.tsx
git commit -m "feat(web): wire LibrarySwitcher end-to-end (switch/create/open + remove-folder)"
```

---

## Task 12 (LOWER PRIORITY): Settings "资源库" section

**Files:**
- Modify: `apps/web/src/features/settings/settingsPanels.tsx`

**Interfaces:**
- Consumes: same bridge methods (Task 2) + `library` for active-library root management.
- Produces: a "资源库" settings section with the full library list + CRUD + active-library root management (superset of the switcher).

- [ ] **Step 1: Add the section**

Add a `LibrariesTab` / section to `settingsPanels.tsx` reusing `listLibraries`/`switchLibrary`/`createLibrary`/`openLibrary`/`removeLibraryFromList`/`renameLibraryEntry`. For active-library roots, list `library.roots` with add/remove. Follow the existing `SettingsGroup`/`StyleSlider` component conventions in the file.

- [ ] **Step 2: Typecheck + checks + commit**

Run: `npm --workspace @megle/web run typecheck` → clean; `node tools/checks/validate-ui-design.mjs` → PASS.
```bash
git add apps/web/src/features/settings/settingsPanels.tsx
git commit -m "feat(web): Settings library management section"
```

---

## Task 13: Full verification gate

- [ ] **Step 1: Run the full suite**

Run: `npm test`
Expected: all 8 checks PASS + rust tests pass (the known `file_ops_list_returns_keyset_page` parallel flake may need one re-run in isolation — verify with `cargo test -p megle-core file_ops_list_returns_keyset_page`).

- [ ] **Step 2: Real-window pass** — cold start shows the loading card and settles to the active library; switching preserves chrome + glass; create/open auto-switch. Confirm with the user.

- [ ] **Step 3: Update docs** — add a short "Multi-Library" subsection to `CLAUDE.md` (registry location, restart-Core-reusing-port+token model, `LibraryLoadPhase`, that Core is DB-path-driven and unchanged) and a one-line note in `AGENTS.md`. Commit.

## Parallelization

- **Sequential foundation:** Task 1 → Task 2 → Task 3 → Task 4 → Task 5 (Task 2 unblocks all UI; Task 4 depends on 1+3; Task 5 depends on 2).
- **Parallel once Task 2 lands (mock the bridge):** Tasks 6, 8, 9, 12 are independent UI components; Task 7 depends on Task 6. Dispatch 6+8+9 together, then 7, then integrate in Task 11.
- **Independent anytime:** Task 10 (Core test).
- **Last:** Task 11 (integration) → Task 13 (gate).

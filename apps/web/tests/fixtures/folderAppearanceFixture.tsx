import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { FolderRecord } from "@mangoo/core-client";
import { OverlayProvider } from "../../src/design/overlays";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import type { LibraryLoadPhase } from "../../src/core/libraryTypes";
import { FolderAppearanceEditor } from "../../src/features/library/FolderAppearanceEditor";
import { LibrarySidebar } from "../../src/features/library/LibrarySidebar";
import { SubfolderCard } from "../../src/features/library/SubfolderStrip";
import { PreviewPanel } from "../../src/features/preview/PreviewPanel";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

const fixtureWindow = window as typeof window & { __overlayLifecycleResult?: { ok: boolean; error?: string; scenarios?: string[] } };
const listeners = new Set<(phase: LibraryLoadPhase) => void>();
let revision = 1;
let libraryId = "first";
const original: FolderRecord = { id: 2, rootId: 1, parentId: 1, name: "Child", status: "active", hasChildren: false, mediaCount: 0, fileCount: 0, icon: "future-icon", color: "#123456" };
let saved = { ...original };
let library: LibraryState;
type TreeCase = { leaf: boolean; selected: boolean; expanded: boolean; icon: string | null };
let setTreeCase: (value: TreeCase | null) => void;
const requests: Array<{ path: string; method: string }> = [];
const patches: Array<{ icon: string; color: string; revision: string | null }> = [];
let holdPatch = false;
let rejectPatch = false;
let releasePatch: (() => void) | null = null;
window.mangooDesktop = { coreUrl: `${location.origin}/api`, onLibraryPhase: (listener) => { listeners.add(listener); return () => listeners.delete(listener); } };
function json(body: unknown, status = 200) { return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", "x-mangoo-context-revision": String(revision) } }); }
globalThis.fetch = async (input, init) => {
  const path = new URL(String(input)).pathname;
  requests.push({ path, method: init?.method ?? "GET" });
  if (path === "/api/libraries/active") return json({ library: { libraryUuid: libraryId, name: libraryId }, contextRevision: revision, runtimeGeneration: revision });
  if (path.endsWith("/appearance")) {
    const pair = JSON.parse(String(init?.body)) as { icon: string; color: string };
    patches.push({ ...pair, revision: new Headers(init?.headers).get("x-mangoo-context-revision") });
    const result = { ...saved, ...pair };
    if (holdPatch) await new Promise<void>((resolve) => { releasePatch = resolve; });
    if (rejectPatch) return json({ error: "Save failed" }, 500);
    if (new Headers(init?.headers).get("x-mangoo-context-revision") === String(revision)) saved = result;
    return json(result);
  }
  if (path === "/api/roots") return json({ items: [{ id: 1, rootFolderId: 1, displayName: "Fixture", path: "D:/fixture", enabled: true, hasChildren: true, mediaCount: 0, fileCount: 0, status: "active" }], nextCursor: null });
  if (path.endsWith("/ancestors")) return json({ items: [{ ...original, id: 1, parentId: null, name: "" }, { ...saved }], nextCursor: null });
  if (path.endsWith("/children")) return json({ items: [{ ...saved }], nextCursor: null });
  if (path.endsWith("/cache/stats")) return json({ total: 0, ready: 0, pending: 0, failed: 0 });
  return json({ items: [], nextCursor: null, totalCount: 0 });
};

function Probe() {
  library = useLibraryData({ activeLibraryId: libraryId, persistentThumbnailCacheAutoRefresh: false });
  const [treeCase, changeTreeCase] = useState<TreeCase | null>(null);
  setTreeCase = changeTreeCase;
  const [edit, setEdit] = useState<{ folder: FolderRecord; apply: LibraryState["updateFolderAppearance"] } | null>(null);
  const anchor = useRef<HTMLButtonElement>(null);
  const folder = library.folderChildrenByParent[1]?.[0];
  const treeLibrary = treeCase && folder ? {
    ...library,
    selectedFolderId: treeCase.selected ? folder.id : 1,
    expandedFolderIds: new Set(treeCase.expanded ? [1, folder.id] : [1]),
    folderChildrenByParent: {
      1: [{ ...folder, icon: treeCase.icon, hasChildren: !treeCase.leaf }],
      [folder.id]: treeCase.leaf ? [] : [{ ...folder, id: 3, parentId: folder.id, name: "Grandchild", hasChildren: false }]
    }
  } : library;
  return <>
    <button ref={anchor} id="open" style={{ position: "fixed", top: 20, left: 300 }} onClick={() => { if (folder) setEdit({ folder, apply: library.updateFolderAppearance }); }}>Appearance</button>
    <LibrarySidebar library={treeLibrary} />
    {folder ? <div id="card"><SubfolderCard folder={folder} coverLoaded coverMedia={[]} selected={false} onOpenFolder={() => {}} onSelectFolder={library.setSelectedFolderInfo} /></div> : null}
    <PreviewPanel selectedFolder={library.selectedFolderInfo} selectedFolderCoverMedia={[]} selectedMedia={null} />
    {edit ? <FolderAppearanceEditor folder={edit.folder} anchor={anchor.current} id="folder-appearance-editor" rootId="context-menu" open restoreFocusRef={anchor} onApply={(icon, color) => edit.apply(edit.folder.id, { icon, color })} onClose={() => setEdit(null)} /> : null}
  </>;
}
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function waitFor(predicate: () => boolean, message: string) { const deadline = Date.now() + 5000; while (!predicate() && Date.now() < deadline) await delay(20); check(predicate(), message); }
const editor = () => document.querySelector<HTMLElement>(".library-appearance-editor");
async function click(selector: string) { const element = document.querySelector<HTMLButtonElement>(selector); check(element, `missing ${selector}`); element.click(); await delay(40); }
async function input(selector: string, value: string) { const element = document.querySelector<HTMLInputElement>(selector); check(element, `missing ${selector}`); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(element, value); element.dispatchEvent(new Event("input", { bubbles: true })); await delay(30); }
async function open() { await click("#open"); await waitFor(() => !!editor(), "editor open"); await delay(80); }
const applySelector = ".library-appearance-apply";
function finishPatch() { holdPatch = false; releasePatch?.(); releasePatch = null; }

async function run() {
  const root = createRoot(document.getElementById("root")!);
  const scenarios: string[] = [];
  try {
    root.render(<OverlayProvider><Probe /></OverlayProvider>);
    await waitFor(() => !!library?.folderChildrenByParent[1]?.length && !library.loading, "folders loaded");
    library.setSelectedFolderInfo(library.folderChildrenByParent[1][0]);
    await delay(80);
    for (const state of [
      { leaf: true, selected: true, expanded: false, icon: null, expected: "folder-open" },
      { leaf: true, selected: false, expanded: true, icon: null, expected: "folder" },
      { leaf: false, selected: true, expanded: false, icon: "folder", expected: "folder" },
      { leaf: false, selected: false, expanded: true, icon: "folder", expected: "folder-open" },
      { leaf: false, selected: false, expanded: true, icon: "camera", expected: "camera" },
      { leaf: false, selected: true, expanded: false, icon: "camera", expected: "camera" },
      { leaf: true, selected: true, expanded: false, icon: "future-icon", expected: "folder-open" }
    ]) {
      setTreeCase(state); await delay(40);
      const row = document.querySelector('[data-folder-id="2"]');
      check(row, "folder row exists");
      check(row?.querySelector(`.tree-label [data-identity-icon="${state.expected}"]`), `tree artwork follows original semantics: ${JSON.stringify(state)}`);
      check(row.getAttribute("aria-expanded") === (state.leaf ? null : String(state.expanded)), "leaf selection never implies tree expansion");
    }
    setTreeCase(null); await delay(40);
    scenarios.push("folder-open-selection-semantics");
    check(document.querySelector('#card [data-identity-icon="folder"][data-identity-variant="symbol"]'), "unknown saved icon uses plain folder fallback");
    await open();
    check(document.querySelector('[aria-label="文件夹图标"]'), "folder-specific group");
    check(document.querySelector('.library-appearance-preview [data-identity-icon="folder"]'), "unknown editor fallback");
    await click('[data-color="#5ce1a6"]');
    check(patches.length === 0, "preview does not save");
    await click(applySelector);
    await waitFor(() => !editor(), "color-only apply closes");
    check(patches[0].icon === "future-icon", "untouched unknown key retained");
    scenarios.push("unknown-retention");

    await open();
    await click('[data-icon="camera"]');
    await input('[aria-label="HEX 颜色"]', "#bad-value");
    check(document.querySelector<HTMLButtonElement>(applySelector)?.disabled, "invalid HEX disables apply");
    await input('[aria-label="HEX 颜色"]', "#224466");
    check(document.querySelector('.library-appearance-preview [data-identity-icon="camera"]'), "camera preview");
    const mediaBefore = requests.filter(({ path }) => path === "/api/media").length;
    const mediaRef = library.media;
    holdPatch = true;
    await click(applySelector);
    await click(applySelector);
    check(Number(patches.length) === 2, "one pending pair");
    check(library.folderChildrenByParent[1][0].icon === "future-icon", "draft not painted as saved");
    finishPatch();
    await waitFor(() => !editor() && library.folderChildrenByParent[1][0].icon === "camera", "authoritative folder updated");
    check(library.selectedFolderInfo?.icon === "camera", "selected folder updated");
    check(library.media === mediaRef && requests.filter(({ path }) => path === "/api/media").length === mediaBefore, "appearance does not reload media");
    for (const selector of ['.tree-label', '#card', '.preview-folder-cover-empty']) check(document.querySelector(`${selector} [data-identity-icon="camera"]`), `${selector} shows saved camera`);
    check(patches[1].color === "#224466" && patches[1].revision === "1", "custom color and captured revision");
    scenarios.push("paired-save-consumers-no-teardown");

    await open(); await click(".library-appearance-reset");
    check(document.querySelector('.library-appearance-preview [data-identity-icon="folder"]'), "folder default draft");
    const cancel = [...editor()!.querySelectorAll("button")].find((button) => button.textContent === "取消")!;
    cancel.click(); await waitFor(() => !editor(), "cancel closes");
    check(Number(patches.length) === 2, "reset cancel does not write");
    await open(); check(document.querySelector('.library-appearance-preview [data-identity-icon="camera"]'), "reopen current saved appearance");
    await click(".library-appearance-reset"); rejectPatch = true;
    await click(applySelector); await waitFor(() => !!editor()?.querySelector('[role="status"], [role="alert"]'), "failure visible");
    check(editor(), "failed draft retained"); rejectPatch = false;
    await click(applySelector); await waitFor(() => !editor(), "retry succeeds");
    check(saved.icon === "folder" && saved.color === "#ffffff", "explicit folder reset");
    scenarios.push("reset-cancel-retry");

    const oldApply = library.updateFolderAppearance;
    holdPatch = true;
    const pending = oldApply(2, { icon: "heart", color: "#abc" });
    await waitFor(() => releasePatch !== null, "pending old context");
    for (const listener of listeners) listener("starting-core");
    await delay(80);
    libraryId = "second"; revision = 2; saved = { ...original, name: "Second child", icon: "star", color: "#fff" };
    for (const listener of listeners) listener("loading-media");
    await waitFor(() => library.folderChildrenByParent[1]?.[0]?.name === "Second child", `second library loaded: ${JSON.stringify({ folders: library.folderChildrenByParent, requests: requests.slice(-8) })}`);
    finishPatch(); await pending; await delay(80);
    check(String(library.folderChildrenByParent[1][0].icon) === "star", "late first Library cannot paint second");
    const count = patches.length;
    let staleRejected = false;
    try { await oldApply(2, { icon: "camera", color: "#123" }); } catch { staleRejected = true; }
    check(staleRejected && patches.length === count, "old editor callback cannot submit to new context");
    scenarios.push("switch-fence");
    fixtureWindow.__overlayLifecycleResult = { ok: true, scenarios };
  } catch (error) { fixtureWindow.__overlayLifecycleResult = { ok: false, error: `${error instanceof Error ? error.stack : String(error)}\n${JSON.stringify({ folders: library?.folderChildrenByParent, listeners: listeners.size, requests: requests.slice(-20) })}` }; }
  finally { finishPatch(); root.unmount(); }
}
void run();

import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { FolderRecord, RootRecord } from "@mangoo/core-client";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import { OverlayProvider } from "../../src/design/overlays";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import { useOverlayScrollbars } from "../../src/app-shell/useOverlayScrollbars";
import { LibrarySidebar } from "../../src/features/library/LibrarySidebar";
import { LibraryAppearanceEditor } from "../../src/features/library/LibraryAppearanceEditor";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

const fixtureWindow = window as typeof window & {
  __overlayLifecycleResult?: { ok: boolean; error?: string; scenarios?: string[]; geometry?: unknown[]; treeGeometry?: unknown[] };
  __overlayLifecycleInputRequest?: { id: number; type: string; x?: number; y?: number; keyCode?: string };
  __overlayLifecycleInputResponseId?: number;
};
const scenarios: string[] = [], geometry: unknown[] = [];
const treeGeometry: unknown[] = [];
let library: LibraryState, failId: number | null = null, holdId: number | null = null, releaseRead: (() => void) | null = null;
let contexts = 0, picked = "folder";
const childRequests: number[] = [], mediaRequests: string[] = [];
const rootRecord: RootRecord = { id: 1, rootFolderId: 1, displayName: "Root", path: "D:/fixture", enabled: true, reachable: false, createdAt: 1, lastScanAt: null, mediaCount: 12, fileCount: 12, color: "#336699", icon: "folder" };
const makeFolder = (id: number, parentId: number, hasChildren = false, color: string | null = null): FolderRecord => ({ id, parentId, rootId: 1, name: `Folder ${id}`, status: "active", hasChildren, mediaCount: 10, fileCount: 10, icon: "folder", color });
let rows = [makeFolder(2, 1, true, "#dd8844"), makeFolder(3, 1), makeFolder(4, 1), makeFolder(5, 1, true), makeFolder(6, 1, true), makeFolder(7, 1),
  makeFolder(20, 2, true, "#66bb99"), makeFolder(21, 2), makeFolder(200, 20),
  ...Array.from({ length: 12 }, (_, index) => makeFolder(40 + index, 1)), makeFolder(90, 1, true), makeFolder(900, 90, true), makeFolder(901, 90), makeFolder(9000, 900)];
// A transport without hasChildren remains unknown, not a false leaf.
Object.assign(rows.find((row) => row.id === 4)!, { hasChildren: undefined });
window.mangooDesktop = { coreUrl: `${location.origin}/api` };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", "x-mangoo-context-revision": "1" } });
globalThis.fetch = async (input, init) => {
  const url = new URL(String(input)), path = url.pathname;
  if (path === "/api/libraries/active") return json({ library: { libraryUuid: "tree-fixture", name: "Fixture" }, contextRevision: 1, runtimeGeneration: 1 });
  if (path === "/api/roots") return json({ items: [rootRecord, { ...rootRecord, id: 2, rootFolderId: null, displayName: "Pre-scan" }, { ...rootRecord, id: 3, rootFolderId: 30, displayName: "Empty root" }], nextCursor: null });
  const appearance = path.match(/\/(roots|folders)\/(\d+)\/appearance$/);
  if (appearance) {
    const target = appearance[1] === "roots" ? rootRecord : rows.find(({ id }) => id === Number(appearance[2]))!;
    Object.assign(target, JSON.parse(String(init?.body)));
    return json({ ...target });
  }
  const children = path.match(/\/folders\/(\d+)\/children$/);
  if (children) {
    const id = Number(children[1]); childRequests.push(id);
    if (id === holdId) await new Promise<void>((resolve) => { releaseRead = resolve; });
    if (id === failId) return json({ error: "Folder read failed" }, 500);
    const cursor = id === 6 && !url.searchParams.has("cursor") ? "more" : null;
    return json({ items: rows.filter((row) => row.parentId === id), nextCursor: cursor, sortState: { rootId: 1, folderId: id, explicitRule: null, effectiveOrder: "name_asc", source: null, revision: "0" } });
  }
  if (path.endsWith("/ancestors")) return json({ items: [], nextCursor: null });
  if (path === "/api/media") mediaRequests.push(path);
  if (path.endsWith("/cache/stats")) return json({ total: 0, ready: 0, pending: 0, failed: 0 });
  return json({ items: [], nextCursor: null, totalCount: 0 });
};
function Probe() {
  library = useLibraryData({ activeLibraryId: "tree-fixture", persistentThumbnailCacheAutoRefresh: false });
  useOverlayScrollbars();
  const [editor, setEditor] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  return <><div style={{ position: "absolute", left: 20, top: 20, width: 280, height: 260 }}>
    <LibrarySidebar library={library} onRootContextMenu={() => contexts++} onFolderContextMenu={() => contexts++} />
  </div><button id="open-picker" ref={anchor} style={{ position: "absolute", left: 320, top: 20 }} onClick={() => setEditor(true)}>Appearance</button>
  {editor ? <LibraryAppearanceEditor anchor={anchor.current} id="tree-picker" rootId="context-menu" name="Fixture" icon="folder" kind="folder" open onApply={(icon) => { picked = icon; }} onClose={() => setEditor(false)} /> : null}</>;
}
const delay = (ms = 40) => new Promise((resolve) => setTimeout(resolve, ms));
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function waitFor(fn: () => boolean, message: string) { const deadline = Date.now() + 4500; while (!fn() && Date.now() < deadline) await delay(20); check(fn(), message); }
function row(id: number) { const node = document.querySelector<HTMLElement>(`.tree-item[data-folder-id="${id}"]`); check(node, `row ${id}`); return node; }
function toggle(id: number) { return row(id).querySelector<HTMLButtonElement>("button.tree-disclosure"); }
async function expand(id: number) { check(toggle(id), `toggle ${id}`); toggle(id)!.click(); await delay(80); }
function guides(id: number) { return [...row(id).querySelectorAll<HTMLElement>(".tree-guide")]; }
function checkGuideGroups(scale: number) {
  let groups = 0, segments = 0, maxCenterError = 0;
  for (const parent of document.querySelectorAll<HTMLElement>('.tree-item[aria-expanded="true"]')) {
    const group = parent.parentElement!.querySelector<HTMLElement>(':scope > [role="group"]')!;
    const children = [...group.querySelectorAll<HTMLElement>(':scope > .tree-branch > .tree-item, :scope > .tree-page-row')];
    if (!children.length) continue;
    groups++;
    const caret = parent.querySelector<HTMLElement>('.tree-disclosure')!.getBoundingClientRect();
    const glyph = parent.querySelector<SVGElement>('.tree-disclosure svg')!.getBoundingClientRect();
    const center = caret.left + caret.width / 2;
    check(Math.abs(glyph.left + glyph.width / 2 - center) < 0.03, `actual expand glyph is centered in its caret slot ${JSON.stringify({ parent: parent.dataset.folderId, caret: caret.toJSON(), glyph: glyph.toJSON(), padding: getComputedStyle(parent.querySelector('.tree-disclosure')!).padding })}`);
    const owned = [...group.querySelectorAll<HTMLElement>('.tree-guide')].filter((guide) => guide.style.getPropertyValue('--tree-guide-depth') === parent.dataset.depth);
    const terminals = owned.filter((guide) => guide.dataset.treeGuideLast === 'true');
    check(terminals.length === 1, `one terminal bend per visible sibling group ${parent.dataset.folderId}`);
    check(children.at(-1)!.contains(terminals[0]), 'terminal belongs to final child or paging row');
    for (const guide of owned) {
      segments++;
      const bounds = guide.getBoundingClientRect(), ownerRow = guide.closest<HTMLElement>('.tree-item, .tree-page-row')!.getBoundingClientRect();
      const error = Math.abs(bounds.left + bounds.width / 2 - center);
      maxCenterError = Math.max(maxCenterError, error);
      check(error < 0.03, `guide stroke center aligns with parent caret: ${parent.dataset.folderId}, delta ${error}`);
      const bend = getComputedStyle(guide, '::after'), terminal = guide.dataset.treeGuideLast === 'true';
      check((bend.content !== 'none' && bend.content !== 'normal') === terminal, 'only terminal segments paint horizontal strokes');
      check((guide.dataset.treeGuideConnector === 'true') === terminal, 'intermediate and ancestor segments have no connector');
      check(Math.abs(bounds.height - ownerRow.height * (terminal ? 0.5 : 1)) < 0.03, 'only terminal vertical stops at row midpoint');
      check(Math.abs(bounds.width - scale) < 0.03, 'guide remains one logical pixel');
      if (terminal) {
        check(Math.abs(bounds.top + (parseFloat(bend.top) + parseFloat(bend.height) / 2) * scale - (ownerRow.top + ownerRow.height / 2)) < 0.03, 'terminal horizontal stroke centers at row midpoint');
      }
    }
  }
  return { groups, segments, maxCenterError };
}
let inputId = 0;
async function native(request: { type: "key"; keyCode: string } | { type: "mouse-move" | "mouse-click"; x: number; y: number }) {
  const id = ++inputId; fixtureWindow.__overlayLifecycleInputRequest = { ...request, id };
  await waitFor(() => fixtureWindow.__overlayLifecycleInputResponseId === id, `native ${request.type}`);
}
async function run() {
  const mounted = createRoot(document.getElementById("root")!);
  try {
    mounted.render(<LiquidGlassLayer><OverlayProvider><Probe /></OverlayProvider></LiquidGlassLayer>);
    await waitFor(() => !!library?.folderChildrenByParent[1]?.length && !library.loading, "loaded root");
    await delay(100);
    check(childRequests.every((id) => Number(id) === 1), "rendering does not probe each child");
    check(!toggle(3) && !row(3).hasAttribute("aria-expanded"), "known leaf has no toggle or expanded state");
    check(toggle(4), "unknown metadata remains expandable");
    const preScan = document.querySelector('[data-root-id="2"]')!;
    check(!preScan.querySelector("button.tree-disclosure") && !preScan.hasAttribute("aria-expanded"), "pre-scan root keeps content without disabled expander");
    await expand(30); check(!toggle(30) && !row(30).hasAttribute("aria-expanded"), "completed empty root becomes leaf");
    holdId = 4; await expand(4); check(toggle(4) && library.loadingFolderIds.has(4), "loading unknown remains reachable");
    holdId = null; releaseRead?.(); await waitFor(() => !toggle(4), "completed empty unknown becomes leaf");
    failId = 4;
    await library.requestFolderChildren(4, { force: true }).catch(() => undefined); await delay();
    check(toggle(4) && library.failedFolderIds.has(4), "failed refresh cannot strand former leaf");
    failId = null; rows.push(makeFolder(400, 4));
    const beforeRetry = childRequests.length; await expand(4);
    await waitFor(() => !!library.folderChildrenByParent[4]?.length, "retry reads refreshed page rather than stale cache");
    check(childRequests.length > beforeRetry && !library.failedFolderIds.has(4), "successful retry clears failure");
    scenarios.push("leaf-unknown-loading-error-retry-root");

    // Positive child data overrides stale hasChildren=false after a scan.
    await library.requestFolderChildren(3); await delay();
    check(!toggle(3) && library.folderChildNextCursorByParent[3] === null, "leaf has completed empty page");
    rows.push(makeFolder(300, 3)); rows.find((item) => item.id === 3)!.hasChildren = true;
    const beforeParentRefresh = childRequests.filter((id) => id === 3).length;
    await library.requestFolderChildren(1, { force: true }); await delay();
    check(toggle(3) && library.folderChildNextCursorByParent[3] === undefined, "fresh parent metadata retires old empty child completion");
    check(childRequests.filter((id) => id === 3).length === beforeParentRefresh, "parent refresh never eagerly probes children");
    await expand(3); await waitFor(() => !!library.folderChildrenByParent[3]?.length, "restored toggle loads new child on demand");
    const emptyChild = library.requestFolderChildren(7, { force: true });
    rows.find((item) => item.id === 7)!.hasChildren = true;
    rows.push(makeFolder(700, 7));
    const positiveParent = library.requestFolderChildren(1, { force: true });
    await Promise.all([emptyChild, positiveParent]); await delay();
    check(toggle(7) && library.folderChildNextCursorByParent[7] === undefined, "same-batch parent refresh sees just-committed empty child cursor");
    await expand(6);
    check(toggle(6) && row(6).parentElement!.querySelector('.tree-load-more'), "empty page with next cursor is not leaf");
    await library.requestFolderChildren(1, { force: true }); await delay();
    check(library.folderChildNextCursorByParent[6] === 'more', "parent metadata does not invalidate an incomplete empty page");
    await expand(6); await expand(6);
    check(row(6).getAttribute('aria-expanded') === 'true' && row(6).parentElement!.querySelector('.tree-load-more'), "cached empty paged branch survives collapse and reopen");
    rows.push(makeFolder(60, 6)); await library.requestFolderChildren(6, { force: true }); await delay();
    const page = row(6).parentElement!.querySelector<HTMLElement>('.tree-page-row')!;
    check(guides(60).at(-1)!.dataset.treeGuideLast !== 'true' && getComputedStyle(guides(60).at(-1)!, '::after').content === 'none', 'last loaded child continues vertically while another page exists');
    check(page.querySelectorAll('[data-tree-guide-last="true"]').length === 1, 'paging row owns the visible terminal bend');
    checkGuideGroups(1);
    await library.loadMoreFolderChildren(6); await delay();
    check(row(60), "load more remains reachable");
    check(!row(6).parentElement!.querySelector('.tree-page-row') && guides(60).at(-1)!.dataset.treeGuideLast === 'true', 'completed pagination transfers terminal to final child');
    scenarios.push("child-added-pagination");

    const selected = library.selectedFolderId, mediaCount = mediaRequests.length;
    await expand(2); await expand(20);
    check(library.selectedFolderId === selected && mediaRequests.length === mediaCount, "toggles change expansion only");
    const rootLabel = row(1).querySelector<HTMLElement>(".tree-label")!, rootToggle = toggle(1)!;
    const rootLabelBounds = rootLabel.getBoundingClientRect(), rootToggleBounds = rootToggle.getBoundingClientRect();
    check(rootToggleBounds.right <= rootLabelBounds.left, "toggle and content hit regions do not overlap");
    check(rootLabel.contains(row(1).querySelector('.tree-media-count')), "count belongs to selectable content");
    check(getComputedStyle(row(1)).backgroundColor === "rgba(0, 0, 0, 0)" && (getComputedStyle(rootLabel).backgroundImage !== "none" || getComputedStyle(rootLabel).backgroundColor !== "rgba(0, 0, 0, 0)"), "selection fills content only");
    rootToggle.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true })); check(contexts === 0, "caret does not open folder context");
    rootLabel.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true })); check(Number(contexts) === 1, "content preserves context menu");
    rootToggle.focus(); await native({ type: "key", keyCode: "Tab" });
    check(document.activeElement === rootLabel && rootLabel.matches(":focus-visible"), "keyboard focus moves from independent caret to content");
    check(getComputedStyle(rootLabel).outlineStyle !== "none", "content focus remains visible");
    await native({ type: "mouse-move", x: Math.round(rootLabelBounds.left + 8), y: Math.round(rootLabelBounds.top + 16) }); await delay(100);
    check(rootLabel.dataset.interactivePointerTarget === "true" && !row(1).hasAttribute("data-interactive-pointer-target") && !rootToggle.hasAttribute("data-interactive-pointer-target"), "shared pointer driver targets content only");
    row(3).querySelector<HTMLButtonElement>('.tree-label')!.click(); await waitFor(() => library.selectedFolderId === 3, "content selects folder");
    scenarios.push("toggle-content-focus-pointer-context");

    check(getComputedStyle(toggle(2)!).color === "rgb(221, 136, 68)", "caret uses own folder color");
    check(getComputedStyle(guides(2).at(-1)!).color === "rgb(51, 102, 153)", "first branch follows root color");
    check(getComputedStyle(guides(20).at(-1)!).color === "rgb(221, 136, 68)", "nested branch follows owning parent");
    check(getComputedStyle(guides(200).at(-1)!).color === "rgb(102, 187, 153)", "explicit nested color overrides its own branch");
    const mediaBeforeAppearance = mediaRequests.length;
    await library.updateFolderAppearance(2, { icon: "camera", color: "#abcdef" });
    await library.updateRootAppearance(1, { icon: "folder", color: "#ffffff" }); await delay();
    await waitFor(() => getComputedStyle(toggle(2)!).color === "rgb(171, 205, 239)" && getComputedStyle(guides(20).at(-1)!).color === "rgb(171, 205, 239)", "live save updates caret and child branch");
    check(getComputedStyle(guides(2).at(-1)!).color === "rgb(255, 255, 255)", "root reset updates branch to default white");
    check(row(1).querySelector('[aria-label="资源库离线"]') && mediaRequests.length === mediaBeforeAppearance, "status and media remain independent");
    scenarios.push("live-parent-color-root-reset");

    await expand(90); await expand(900);
    const last = guides(90).at(-1)!;
    check(last.dataset.treeGuideLast === "true" && Math.abs(last.getBoundingClientRect().height - 16) < 0.1, "last child guide ends at row midpoint");
    check(guides(900).length === 1, "last root branch does not run through descendants");
    check(guides(901).at(-1)!.dataset.treeGuideLast === "true", "last nested sibling connector stops");
    const treeHost = document.querySelector<HTMLElement>('.library-sidebar')!.parentElement!;
    for (const [width, scale] of [[280, 1], [220, 1], [280, 1.25]]) {
      treeHost.style.width = `${width}px`; treeHost.style.zoom = String(scale);
      window.dispatchEvent(new Event('resize')); await delay(100);
      treeGeometry.push({ width, scale, ...checkGuideGroups(scale) });
    }
    treeHost.style.width = '280px'; treeHost.style.zoom = '1'; window.dispatchEvent(new Event('resize')); await delay();
    await expand(900);
    check(!document.querySelector('.tree-item[data-folder-id="9000"]') && !row(900).parentElement!.querySelector('[role="group"]'), 'collapse removes descendant guides with rows');
    checkGuideGroups(1);
    await expand(900);
    for (const guide of guides(9000)) check(getComputedStyle(guide).pointerEvents === "none", "guide never intercepts input");
    const sidebar = document.querySelector<HTMLElement>('.library-sidebar')!;
    sidebar.scrollTop = 58; sidebar.dispatchEvent(new Event('scroll')); await delay(100);
    check(row(1).dataset.treeStickyPinned === "true", "root pins while scrolling");
    const occluded = [...sidebar.querySelectorAll<HTMLElement>('[data-tree-sticky-occluded="true"]')];
    check(occluded.some((node) => node.querySelector('.tree-guide') && node.style.clipPath.startsWith('inset(')), "sticky clip includes descendant guide and content together");
    sidebar.scrollTop = 0; sidebar.dispatchEvent(new Event('scroll')); await delay();
    scenarios.push("nested-last-child-sticky-clipping");

    document.querySelector<HTMLButtonElement>('#open-picker')!.click();
    await waitFor(() => !!document.querySelector('.library-appearance-editor'), 'picker opened'); await delay(150);
    const popup = document.querySelector<HTMLElement>('.library-appearance-editor')!;
    const scroll = popup.querySelector<HTMLElement>('.library-icon-picker-grid')!;
    for (const width of [344, 280]) {
      popup.style.width = `${width}px`; window.dispatchEvent(new Event('resize')); await delay(120);
      scroll.scrollTop = 0;
      const group = scroll.querySelector<HTMLElement>('.library-icon-picker-group-grid')!;
      const cells = [...group.querySelectorAll<HTMLButtonElement>('.library-icon-picker-cell')];
      const top = cells[0].getBoundingClientRect().top;
      const firstRow = cells.filter((cell) => Math.abs(cell.getBoundingClientRect().top - top) < 2);
      const cell = firstRow.at(-1)!, bounds = cell.getBoundingClientRect(), scrollBounds = scroll.getBoundingClientRect();
      await native({ type: 'mouse-move', x: Math.round(bounds.left + bounds.width / 2), y: Math.round(bounds.top + 16) });
      scroll.dispatchEvent(new Event('scroll')); await delay(40);
      const rail = document.querySelector<HTMLElement>('.mangoo-overlay-scrollbar-vertical')!, railBounds = rail.getBoundingClientRect();
      const thumbBounds = rail.querySelector<HTMLElement>('.mangoo-overlay-scrollbar-thumb')!.getBoundingClientRect();
      check(rail.dataset.scrollbarVisible === 'true', `real shared rail visible ${JSON.stringify({ width, cell: bounds.toJSON(), scroll: scrollBounds.toJSON(), scrollHeight: scroll.scrollHeight, clientHeight: scroll.clientHeight, rail: railBounds.toJSON(), hit: document.elementFromPoint(bounds.left + bounds.width / 2, bounds.top + 16)?.outerHTML.slice(0, 150) })}`);
      check(railBounds.width === 10 && thumbBounds.width === 5, 'shared rail and thumb geometry preserved');
      check(bounds.right < railBounds.left, `entire rail separated from last icon at width ${width}`);
      check(Math.abs(scrollBounds.right - thumbBounds.right - 5) < 0.6, 'thumb keeps global 5px edge gap with controller rounding');
      check(bounds.width >= 28 && bounds.height === 28 && getComputedStyle(group).gap === '0px', '28px minimum targets and zero grid gap');
      check(cell.querySelector('svg')!.getBoundingClientRect().width === 18, '18px glyph preserved');
      check(firstRow.length === (width === 344 ? 11 : 8), 'normal and narrow popups use eleven and eight columns');
      check(scroll.scrollWidth === scroll.clientWidth && popup.scrollWidth === popup.clientWidth, 'no horizontal overflow');
      check(scroll.clientHeight === 172, 'group viewport remains 172px');
      const body = popup.querySelector<HTMLElement>('.overlay-dialog-body')!;
      check(body.scrollHeight <= body.clientHeight + 1, 'no nested body scrollbar at standard height');
      const hit = document.elementFromPoint(bounds.right - 1, bounds.top + 16);
      check(hit?.closest('.library-icon-picker-cell') === cell, 'rightmost cell edge remains hit-testable');
      check(document.elementFromPoint(railBounds.left + 1, bounds.top + 16)?.closest('.mangoo-overlay-scrollbar') === rail, 'rail hit stays on rail');
      await native({ type: 'mouse-click', x: Math.floor(bounds.right - 1), y: Math.round(bounds.top + 16) });
      check(cell.getAttribute('aria-checked') === 'true', 'rightmost icon click selects identity');
      geometry.push({ width, columns: firstRow.length, cellWidth: bounds.width, cellHeight: bounds.height, glyphWidth: parseFloat(getComputedStyle(cell.querySelector('svg')!).width), gap: getComputedStyle(group).gap, viewportHeight: scroll.clientHeight, railClearance: railBounds.left - bounds.right, cellRight: bounds.right, railLeft: railBounds.left, thumbCenter: thumbBounds.left + 2.5, ownerRight: scrollBounds.right });
    }
    scenarios.push('picker-full-rail-clearance-normal-narrow');
    fixtureWindow.__overlayLifecycleResult = { ok: true, scenarios, geometry, treeGeometry };
  } catch (error) {
    fixtureWindow.__overlayLifecycleResult = { ok: false, scenarios, geometry, treeGeometry, error: `${error instanceof Error ? error.stack : String(error)}\n${JSON.stringify({ childRequests, picked, selected: library?.selectedFolderId })}` };
  } finally { releaseRead?.(); mounted.unmount(); }
}
void run();

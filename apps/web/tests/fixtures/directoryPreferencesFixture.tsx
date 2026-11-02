import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import type { DirectorySortRule, DirectorySortState, FolderRecord, RootRecord } from "@mangoo/core-client";
import { queryClient } from "../../src/core/queryClient";
import { OverlayProvider } from "../../src/design/overlays";
import { App } from "../../src/app/App";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import type { LibraryLoadPhase } from "../../src/core/libraryTypes";
import { compareFolderRecords } from "../../src/core/folderTreeState";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

const fixtureWindow = window as typeof window & { __overlayLifecycleResult?: { ok: boolean; error?: string; scenarios?: string[] } };
const listeners = new Set<(phase: LibraryLoadPhase) => void>();
let revision = 1, preferenceRevision = 0, libraryId = "first";
let library: LibraryState;
let rootRecord: RootRecord = { id: 1, rootFolderId: 1, displayName: "Fixture root", path: "D:/fixture", enabled: true, reachable: false, createdAt: 1, lastScanAt: null, mediaCount: 0, fileCount: 0, icon: null, color: null };
const folder = (id: number, parentId: number | null, name: string): FolderRecord => ({ id, parentId, name, rootId: 1, status: "active", hasChildren: id === 2 || id === 3, mediaCount: 0, fileCount: 0, icon: null, color: null });
const rows = [folder(1, null, ""), folder(2, 1, "Alpha"), folder(3, 1, "Zulu"), folder(4, 2, "Grandchild"), folder(5, 4, "Deep")];
const wide = Array.from({ length: 510 }, (_, i) => folder(100 + i, 3, `item-${String(i).padStart(4, '0')}`));
rows.push(...wide);
const rules = new Map<string, DirectorySortRule>();
const requests: Array<{ path: string; method: string; body: unknown }> = [];
const focusEvents: string[] = [];
document.addEventListener('focusin', (event) => focusEvents.push(`focus:${(event.target as HTMLElement).outerHTML.slice(0, 240)}`), true);
window.addEventListener('blur', () => focusEvents.push('window-blur'));
let failSort = false, holdSort = false, holdPage = false, holdRoots = false, holdAppearance = false;
let staleCursorOnce = false, failChildId: number | null = null;
let releaseSort: (() => void) | null = null, releasePage: (() => void) | null = null, releaseRoots: (() => void) | null = null, releaseAppearance: (() => void) | null = null;
function sortState(kind: string, id: number): DirectorySortState {
  const root = kind === "root" || id === 1;
  const key = root ? "root:1" : `folder:${id}`;
  const explicitRule = rules.get(key) ?? null;
  let rule = explicitRule, source: DirectorySortState['source'] = rule ? { rootId: 1, folderId: root ? null : id } : null;
  let parent = rows.find((row) => row.id === id)?.parentId;
  while (!rule && parent != null) {
    const candidate = rules.get(parent === 1 ? 'root:1' : `folder:${parent}`);
    if (candidate?.recursive) { rule = candidate; source = { rootId: 1, folderId: parent === 1 ? null : parent }; }
    parent = rows.find((row) => row.id === parent)?.parentId;
  }
  return { rootId: 1, folderId: root ? null : id, explicitRule, effectiveOrder: rule?.order ?? "name_asc", source, revision: String(preferenceRevision) };
}
window.mangooDesktop = { coreUrl: `${location.origin}/api`, onLibraryPhase: (listener) => { listeners.add(listener); return () => listeners.delete(listener); } };
function json(body: unknown, status = 200) { return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", "x-mangoo-context-revision": String(revision) } }); }
globalThis.fetch = async (input, init) => {
  const url = new URL(String(input)), path = url.pathname, method = init?.method ?? "GET";
  const body = init?.body ? JSON.parse(String(init.body)) : null;
  requests.push({ path, method, body });
  if (path === "/api/libraries/active") return json({ library: { libraryUuid: libraryId, name: libraryId }, contextRevision: revision, runtimeGeneration: revision });
  if (path === "/api/libraries") return json({ items: [{ libraryUuid: libraryId, name: libraryId, active: true }] });
  if (path === "/api/roots") {
    const response = json({ items: [{ ...rootRecord }], nextCursor: null });
    if (holdRoots) await new Promise<void>((resolve) => { releaseRoots = resolve; });
    return response;
  }
  if (path.endsWith('/appearance')) {
    const result = { ...rootRecord, ...body, reachable: true };
    if (holdAppearance) await new Promise<void>((resolve) => { releaseAppearance = resolve; });
    rootRecord = { ...rootRecord, ...body };
    return json(result);
  }
  const sort = path.match(/\/(roots|folders)\/(\d+)\/sort$/);
  if (sort) {
    const kind = sort[1] === 'roots' ? 'root' : 'folder', id = Number(sort[2]), key = `${kind}:${id}`;
    if (method !== 'GET') {
      if (holdSort) await new Promise<void>((resolve) => { releaseSort = resolve; });
      if (failSort) return json({ error: 'Sort fixture failed' }, 500);
      const old = rules.get(key);
      const changed = JSON.stringify(old ?? null) !== JSON.stringify(body);
      if (method === 'DELETE') rules.delete(key); else rules.set(key, body);
      if (changed) preferenceRevision += 1;
      return json({ state: sortState(kind, id), invalidateDescendants: changed && Boolean(old?.recursive || body?.recursive) });
    }
    return json(sortState(kind, id));
  }
  const children = path.match(/\/folders\/(\d+)\/children$/);
  if (children) {
    const id = Number(children[1]), state = sortState('folder', id);
    if (id === failChildId) return json({ error: 'Child page failed' }, 500);
    if (staleCursorOnce && url.searchParams.has('cursor')) { staleCursorOnce = false; return json({ code: 'stale_folder_cursor', error: 'Cursor expired' }, 400); }
    const ordered = rows.filter((row) => row.parentId === id).sort((a, b) => compareFolderRecords(a, b, state.effectiveOrder));
    const offset = Number(url.searchParams.get('cursor') ?? 0), limit = Number(url.searchParams.get('limit') ?? 500);
    const response = json({ items: ordered.slice(offset, offset + limit), nextCursor: offset + limit < ordered.length ? String(offset + limit) : null, sortState: state });
    if (holdPage && id === 3) await new Promise<void>((resolve) => { releasePage = resolve; });
    return response;
  }
  if (path.endsWith('/ancestors')) {
    let row = rows.find((item) => item.id === Number(path.split('/').at(-2))), pathRows: FolderRecord[] = [];
    while (row) { pathRows.unshift(row); row = rows.find((item) => item.id === row?.parentId); }
    return json({ items: pathRows, nextCursor: null });
  }
  if (path.endsWith('/cache/stats')) return json({ total: 0, ready: 0, pending: 0, failed: 0 });
  return json({ items: [], nextCursor: null, totalCount: 0 });
};
function Probe() { library = useLibraryData({ activeLibraryId: libraryId, persistentThumbnailCacheAutoRefresh: false }); return null; }
const delay = (ms = 40) => new Promise((resolve) => setTimeout(resolve, ms));
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function waitFor(predicate: () => boolean, message: string) { const end = Date.now() + 5000; while (!predicate() && Date.now() < end) await delay(20); check(predicate(), message); }
async function button(text: string, selector = 'button') {
  const find = () => [...document.querySelectorAll<HTMLButtonElement>(selector)].find((node) => (node.querySelector('.overlay-menu-item-label')?.textContent ?? node.textContent) === text && !node.disabled && getComputedStyle(node).visibility !== 'hidden');
  await waitFor(() => Boolean(find()), `visible enabled button: ${text}`);
  const match = find()!; match.focus(); match.click(); await delay(); return match;
}
async function context(selector: string) { const row = document.querySelector(selector); check(row, `context target ${selector}`); (row.querySelector('.tree-label') ?? row).dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 160, clientY: 170 })); await waitFor(() => !!document.getElementById('context-menu-root'), 'context menu opens'); }
async function closeMenus() { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await delay(100); document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await delay(160); }

async function run() {
  let mounted = createRoot(document.getElementById('root')!);
  const scenarios: string[] = [];
  try {
    mounted.render(<Probe />);
    await waitFor(() => library?.folderChildrenByParent[1]?.length === 2 && !library.loading, 'hook loads');
    const media = library.media, selected = library.selectedFolderId;
    await library.requestFolderChildren(3);
    await waitFor(() => Boolean(library.folderChildNextCursorByParent[3]), 'first page cursor ready');
    await library.loadMoreFolderChildren(3);
    await waitFor(() => library.folderChildrenByParent[3]?.length === 510, 'second page merged');
    await library.revealFolderPath(609);
    check(library.folderChildrenByParent[3].length === 510, 'ancestor hydration deduplicates');
    await waitFor(() => library.expandedFolderIds.has(3), 'ancestor reveal expands paged parent');
    holdPage = true;
    const oldPage = library.requestFolderChildren(3, { force: true });
    await waitFor(() => releasePage !== null, 'old ascending request held');
    holdPage = false;
    await library.updateDirectorySort({ kind: 'folder', id: 3 }, { order: 'name_desc', recursive: false });
    releasePage?.(); releasePage = null; await oldPage;
    await waitFor(() => library.folderChildrenByParent[3]?.[0]?.id === 609, 'late page cannot overwrite descending');
    await waitFor(() => library.folderChildrenByParent[3]?.length === 510, 'descending second page');
    check(library.folderChildrenByParent[3].at(-1)?.id === 100 && new Set(library.folderChildrenByParent[3].map((row) => row.id)).size === 510, 'all descending siblings unique');
    check(library.media === media && library.selectedFolderId === selected, 'directory sort preserves media and selection');
    scenarios.push('paged-order-stale-load-selection');

    await library.requestFolderChildren(3, { force: true });
    await waitFor(() => Boolean(library.folderChildNextCursorByParent[3]), 'stale test first cursor');
    staleCursorOnce = true;
    await library.loadMoreFolderChildren(3);
    await waitFor(() => !library.loadingMoreFolderIds.has(3), 'stale recovery clears loading more');
    await library.loadMoreFolderChildren(3);
    await waitFor(() => library.folderChildrenByParent[3]?.length === 510, 'load next page after cursor recovery');
    await library.requestFolderChildren(2);
    const beforeRecursive = requests.length;
    await library.updateDirectorySort({ kind: 'root', id: 1 }, { order: 'name_desc', recursive: true });
    check(!requests.slice(beforeRecursive).some(({ path }) => path === '/api/folders/2/children'), 'recursive setting does not refetch collapsed descendants');
    await library.updateDirectorySort({ kind: 'folder', id: 2 }, { order: 'name_asc', recursive: false });
    check((await library.getDirectorySort({ kind: 'folder', id: 2 })).effectiveOrder === 'name_asc', 'local directory override');
    check((await library.getDirectorySort({ kind: 'folder', id: 4 })).effectiveOrder === 'name_desc', 'deeper child still inherits older recursive ancestor');
    await library.updateDirectorySort({ kind: 'folder', id: 2 }, null);
    check((await library.getDirectorySort({ kind: 'folder', id: 2 })).effectiveOrder === 'name_desc', 'reset restores inherited state');
    failChildId = 3;
    let reloadFailed = false;
    try { await library.updateDirectorySort({ kind: 'root', id: 1 }, { order: 'name_asc', recursive: true }); } catch { reloadFailed = true; }
    check(reloadFailed && library.folderChildrenByParent[3].length === 510, 'failed branch reload retains full visible window');
    failChildId = null;
    const beforeRetry = requests.length;
    await library.updateDirectorySort({ kind: 'root', id: 1 }, { order: 'name_asc', recursive: true });
    check(requests.slice(beforeRetry).filter(({ path }) => path === '/api/folders/3/children').length === 2, 'no-op retry reloads failed branch and its existing second page');
    await library.updateDirectorySort({ kind: 'root', id: 1 }, null);
    scenarios.push('cursor-recovery-recursion-reload-retry');

    holdRoots = true;
    const pendingRoots = library.refresh();
    await waitFor(() => releaseRoots !== null, 'old roots held');
    await library.updateRootAppearance(1, { icon: 'camera', color: '#123456' });
    check(library.roots[0].reachable === false, 'metadata response does not flash root online');
    holdRoots = false; releaseRoots?.(); releaseRoots = null; await pendingRoots; await delay();
    check(library.roots[0].icon === 'camera' && library.roots[0].reachable === false, 'older roots preserve confirmed appearance and status');
    const oldAppearance = library.updateRootAppearance, oldSort = library.updateDirectorySort;
    holdAppearance = true;
    const save = oldAppearance(1, { icon: 'heart', color: '#abc' });
    await waitFor(() => releaseAppearance !== null, 'old root save held');
    for (const listener of listeners) listener('starting-core'); await delay();
    libraryId = 'second'; revision = 2; rootRecord = { ...rootRecord, icon: 'star' };
    for (const listener of listeners) listener('loading-media');
    await waitFor(() => library.roots[0]?.icon === 'star' && !library.loading, 'second Library loads');
    holdAppearance = false; releaseAppearance?.(); releaseAppearance = null; await save; await delay();
    check(String(library.roots[0].icon) === 'star', 'old mutation cannot repaint current Library');
    const count = requests.length;
    let rejected = 0;
    try { await oldAppearance(1, { icon: 'heart', color: '#abc' }); } catch { rejected++; }
    try { await oldSort({ kind: 'root', id: 1 }, null); } catch { rejected++; }
    check(rejected === 2 && requests.length === count, 'old callbacks reject before any new Library write');
    scenarios.push('root-read-and-library-fences');

    mounted.unmount(); await delay();
    rootRecord = { ...rootRecord, rootFolderId: null, icon: null, color: null };
    mounted = createRoot(document.getElementById('root')!);
    mounted.render(<QueryClientProvider client={queryClient}><OverlayProvider><App /></OverlayProvider></QueryClientProvider>);
    await waitFor(() => !!document.querySelector('[data-root-id="1"]'), 'production App root appears');
    await waitFor(() => document.querySelector('.shell-library-switcher-name')?.textContent === libraryId, 'production App identity settles');
    await delay(150);
    await context('[data-root-id="1"]');
    await button('图标与颜色…');
    await waitFor(() => !!document.querySelector('.library-appearance-editor'), 'actual root menu opens editor without sentinel');
    (document.querySelector('[data-icon="camera"]') as HTMLButtonElement).click();
    await delay();
    await button('应用');
    await waitFor(() => !document.querySelector('.library-appearance-editor'), 'root apply closes');
    check(document.querySelector('[data-root-id="1"] [data-identity-icon="camera"]'), 'custom identity visible on real root');
    check(document.querySelector('[data-root-id="1"] [aria-label="资源库离线"]'), 'offline mark remains separate');
    const offlineLabel = document.querySelector('[data-root-id="1"] .tree-label')!;
    const identityBounds = offlineLabel.querySelector('[data-identity-icon]')!.getBoundingClientRect();
    const statusBounds = offlineLabel.querySelector('[aria-label="资源库离线"]')!.getBoundingClientRect();
    check(Math.abs(identityBounds.top + identityBounds.height / 2 - statusBounds.top - statusBounds.height / 2) < 1,
      'offline status shares the identity row instead of wrapping beneath the label');
    scenarios.push('actual-root-menu-offline-before-scan');

    await context('[data-root-id="1"]'); await button('排序');
    await waitFor(() => !!document.querySelector('#directory-sort-menu [role="menuitemradio"]:not(:disabled)'), 'sort state loaded');
    const sortList = document.querySelector('#directory-sort-menu .overlay-menu-list')!;
    const checkedOrder = sortList.querySelector('[role="menuitemradio"][aria-checked="true"]')!;
    check(getComputedStyle(sortList).display === 'grid' && checkedOrder.getBoundingClientRect().height >= 30,
      'actual sort submenu uses shared stacked context-menu rows');
    check(checkedOrder.getAttribute('data-selected') === 'true' && checkedOrder.querySelector('.overlay-menu-item-icon svg'),
      'effective order has the shared selected fill and visible check');
    failSort = true; await button('名称降序');
    await waitFor(() => !!document.querySelector('#directory-sort-menu [role="alert"]'), 'write failure shown');
    failSort = false; await button('重试');
    await waitFor(() => rules.get('root:1')?.order === 'name_desc', 'retry keeps failed descending draft');
    holdSort = true; await button('递归应用到子文件夹');
    await waitFor(() => releaseSort !== null, 'save held');
    check(document.querySelector<HTMLButtonElement>('#directory-sort-menu [role="menuitemradio"]')?.disabled, 'pending edits disabled');
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await delay();
    check(document.querySelector('#directory-sort-menu'), 'pending menu does not dismiss');
    holdSort = false; releaseSort?.(); releaseSort = null;
    await waitFor(() => rules.get('root:1')?.recursive === true, 'recursive flag persisted'); await delay();
    failSort = true; await button('恢复继承 / 默认');
    await waitFor(() => !!document.querySelector('#directory-sort-menu [role="alert"]'), 'reset failure shown');
    failSort = false; await button('重试');
    await waitFor(() => !rules.has('root:1'), 'failed reset retries DELETE');
    await closeMenus();
    scenarios.push('actual-sort-menu-pending-error-retry-reset');

    rootRecord = { ...rootRecord, rootFolderId: 1 };
    mounted.unmount(); await delay(); mounted = createRoot(document.getElementById('root')!);
    mounted.render(<QueryClientProvider client={queryClient}><OverlayProvider><App /></OverlayProvider></QueryClientProvider>);
    await waitFor(() => !!document.querySelector('[data-folder-id="2"]'), 'ordinary folder appears');
    await context('[data-folder-id="2"]');
    check([...document.querySelectorAll('#context-menu-root button')].some((node) => node.textContent === '图标与颜色…'), 'ordinary folder retains appearance action');
    await button('排序'); await waitFor(() => !!document.querySelector('#directory-sort-menu [role="menuitemradio"]:not(:disabled)'), 'ordinary sort loaded');
    await button('名称降序'); await waitFor(() => rules.get('folder:2')?.order === 'name_desc', 'ordinary independent sort');
    check(!rules.has('root:1'), 'ordinary rule leaves root default');
    scenarios.push('actual-ordinary-folder-menu');
    fixtureWindow.__overlayLifecycleResult = { ok: true, scenarios };
  } catch (error) {
    fixtureWindow.__overlayLifecycleResult = { ok: false, error: `${error instanceof Error ? error.stack : String(error)}\n${JSON.stringify(requests.slice(-12))}\nMenus: ${document.getElementById('directory-sort-menu')?.outerHTML ?? 'absent'}\nRootMenu: ${document.getElementById('context-menu-root')?.textContent ?? 'absent'}\n${focusEvents.slice(-12).join('\n')}` };
  } finally { releaseRoots?.(); releasePage?.(); releaseSort?.(); releaseAppearance?.(); mounted.unmount(); }
}
void run();

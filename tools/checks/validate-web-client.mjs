import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const webSrc = path.join(root, "apps", "web", "src");

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function read(relativePath) {
  // Normalize CRLF to LF so inspectable-source guards behave the same on Windows
  // checkouts and POSIX worktrees.
  return readFileSync(path.join(root, relativePath), "utf8").replace(/\r\n/g, "\n");
}

function readJson(relativePath) {
  return JSON.parse(read(relativePath));
}

function walk(directory) {
  const files = [];
  for (const entry of readdirSync(directory)) {
    const fullPath = path.join(directory, entry);
    const stat = statSync(fullPath);
    if (stat.isDirectory()) {
      files.push(...walk(fullPath));
    } else if (/\.(ts|tsx)$/.test(entry)) {
      files.push(fullPath);
    }
  }
  return files;
}

function sourceMatch(source, pattern) {
  return source.match(pattern)?.[0] ?? "";
}

const useLibraryData = read("apps/web/src/core/useLibraryData.ts");
const refreshCurrentScanViewBlock = sourceMatch(
  useLibraryData,
  /const\s+refreshCurrentScanView\s*=\s*useCallback\([\s\S]*?\n\s*\]\);\n\n\s*const\s+loadLibrary/
);
const loadLibraryBlock = sourceMatch(
  useLibraryData,
  /const\s+loadLibrary\s*=\s*useCallback\([\s\S]*?\n\s*\]\);/
);
const loadLibraryDependenciesBlock =
  useLibraryData.match(/const\s+loadLibrary\s*=\s*useCallback\([\s\S]*?\},\s*(\[[^\]]*\])\);/)?.[1] ??
  "";
const scanRefreshEffectBlock = sourceMatch(
  useLibraryData,
  /useEffect\(\(\)\s*=>\s*\{\s*if\s*\(\s*!scanActiveRootTask[\s\S]*?\},\s*\[refreshCurrentScanView,\s*scanActiveRootTask,\s*(?:taskPollFailures|scanRefreshFailures)\]\);/
);
const requestThumbnailStatesBlock = sourceMatch(
  useLibraryData,
  /const\s+requestThumbnailStates\s*=\s*useCallback\(\([\s\S]*?\n\s*\]\);/
);
const selectedRootHandlerBlock = sourceMatch(
  useLibraryData,
  /setSelectedRootId:\s*\(rootId:\s*number\)\s*=>\s*\{[\s\S]*?\n\s*\},\n\s*setSelectedFolder:/
);
const selectedFolderHandlerBlock = sourceMatch(
  useLibraryData,
  /setSelectedFolder:\s*\(folder:\s*FolderRecord\)\s*=>\s*\{[\s\S]*?\n\s*\},\n\s*setSelectedMediaId:/
);
const selectLibraryFolderBlock = sourceMatch(
  useLibraryData,
  /const\s+selectLibraryFolder\s*=\s*useCallback\([\s\S]*?\n\s*\]\s*\);\n\n\s*const\s+navigateFolderHistory/
);
const initialLoadEffectBlock = sourceMatch(
  useLibraryData,
  /useEffect\(\(\)\s*=>\s*\{\s*void\s+loadLibrary\(\);\s*\},\s*\[loadLibrary\]\);/
);
const mediaResourcesPath = "apps/web/src/core/mediaResources.ts";
const mediaResources = existsSync(path.join(root, mediaResourcesPath))
  ? read(mediaResourcesPath)
  : "";
const thumbnailObjectUrlCacheModulePath = "apps/web/src/core/thumbnailObjectUrlCache.ts";
const thumbnailObjectUrlCacheModule = existsSync(path.join(root, thumbnailObjectUrlCacheModulePath))
  ? read(thumbnailObjectUrlCacheModulePath)
  : "";
const mediaGrid = read("apps/web/src/features/media-grid/MediaGrid.tsx");
const mediaGridLayoutGeometry = read("apps/web/src/features/media-grid/layoutGeometry.ts");
const main = read("apps/web/src/main.tsx");
const app = read("apps/web/src/app/App.tsx");
const shellTitlebar = read("apps/web/src/app-shell/ShellTopBar.tsx");
const libraryView = read("apps/web/src/features/library/LibraryView.tsx");
const librarySidebar = read("apps/web/src/features/library/LibrarySidebar.tsx");
const subfolderStrip = read("apps/web/src/features/library/SubfolderStrip.tsx");
const settingsView = read("apps/web/src/features/settings/SettingsDialog.tsx");
const recentOpsPanel = read("apps/web/src/features/file-ops/RecentOpsPanel.tsx");
const pluginsView = read("apps/web/src/features/plugins/PluginsView.tsx");
const previewPanelPath = "apps/web/src/features/preview/PreviewPanel.tsx";
const previewPanel = existsSync(path.join(root, previewPanelPath))
  ? read(previewPanelPath)
  : "";
const mediaPreviewPath = "apps/web/src/features/preview/MediaPreview.tsx";
const mediaPreview = existsSync(path.join(root, mediaPreviewPath))
  ? read(mediaPreviewPath)
  : "";
const previewPreferencesPath = "apps/web/src/features/preview/previewPreferences.ts";
const previewPreferences = existsSync(path.join(root, previewPreferencesPath))
  ? read(previewPreferencesPath)
  : "";
const centralPreviewStagePath = "apps/web/src/features/preview/CentralPreviewStage.tsx";
const centralPreviewStage = existsSync(path.join(root, centralPreviewStagePath))
  ? read(centralPreviewStagePath)
  : "";
const shortcutBindingsPath = "apps/web/src/features/shortcuts/shortcutBindings.ts";
const shortcutBindings = existsSync(path.join(root, shortcutBindingsPath))
  ? read(shortcutBindingsPath)
  : "";
const useShortcutsPath = "apps/web/src/features/shortcuts/useShortcuts.ts";
const useShortcuts = existsSync(path.join(root, useShortcutsPath))
  ? read(useShortcutsPath)
  : "";
const desktopAdapterPath = "apps/web/src/core/desktop.ts";
const desktopAdapter = read(desktopAdapterPath);
const packageJson = readJson("package.json");
const webPackageJson = readJson("apps/web/package.json");
const coreClientPackagePath = "packages/core-client/package.json";
const coreClient = read("packages/core-client/src/client.ts");
const coreClientContract = read("packages/core-client/src/generated-contract.ts");
const duplicateCoreContractNames = [
  "Page",
  "RootRecord",
  "FolderRecord",
  "MediaRecord",
  "AcceptedRootResponse",
  "ScanSummary",
  "TaskRecord",
  "ListMediaParams",
  "CoreClientConfig",
  "CoreApiError"
];

if (!existsSync(path.join(root, coreClientPackagePath))) {
  fail("packages/core-client must be a real workspace package");
} else {
  const coreClientPackageJson = readJson(coreClientPackagePath);
  if (coreClientPackageJson.name !== "@megle/core-client") {
    fail("packages/core-client package name must be @megle/core-client");
  }
  if (!coreClientPackageJson.scripts?.check) {
    fail("@megle/core-client must expose a package boundary check");
  }
}

if (!packageJson.workspaces?.includes("packages/core-client")) {
  fail("root workspaces must include packages/core-client");
}

if (webPackageJson.dependencies?.["@megle/core-client"] !== "*") {
  fail("apps/web must depend on @megle/core-client through the root workspace");
}

if (
  !/getThumbnail:\s*\(\s*fileId:\s*number,\s*target:\s*"grid_320"\s*=\s*"grid_320",\s*priority:\s*ThumbnailPriority/.test(
    coreClient
  )
) {
  fail("core-client getThumbnail must expose the target query vocabulary");
}
if (!/getThumbnailBlob:\s*async\s*\([\s\S]*?fileId:\s*number,[\s\S]*?target:\s*"grid_320"/.test(coreClient)) {
  fail("core-client getThumbnailBlob must expose the target query vocabulary");
}
if (!/interface\s+BlobRequestOptions[\s\S]*?signal\?:\s*AbortSignal/.test(coreClient)) {
  fail("core-client blob helpers must expose AbortSignal request options");
}
if (!/interface\s+CoreRequestOptions[\s\S]*?signal\?:\s*AbortSignal/.test(coreClient)) {
  fail("core-client page helpers must expose AbortSignal request options");
}
if (
  !/export\s+type\s+CoreRequestPriority/.test(coreClient) ||
  !/requestPriority\?:\s*CoreRequestPriority/.test(coreClient) ||
  !/MAX_CORE_REQUESTS\s*=\s*12/.test(coreClient) ||
  !/MAX_CORE_INTERACTIVE_REQUESTS/.test(coreClient) ||
  !/activeCoreInteractiveRequests/.test(coreClient) ||
  !/scheduleCoreRequest/.test(coreClient) ||
  !/coreRequestPriorityRank/.test(coreClient)
) {
  fail("core-client must globally schedule Core fetches with reserved navigation/interactive slots");
}
if (
  !/const\s+response\s*=\s*await\s+scheduleCoreRequest\([\s\S]*?fetch\(resolveUrl/.test(coreClient) ||
  !/async\s+function\s+fetchBlob[\s\S]*?scheduleCoreRequest\([\s\S]*?fetch\(resolveUrl/.test(coreClient)
) {
  fail("core-client JSON and blob helpers must both route through the global request scheduler");
}
if (
  !/listFolderChildren:\s*\(\s*folderId:\s*number,\s*params:\s*ListFolderChildrenParams\s*=\s*\{\},\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<Page<FolderRecord>>\([\s\S]*?\{[\s\S]*?signal:\s*options\.signal[\s\S]*?\}\s*\)/.test(
    coreClient
  )
) {
  fail("core-client listFolderChildren must forward AbortSignal to fetch");
}
if (
  !/listMedia:\s*\(\s*params:\s*ListMediaParams\s*=\s*\{\},\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<Page<MediaRecord>>\([\s\S]*?\{[\s\S]*?signal:\s*options\.signal[\s\S]*?\}\s*\)/.test(
    coreClient
  )
) {
  fail("core-client listMedia must forward AbortSignal to fetch");
}
if (
  !/searchMedia:\s*\(\s*params:\s*SearchParams\s*=\s*\{\},\s*options:\s*CoreRequestOptions\s*=\s*\{\}\s*\)\s*=>[\s\S]*?request<Page<MediaRecord>>\([\s\S]*?\{[\s\S]*?signal:\s*options\.signal[\s\S]*?\}\s*\)/.test(
    coreClient
  )
) {
  fail("core-client searchMedia must forward AbortSignal to fetch");
}
if (!/interface\s+BlobRequestOptions[\s\S]*?version\?:\s*number\s*\|\s*string\s*\|\s*null/.test(coreClient)) {
  fail("core-client thumbnail blob helper must expose a version cache-buster option");
}
if (!/getThumbnailBlob[\s\S]*?query\(\{\s*target,\s*v:\s*options\.version/.test(coreClient)) {
  fail("core-client getThumbnailBlob must put the version cache-buster in the HTTP request URL");
}
if (!/getPreviewBlob[\s\S]*?query\(\{\s*v:\s*options\.version/.test(coreClient)) {
  fail("core-client getPreviewBlob must put the version cache-buster in the HTTP request URL");
}
if (/thumbnail\$\{query\(\{\s*profile/.test(coreClient) || /thumbnail\/blob\$\{query\(\{\s*profile/.test(coreClient)) {
  fail("core-client thumbnail helpers must not serialize the retired profile query");
}
if (
  !/scheduleForegroundResourceRequest\(\s*resourcePriority,\s*\(signal\)\s*=>\s*fetchThumbnailBlob\(fileId,\s*versionKey,\s*signal,\s*coreRequestPriority\),\s*controller\s*\)/.test(
    mediaResources
  )
) {
  fail("requestThumbnailBlob must forward shared AbortController and promoted priority to the thumbnail blob fetch");
}
if (
  !/scheduleForegroundResourceRequest\(\s*resourcePriority,\s*\(signal\)\s*=>\s*fetchOriginalPreviewBlob\(mediaRecord,\s*mediaSignature,\s*signal,\s*coreRequestPriority\),\s*controller\s*\)/.test(
    mediaResources
  )
) {
  fail("requestOriginalPreviewBlob must forward shared AbortController and promoted priority to the preview blob fetch");
}
if (
  !/scheduleForegroundResourceRequest\(\s*schedulerPriority,\s*\(signal\)\s*=>\s*fetchThumbnailState\(mediaRecord,\s*priority,\s*signal\),\s*controller\s*\)/.test(
    mediaResources
  )
) {
  fail("requestThumbnailState must forward shared AbortController to the thumbnail state fetch");
}
if (
  !/MAX_FOREGROUND_RESOURCE_REQUESTS/.test(mediaResources) ||
  !/MAX_FOREGROUND_RESOURCE_REQUESTS\s*=\s*12/.test(mediaResources) ||
  !/MAX_INTERACTIVE_FOREGROUND_RESOURCE_REQUESTS/.test(mediaResources) ||
  !/MAX_AHEAD_FOREGROUND_RESOURCE_REQUESTS/.test(mediaResources) ||
  !/activeAheadForegroundResourceRequests/.test(mediaResources) ||
  !/activeInteractiveForegroundResourceRequests/.test(mediaResources) ||
  !/scheduleForegroundResourceRequest/.test(mediaResources) ||
  !/thumbnailRequestPriorityRank/.test(mediaResources)
) {
  fail("mediaResources must reserve selected/preview slots and cap ahead prefetch so visible resources are not starved");
}
if (
  !/resourcePriority\?:\s*ForegroundResourcePriority/.test(mediaResources) ||
  !/requestPriority\?:\s*CoreRequestPriority/.test(mediaResources) ||
  !/foregroundResourceCoreRequestPriority/.test(mediaResources) ||
  !/getThumbnailBlob\(fileId,\s*GRID_THUMBNAIL_TARGET,[\s\S]*?requestPriority:\s*coreRequestPriority/.test(
    mediaResources
  ) ||
  !/getPreviewBlob\(mediaRecord\.id,[\s\S]*?requestPriority:\s*coreRequestPriority/.test(
    mediaResources
  )
) {
  fail("mediaResources must map visible/selected resource priority into core-client request priority");
}
for (const value of ["previewPlaceholder", "previewPlaceholderFormat", "servedBy", "db_blob"]) {
  if (!coreClientContract.includes(value)) {
    fail(`core-client generated contract missing ${value}`);
  }
}

if (!existsSync(path.join(root, desktopAdapterPath))) {
  fail("web desktop bridge access must be isolated in apps/web/src/core/desktop.ts");
}

for (const value of ["notifyShellReady", "notifyDesktopShellReady"]) {
  if (!desktopAdapter.includes(value)) {
    fail(`web desktop bridge helper missing ${value}`);
  }
}

if (
  !/const\s+didNotify\s*=\s*await\s+notifyShellReady\(\)[\s\S]*?if\s*\(!didNotify\)\s*\{\s*desktopShellReadyNotified\s*=\s*false;?\s*\}[\s\S]*?return\s+didNotify/.test(
    desktopAdapter
  )
) {
  fail("notifyDesktopShellReady must reset its latch when notifyShellReady resolves false so later calls can retry");
}

if (!/import\s+\{\s*notifyDesktopShellReady\s*\}\s+from\s+"\.\/core\/desktop";/.test(main)) {
  fail("web entrypoint must import notifyDesktopShellReady for the earliest desktop shell-ready handshake");
}

if (!/void\s+notifyDesktopShellReady\(\);\s*[\s\S]*?ReactDOM\.createRoot/.test(main)) {
  fail("web entrypoint must notify desktop shell readiness before mounting React");
}

if (/import\s+\{\s*App\s*\}\s+from\s+"\.\/app\/App";/.test(main)) {
  fail("web entrypoint must not statically import App before the desktop shell-ready handshake");
}

if (!/void\s+notifyDesktopShellReady\(\);\s*[\s\S]*?import\("\.\/app\/App"\)[\s\S]*?ReactDOM\.createRoot/.test(main)) {
  fail("web entrypoint must load App only after the early desktop shell-ready handshake has been queued");
}

if (!mediaGrid.includes("@tanstack/react-virtual") || !mediaGrid.includes("useVirtualizer")) {
  fail("MediaGrid must use @tanstack/react-virtual for scalable media layout");
}

if (/Loading library/.test(mediaGrid)) {
  fail("MediaGrid tiles must keep stable dimensions without loading text layout shifts");
}
if (!/function\s+isLikelyImageMedia\(/.test(mediaGrid) || /item\.kind\s*===\s*"image"/.test(mediaGrid)) {
  fail("MediaGrid original fallback must use image extensions as well as classified image kind");
}
if (/OriginalFallbackThumbnail[\s\S]*?useEffect\(\(\)\s*=>[\s\S]*?\},\s*\[[^\]]*\bitem\b[^\]]*\]\);/.test(mediaGrid)) {
  fail("OriginalFallbackThumbnail must not restart original-preview fallback requests on MediaRecord object identity churn");
}
if (/function\s+OriginalFallbackThumbnail[\s\S]*?return\s+preloadImageObjectUrl\(objectUrl\);/.test(mediaGrid)) {
  fail("OriginalFallbackThumbnail must mount the original-preview image as soon as the blob URL is available");
}
if (!/function\s+OriginalFallbackThumbnail[\s\S]*?if\s*\(\s*error\s*&&\s*!src\s*\)[\s\S]*?tile-thumb-failed/.test(mediaGrid)) {
  fail("OriginalFallbackThumbnail failed original-preview fallback must resolve to a failed tile, not stay in loading");
}
if (/function\s+ReadyThumbnail[\s\S]*?return\s+preloadImageObjectUrl\(objectUrl\);/.test(mediaGrid)) {
  fail("ReadyThumbnail must mount the generated thumbnail image as soon as the blob URL is available");
}
if (/\bfreshThumbnailStatesByMediaId\b/.test(requestThumbnailStatesBlock.match(/\[[\s\S]*?\]\);$/)?.[0] ?? "")) {
  fail("requestThumbnailStates must stay stable while thumbnail state updates stream in");
}
if (!/\.virtual-grid\s*\{[\s\S]*?overflow-x:\s*hidden;[\s\S]*?overflow-y:\s*auto;/.test(read("apps/web/src/styles.css"))) {
  fail("MediaGrid must disable horizontal scrolling while preserving vertical scrolling");
}
if (/\.library-browser-layout\s*\{[^}]*overflow-y:\s*auto;/.test(read("apps/web/src/styles.css"))) {
  fail("Library browser sections must share the MediaGrid scroll container instead of adding a parent scroller");
}
if (/leadingContent\??/.test(mediaGrid) || /leadingContentRef/.test(mediaGrid) || /contentOffset/.test(mediaGrid)) {
  fail("MediaGrid must not keep subfolders as static measured leading content; folder cards must be virtualized with media rows");
}
if (
  !/folderSection\?/.test(mediaGrid) ||
  !/buildFolderCoverLayout/.test(mediaGridLayoutGeometry) ||
  !/renderFolder/.test(mediaGrid)
) {
  fail("MediaGrid must virtualize the subfolder section as responsive rows in the same scroll model as media");
}
if (!/previousMediaOffsetRef/.test(mediaGrid) || !/mediaOffsetDelta/.test(mediaGrid)) {
  fail("MediaGrid must compensate scrollTop when late-loading subfolder sections change the media offset");
}
if (/leadingContent=/.test(libraryView) || /<SubfolderStrip[\s\S]*?folders=\{/.test(libraryView)) {
  fail("LibraryCenterPane must pass folder rows into MediaGrid instead of rendering a static subfolder strip");
}
if (/folders\.map/.test(subfolderStrip)) {
  fail("SubfolderStrip must not render every folder card eagerly; folder cards belong to MediaGrid virtualization");
}
if (/const\s+PAGE_LIMIT\s*=\s*200;/.test(useLibraryData)) {
  fail("Library media loading must not keep the old 200-item page cap");
}
if (/autoloadRemainingMediaPages/.test(useLibraryData) || /AUTOLOAD_MEDIA_PAGE_LIMIT/.test(useLibraryData)) {
  fail("Library data loading must not eagerly load every media metadata page; use viewport-indexed windows instead");
}
if (!/mediaTotalCount/.test(useLibraryData) || !/loadedMediaRanges/.test(useLibraryData)) {
  fail("Library data loading must track total media count and loaded media index ranges for million-item folders");
}
if (
  !/mediaPageControllersRef\s*=\s*useRef<Map<string,\s*AbortController>>/.test(useLibraryData) ||
  !/abortStaleMediaPageRequests/.test(useLibraryData) ||
  !/abortAllControllers\(mediaPageControllersRef\.current\)/.test(useLibraryData)
) {
  fail("useLibraryData must abort stale media page requests so the current viewport can preempt old windows");
}
if (
  !/thumbnailStateControllersRef\s*=\s*useRef<Map<ThumbnailRequestPriority,\s*AbortController>>/.test(
    useLibraryData
  ) ||
  !/abortThumbnailStateControllersForPriority/.test(useLibraryData) ||
  !/requestThumbnailState\(mediaRecord,\s*priority,\s*\{\s*signal:\s*controller\.signal\s*\}\)/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must abort stale thumbnail state requests so old visible/ahead scopes cannot starve the current viewport");
}
if (
  !/client\.(?:searchMedia|listMedia)\([\s\S]*?\{\s*signal:\s*scope\.signal\s*\}/.test(
    useLibraryData
  ) ||
  !/if\s*\(\s*isAbortError\(cause\)\s*\)\s*\{\s*return;?\s*\}/.test(useLibraryData)
) {
  fail("useLibraryData media page loading must pass AbortSignal and ignore aborts without showing errors");
}
if (
  !/folderChildControllersRef\s*=\s*useRef<Map<string,\s*AbortController>>/.test(
    useLibraryData
  ) ||
  !/folderDescendantControllersRef\s*=\s*useRef<Map<number,\s*AbortController>>/.test(
    useLibraryData
  ) ||
  !/abortStaleFolderRequests/.test(useLibraryData)
) {
  fail("useLibraryData must abort stale folder child/descendant requests during high-priority navigation");
}
if (
  !/folderChildrenByParentRef\s*=\s*useRef\(folderChildrenByParent\)/.test(useLibraryData) ||
  !/folderChildInitialRequestsRef\s*=\s*useRef<Map<number,\s*Promise<FolderRecord\[\]>>>/.test(
    useLibraryData
  ) ||
  !/const\s+cachedChildren\s*=\s*folderChildrenByParentRef\.current\[folderId\]/.test(
    useLibraryData
  ) ||
  !/const\s+existingRequest\s*=\s*folderChildInitialRequestsRef\.current\.get\(folderId\)/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must dedupe initial folder child probes so subfolder browsing cannot exhaust browser fetch resources");
}
if (
  !/loadTasksRequestRef\s*=\s*useRef<Promise<TaskRecord\[\]>\s*\|\s*null>/.test(
    useLibraryData
  ) ||
  !/if\s*\(\s*loadTasksRequestRef\.current\s*\)\s*\{\s*return\s+loadTasksRequestRef\.current;/.test(
    useLibraryData
  )
) {
  fail("useLibraryData task polling must dedupe overlapping /tasks requests during active scans");
}
if (
  !/INTERACTIVE_FOLDER_SCAN_DEBOUNCE_MS/.test(useLibraryData) ||
  !/interactiveFolderScanControllerRef\s*=\s*useRef<AbortController\s*\|\s*null>/.test(
    useLibraryData
  ) ||
  !/enqueueInteractiveFolderScan\(\s*selectedFolderId,\s*\{\s*signal:\s*controller\.signal\s*\}/.test(
    useLibraryData
  )
) {
  fail("useLibraryData interactive folder scans must be debounced and abort stale requests");
}
if (
  !/thumbnailPriorityScopeSyncControllerRef\s*=\s*useRef<AbortController\s*\|\s*null>/.test(
    useLibraryData
  ) ||
  !/syncThumbnailPriorityScope\(input,\s*\{[\s\S]*?requestPriority:\s*"interactive"[\s\S]*?signal:\s*controller\.signal[\s\S]*?\}\)/.test(
    useLibraryData
  )
) {
  fail("useLibraryData thumbnail priority sync must abort obsolete scope requests and use interactive priority");
}
if (
  !/if\s*\(\s*priority\s*===\s*"selected"\s*\|\|\s*priority\s*===\s*"visible"\s*\)\s*\{[\s\S]*?flushThumbnailPriorityScopeSync\(\)/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must flush selected/visible thumbnail scope changes immediately");
}
if (
  !/const\s+uniformWindowLayoutMode\s*=\s*layoutMode\s*===\s*"grid"\s*\|\|\s*layoutMode\s*===\s*"list"/.test(
    mediaGrid
  ) ||
  // The full-total justified layout is O(totalCount); above a cap it must fall
  // back to the uniform windowed grid or the renderer freezes at million scale.
  !/JUSTIFIED_FULL_TOTAL_MAX_ITEMS/.test(mediaGrid) ||
  !/justifiedTooLargeForFullTotal\s*=\s*[\s\S]*?justifiedLayoutMode\s*&&\s*mediaItemCount\s*>\s*JUSTIFIED_FULL_TOTAL_MAX_ITEMS/.test(
    mediaGrid
  ) ||
  !/windowedMedia\s*=\s*[\s\S]*?uniformWindowLayoutMode\s*\|\|\s*justifiedTooLargeForFullTotal/.test(
    mediaGrid
  ) ||
  !/justifiedFullTotal\s*=\s*[\s\S]*?!justifiedTooLargeForFullTotal/.test(mediaGrid)
) {
  fail(
    "MediaGrid must window grid/list (and over-cap justified/masonry) via the uniform mediaWindow, and use the full-total justified layout only at or below JUSTIFIED_FULL_TOTAL_MAX_ITEMS"
  );
}
if (/Load more media/.test(mediaGrid) || /type:\s*"load-more"/.test(mediaGrid)) {
  fail("MediaGrid must expose the full media count through virtual rows instead of a Load more row");
}
if (/Math\.max\(\s*clampedEnd,\s*pageOffset\s*\+\s*INITIAL_MEDIA_PAGE_LIMIT\s*\)/.test(useLibraryData)) {
  fail("useLibraryData requestMediaWindow must request the visible/ahead window, not inflate every viewport request to the page cap");
}
if (!/onRequestMediaWindow/.test(mediaGrid) || !/visibleMediaIndexRange/.test(mediaGrid)) {
  fail("MediaGrid must report visible/ahead media index windows instead of requesting more records only at the end");
}
if (/function\s+renderLoadingState/.test(mediaGrid) || /Loading media/.test(mediaGrid)) {
  fail("MediaGrid refresh must keep the grid shell stable and must not replace it with a Loading media page");
}
if (!/media-grid-refresh-indicator/.test(mediaGrid) || !/aria-busy=\{loading/.test(mediaGrid)) {
  fail("MediaGrid must expose refresh progress through a non-blocking overlay indicator");
}
if (
  !/isFinalInputItem\s*=\s*itemIndex\s*===\s*count\s*-\s*1/.test(mediaGridLayoutGeometry) ||
  !/if\s*\(\s*shouldFlush\s*&&\s*!isFinalInputItem\s*\)\s*\{\s*flushRow\(true\);/.test(mediaGridLayoutGeometry) ||
  /shouldFillRemainingWidth\s*=\s*justify\s*&&\s*entryIndex\s*===\s*rowItems\.length\s*-\s*1/.test(mediaGridLayoutGeometry) ||
  /entryIndex\s*===\s*rowItems\.length\s*-\s*1\s*\?\s*Math\.max\(1,\s*remainingWidth\)/.test(mediaGridLayoutGeometry)
) {
  fail("adaptive media layout must not stretch the final underfilled row across the viewport");
}
if (/function\s+ReadyThumbnail[\s\S]*?setSrc\(null\);/.test(mediaGrid)) {
  fail("ReadyThumbnail must keep the previous thumbnail visible while a refreshed blob loads");
}
if (
  !/lastRestoredScrollKeyRef/.test(mediaGrid) ||
  !/savedScrollTop\s*===\s*0/.test(mediaGrid) ||
  !/scrollKeyChanged/.test(mediaGrid)
) {
  fail("MediaGrid scroll restoration must not force the same folder back to top while a deep scroll is in progress");
}
if (!/FOLDER_COVER_MEDIA_LIMIT\s*=\s*1/.test(read("apps/web/src/features/library/useFolderCovers.ts"))) {
  fail("Folder covers must request only the first preview image for the vertical cover card");
}
if (!/sort:\s*"name_asc"/.test(read("apps/web/src/features/library/useFolderCovers.ts"))) {
  fail("Folder covers must use the first media item by Name A-Z, not the newest media item");
}
if (
  !/listMedia\(\{[\s\S]*?sort:\s*"name_asc"[\s\S]*?\},\s*\{[\s\S]*?requestPriority:\s*"resource"/.test(
    read("apps/web/src/features/library/useFolderCovers.ts")
  )
) {
  fail("Folder cover metadata requests must not use navigation priority");
}
if (!/onVisibleFolderIndexesChange/.test(mediaGrid) || !/folderCoverPriorityIndexes/.test(libraryView)) {
  fail("Folder cover loading must follow the virtualized visible/ahead folder window instead of the full folder list");
}
if (/visibleSubfolderEntries\.slice\(0,\s*48\)/.test(libraryView)) {
  fail("Folder cover loading must not fall back to the first 48 folders before MediaGrid reports the visible folder window");
}
if (/Promise\.all/.test(read("apps/web/src/features/library/useFolderCovers.ts"))) {
  fail("Folder covers must disclose each cover as it resolves; waiting for a full batch blocks visible cover refresh");
}
if (!/inFlightFolderIds/.test(read("apps/web/src/features/library/useFolderCovers.ts"))) {
  fail("Folder cover loading must track in-flight folder requests to avoid duplicate visible-window fetches");
}
if (
  !/FOLDER_COVER_CONCURRENT_FETCH_LIMIT/.test(read("apps/web/src/features/library/useFolderCovers.ts")) ||
  !/AbortController/.test(read("apps/web/src/features/library/useFolderCovers.ts")) ||
  !/client\.listMedia\([\s\S]*?\{[\s\S]*?signal:\s*controller\.signal[\s\S]*?\}/.test(
    read("apps/web/src/features/library/useFolderCovers.ts")
  )
) {
  fail("Folder cover probes must be bounded and abortable so they cannot starve visible media windows");
}
if (
  !/export function useFolderCovers\(\s*folders:\s*FolderRecord\[\],\s*options:\s*\{\s*disabled\?:\s*boolean;\s*activeScan\?:\s*boolean\s*\}\s*=\s*\{\}\s*\)/.test(
    read("apps/web/src/features/library/useFolderCovers.ts")
  ) ||
  !/if\s*\(\s*disabled\s*\)\s*\{[\s\S]*?abortAllControllers\(inFlightControllersByFolderId\.current\)/.test(
    read("apps/web/src/features/library/useFolderCovers.ts")
  ) ||
  !/useFolderCovers\(folderCoverPriorityFolders,\s*\{\s*disabled:\s*library\.loading,\s*activeScan:\s*library\.scanActive\s*\}\)/.test(
    libraryView
  )
) {
  fail("Folder cover loading must pause only during full-folder loading; load-more must not starve visible folder covers");
}
if (/disabled:\s*library\.loading\s*\|\|\s*library\.loadingMoreMedia/.test(libraryView)) {
  fail("Folder cover loading must continue while media load-more is running");
}
if (/previewPlaceholderDataUrl/.test(subfolderStrip)) {
  fail("Folder covers must not depend on low-resolution placeholder data; they need decoded thumbnail/original preview blobs");
}
if (!/requestThumbnailBlob/.test(subfolderStrip) || !/requestOriginalPreviewBlob/.test(subfolderStrip)) {
  fail("Folder cover cards must load a real image blob, falling back from thumbnail to original preview when needed");
}
if (/requestThumbnailBlob\(\s*coverMediaItem\.id,\s*null/.test(subfolderStrip)) {
  fail("Subfolder covers must not request thumbnail blobs before live ready thumbnail metadata is available");
}
if (
  !/FolderCoverPreview[\s\S]*?requestThumbnailBlob\([\s\S]*?resourcePriority:\s*"visible"[\s\S]*?requestOriginalPreviewBlob\([\s\S]*?resourcePriority:\s*"visible"/.test(
    subfolderStrip
  )
) {
  fail("Folder cover image requests must use visible priority because visible folder covers are part of the current viewport");
}
if (
  !/visibleFolderIndexes/.test(mediaGrid) ||
  !/prioritizedRenderedFolderIndexes/.test(mediaGrid) ||
  !/folderSection\.onVisibleFolderIndexesChange\(prioritizedRenderedFolderIndexes\)/.test(mediaGrid)
) {
  fail("MediaGrid must send actually visible folder indexes before overscan indexes for cover loading priority");
}
if (
  !/thumbnailObjectUrlCache/.test(mediaResources) ||
  !/readCachedThumbnailObjectUrl/.test(mediaGrid) ||
  !/rememberThumbnailObjectUrl/.test(mediaGrid)
) {
  fail("ReadyThumbnail must reuse cached object URLs so remounted virtual tiles do not flash blank");
}
if (/function\s+ReadyPreviewMedia[\s\S]*?setSrc\(null\);/.test(mediaPreview)) {
  fail("ReadyPreviewMedia must keep the previous preview visible while a new blob loads");
}
if (!/useLayoutEffect/.test(centralPreviewStage) || !/fitLongEdgeScaleForMedia/.test(centralPreviewStage)) {
  fail("Central preview must compute fit scale from media dimensions before the first preview paint to avoid size flicker");
}
if (!/preloadImageObjectUrl/.test(mediaGrid) || !/preloadImageObjectUrl[\s\S]*?then\(\(\)\s*=>\s*\{[\s\S]*?setSrc\((?:objectUrl|nextObjectUrl)\)/.test(mediaGrid)) {
  fail("ReadyThumbnail must decode refreshed image blobs before swapping the visible thumbnail src");
}
if (
  !/requestOriginalPreviewBlob/.test(mediaGrid) ||
  !/allowOriginalFallback/.test(mediaGrid)
) {
  fail("MediaGrid visible thumbnails must use original-preview fallback while grid_320 is still pending");
}
if (!/preloadImageObjectUrl/.test(mediaPreview) || !/setSrc\(nextObjectUrl\)[\s\S]*?preloadImageObjectUrl\(nextObjectUrl\)/.test(mediaPreview)) {
  fail("ReadyPreviewMedia must show the object URL immediately and decode asynchronously for sizing");
}
if (/tile-thumb-loading[\s\S]*?<span>(?:loading|pending|queued)<\/span>/.test(mediaGrid)) {
  fail("Pending thumbnails must not flash loading/pending/queued text while refreshing");
}
if (/className="preview-placeholder pending"[\s\S]{0,160}<span>/.test(mediaPreview)) {
  fail("Pending previews must use stable empty placeholders instead of flashing loading text");
}
if (/thumbnail\?\.state\s*\?\?\s*selectedMedia\.thumbnailState\s*\?\?\s*"pending"/.test(previewPanel)) {
  fail("PreviewPanel must not expose raw pending/queued thumbnail state text during refresh");
}
if (/animation:\s*shimmer/.test(read("apps/web/src/styles.css"))) {
  fail("Refresh placeholders must not use shimmer animations that flicker during global refresh");
}
if (
  /Loading children|loading"\s*:|>\s*loading\s*</.test(librarySidebar) ||
  /正在加载子文件夹/.test(subfolderStrip) ||
  /Loading…|Loading plugins/.test(recentOpsPanel + pluginsView)
) {
  fail("Global refresh surfaces must not replace existing chrome with Loading text");
}
if (!mediaGrid.includes("scrollToIndex")) {
  fail("MediaGrid keyboard navigation must scroll the selected row into view");
}
if (!mediaGrid.includes('role="row"')) {
  fail("MediaGrid role=grid must expose row roles around grid cells");
}
if (
  /const\s+virtualSegments\s*=\s*useMemo/.test(mediaGrid) ||
  /rowIndex\s*<\s*mediaWindow\.rowCount[\s\S]*?segments\.push/.test(mediaGrid)
) {
  fail("MediaGrid must resolve virtual media rows by index instead of materializing every row in huge folders");
}
if (
  !/interface\s+VirtualSectionLayout/.test(mediaGrid) ||
  !/resolveVirtualSegment/.test(mediaGrid) ||
  !/mediaRowStartIndex/.test(mediaGrid)
) {
  fail("MediaGrid must keep lightweight virtual section metadata for million-file folders");
}
if (
  !/onRequestThumbnailStates\(visible(?:Priority)?MediaIds,\s*"visible"\)/.test(mediaGrid) ||
  !/onRequestThumbnailStates\(ahead(?:Priority)?MediaIds,\s*"ahead"\)/.test(mediaGrid)
) {
  fail("MediaGrid must request visible and ahead thumbnail scopes separately");
}
if (
  !/thumbnailStateRequestKeyByPriorityRef/.test(useLibraryData) ||
  !/Promise\.allSettled\(pendingRequests\)/.test(requestThumbnailStatesBlock) ||
  !/activeController[\s\S]*thumbnailStateRequestKeyByPriorityRef\.current\.get\(priority\)\s*===\s*requestKey/.test(
    requestThumbnailStatesBlock
  )
) {
  fail("useLibraryData must not abort and restart identical in-flight thumbnail priority requests on every repoll");
}
if (
  !/VISIBLE_THUMBNAIL_REPOLL_MS\s*=\s*150/.test(mediaGrid) ||
  !/AHEAD_THUMBNAIL_REPOLL_MS\s*=\s*1000/.test(mediaGrid) ||
  !/hasVisiblePending\s*\?\s*VISIBLE_THUMBNAIL_REPOLL_MS\s*:\s*AHEAD_THUMBNAIL_REPOLL_MS/.test(mediaGrid)
) {
  fail("MediaGrid must repoll visible thumbnails faster than ahead thumbnails without promoting ahead noise");
}
if (
  /visiblePriorityMediaIds,\s*\n\s*visiblePriorityMediaKey/.test(mediaGrid) ||
  /aheadPriorityMediaIds,\s*\n\s*aheadPriorityMediaKey/.test(mediaGrid)
) {
  fail("MediaGrid thumbnail request effects must depend on stable scope keys rather than array identity");
}
if (/visibleMediaIds\.slice\(\s*0\s*,/.test(mediaGrid) || /visibleOverflowMediaIds/.test(mediaGrid)) {
  fail("MediaGrid must send every currently visible media id at visible priority instead of demoting visible overflow to ahead");
}
if (
  !/const\s+AHEAD_THUMBNAIL_(?:ROW|VIEWPORT)_COUNT\s*=/.test(mediaGrid) &&
  !/ahead(?:Row|Viewport)Count/.test(mediaGrid)
) {
  fail("MediaGrid must keep ahead-of-viewport thumbnail scope configurable");
}
if (!/const\s+visibleMedia\s*=\s*useMemo/.test(mediaGrid) || !/const\s+aheadMedia\s*=\s*useMemo/.test(mediaGrid)) {
  fail("MediaGrid must derive distinct visible and ahead media scopes");
}
if (
  !/VISIBLE_THUMBNAIL_REPOLL_MS|FOREGROUND_THUMBNAIL_REPOLL_MS/.test(mediaGrid) ||
  /1500/.test(mediaGrid)
) {
  fail("MediaGrid foreground thumbnail repoll must be explicit and much faster than 1500ms");
}

for (const value of ["listRoots", "listFolderChildren", "listMedia", "addRoot", "listTasks", "enqueueScan"]) {
  if (!useLibraryData.includes(value)) {
    fail(`useLibraryData must call ${value}`);
  }
}
if (!/setSelectedFolder:\s*\(folder:\s*FolderRecord\)\s*=>\s*void/.test(useLibraryData)) {
  fail("useLibraryData must select folders by FolderRecord so rootId and folderId stay in sync");
}
if (
  !/selectedFolderInfo:\s*FolderRecord\s*\|\s*null/.test(useLibraryData) ||
  !/setSelectedFolderInfo:\s*\(folder:\s*FolderRecord\s*\|\s*null\)\s*=>\s*void/.test(useLibraryData)
) {
  fail("useLibraryData must keep inspector folder selection separate from the active browsing folder");
}
if (
  !/setSelectedFolderInfo:\s*\(folder:\s*FolderRecord\s*\|\s*null\)\s*=>\s*\{[\s\S]*?setSelectedFolderInfoState\(folder\)[\s\S]*?selectMedia\(null\)/.test(
    useLibraryData
  )
) {
  fail("selecting a folder tile must update the inspector folder and clear selected media without navigating");
}
if (!/enqueueInteractiveFolderScan/.test(useLibraryData)) {
  fail("useLibraryData must call enqueueInteractiveFolderScan for the active folder");
}
if (!/syncThumbnailPriorityScope/.test(coreClient) || !/syncThumbnailPriorityScope/.test(useLibraryData)) {
  fail("thumbnail scope sync must be exposed by core-client and used from useLibraryData");
}
if (
  !/requestThumbnailStates:\s*\(mediaIds:\s*number\[\],\s*priority:\s*ThumbnailRequestPriority\)\s*=>\s*void/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must expose explicit thumbnail priority when requesting states");
}
if (
  !/thumbnailPriorityScopeRef/.test(useLibraryData) ||
  !/scheduleThumbnailPriorityScopeSync/.test(useLibraryData) ||
  !/requestThumbnailStates[\s\S]*scheduleThumbnailPriorityScopeSync\(\s*priority,\s*mediaIds\s*\)/.test(
    useLibraryData
  )
) {
  fail("useLibraryData requestThumbnailStates must coalesce current selected/visible/ahead scope into a thumbnail scope sync call");
}
if (!/requestThumbnailStates\(\[selectedMedia\.id\],\s*"selected"\)/.test(useLibraryData)) {
  fail("useLibraryData must request selected media thumbnails through the selected priority path");
}
if (!/onRequestThumbnailStates\(\[item\.id\],\s*"selected"\)/.test(mediaGrid)) {
  fail("MediaGrid must promote clicked media to selected thumbnail priority immediately");
}
if (
  !/onSelectFolder=\{library\.setSelectedFolderInfo\}/.test(libraryView) ||
  !/onOpenFolder=\{library\.setSelectedFolder\}/.test(libraryView) ||
  !/selected=\{folder\.id === library\.selectedFolderInfo\?\.id\}/.test(libraryView)
) {
  fail("LibraryView subfolder tiles must single-click select for inspector and double-click open for navigation");
}
if (
  !/selectedFolder=\{library\.selectedFolderInfo\}/.test(libraryView) ||
  !/selectedFolderCoverMedia=/.test(libraryView) ||
  !/selectedFolder:\s*FolderRecord\s*\|\s*null/.test(previewPanel)
) {
  fail("right inspector PreviewPanel must render selected folder information when a folder tile is selected");
}
if (
  !/const\s+loadedMediaById\s*=\s*useMemo\(\(\)\s*=>\s*\{[\s\S]*?media\.forEach[\s\S]*?mediaSlots\.forEach[\s\S]*?return\s+next[\s\S]*?\},\s*\[media,\s*mediaSlots\]\)/.test(
    useLibraryData
  ) ||
  !/selectedMediaSnapshotRef/.test(useLibraryData) ||
  !/const\s+mediaById\s*=\s*useMemo\(\(\)\s*=>\s*\{[\s\S]*?new\s+Map\(loadedMediaById\)[\s\S]*?selectedMediaSnapshotRef\.current[\s\S]*?next\.set\(selectedSnapshot\.id,\s*selectedSnapshot\)[\s\S]*?return\s+next[\s\S]*?\},\s*\[loadedMediaById\]\)/.test(
    useLibraryData
  ) ||
  !/liveSelectedMedia/.test(useLibraryData) ||
  !/selectedMediaSnapshotRef\.current\?\.id\s*===\s*selectedMediaId/.test(useLibraryData) ||
  !/const\s+selectedMedia\s*=\s*selectedMediaId\s*===\s*null[\s\S]*?\?\s*null[\s\S]*?:\s*liveSelectedMedia\s*\?\?[\s\S]*?selectedMediaSnapshotRef\.current/.test(
    useLibraryData
  )
) {
  fail("useLibraryData mediaById and selectedMedia must include virtual mediaSlots plus a selected snapshot so preview does not close during transient media window refreshes");
}
if (
  !/useEffect\(\(\)\s*=>\s*\{[\s\S]*?selectedFolderId[\s\S]*?enqueueInteractiveFolderScan\(\s*selectedFolderId,\s*\{\s*signal:\s*controller\.signal\s*\}/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must enqueue interactive folder scan when the current folder becomes active");
}
if (!/setSelectedFolder\(folder\)/.test(librarySidebar)) {
  fail("LibrarySidebar must pass the full FolderRecord when selecting folders");
}
if (/loadAllPages/.test(useLibraryData)) {
  fail("useLibraryData must not exhaust all cursor pages into React state");
}
for (const value of [
  "mediaNextCursor",
  "mediaHasMore",
  "mediaPageGeneration",
  "requestGeneration",
  "loadingMoreMedia",
  "loadMoreMedia",
  "folderChildNextCursorByParent",
  "loadMoreFolderChildren"
]) {
  if (!useLibraryData.includes(value)) {
    fail(`useLibraryData must keep bounded pagination state/action ${value}`);
  }
}
if (!/cursor:\s*cursor\s*\?\?\s*undefined/.test(useLibraryData)) {
  fail("useLibraryData must request one cursor page at a time");
}
if (!/requestGeneration\s*!==\s*mediaPageGeneration\.current/.test(useLibraryData)) {
  fail("loadMoreMedia must discard stale page responses after media context changes");
}
if (!/const\s+loadLibrary\s*=\s*useCallback\(async\s*\(scope\?:\s*LibrarySelectionScope\)\s*=>\s*\{\s*const requestGeneration = \+\+mediaPageGeneration\.current;[\s\S]*?await loadRoots\(scope\)/.test(useLibraryData)) {
  fail("loadLibrary must invalidate media page generation before awaited reload work starts");
}
if (
  !/const\s+loadRoots\s*=\s*useCallback\(async\s*\(scope\?:\s*LibrarySelectionScope\)\s*=>/.test(
    useLibraryData
  ) ||
  !/selectedRootIdRef\.current/.test(useLibraryData) ||
  !/selectedFolderIdRef\.current/.test(useLibraryData)
) {
  fail("useLibraryData loadRoots must resolve selection through explicit scope or selection refs, not captured navigation state");
}
if (
  !/const\s+loadLibrary\s*=\s*useCallback\(async\s*\(scope\?:\s*LibrarySelectionScope\)\s*=>/.test(
    useLibraryData
  ) ||
  !loadLibraryBlock ||
  !loadLibraryDependenciesBlock ||
  /\bselectedRootId\b/.test(loadLibraryDependenciesBlock) ||
  /\bselectedFolderId\b/.test(loadLibraryDependenciesBlock)
) {
  fail("useLibraryData loadLibrary must not capture selectedRootId/selectedFolderId and re-run the full load chain on navigation");
}
if (
  !initialLoadEffectBlock ||
  /selectedRootId/.test(initialLoadEffectBlock) ||
  /selectedFolderId/.test(initialLoadEffectBlock)
) {
  fail("useLibraryData initial load effect must stay isolated from root/folder navigation changes");
}
if (!/SCAN_REFRESH_INTERVAL_MS/.test(useLibraryData)) {
  fail("useLibraryData must define a scan-time current-view refresh interval");
}
if (!/scanActiveRootTask/.test(useLibraryData) || !/loadTasks/.test(useLibraryData)) {
  fail("useLibraryData must track active root scans for the selected root");
}
if (
  !/reloadCurrentMedia\s*=\s*useCallback/.test(useLibraryData) ||
  !/selectedFolderId/.test(useLibraryData) ||
  !/listMedia/.test(useLibraryData)
) {
  fail("useLibraryData must reload current folder media while scanning");
}
if (
  !/refreshCurrentScanView\s*=\s*useCallback[\s\S]*?loadFolderChildren\((?:selectedFolderId|selectionToken\.folderId|selectionToken\.folderId\s*\?\?\s*resolvedFolderId|folderId),\s*\{\s*force:\s*true\s*\}\)[\s\S]*?reloadCurrentMedia\(\{[\s\S]*?folderId:\s*(?:selectedFolderId|selectionToken\.folderId|selectionToken\.folderId\s*\?\?\s*resolvedFolderId|folderId)[\s\S]*?incremental:\s*true/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must incrementally refresh current folder media and force-refresh children during scan");
}
if (!refreshCurrentScanViewBlock) {
  fail("useLibraryData must keep refreshCurrentScanView as an inspectable useCallback");
}
if (!scanRefreshEffectBlock) {
  fail("useLibraryData must keep the root-scan refresh effect inspectable");
}
if (!selectedRootHandlerBlock || !selectedFolderHandlerBlock) {
  fail("useLibraryData must keep root and folder navigation handlers inspectable");
}
if (!selectLibraryFolderBlock) {
  fail("useLibraryData must keep shared folder navigation selection logic inspectable");
}
if (
  !/const\s+\[scanRefreshFailures,\s*setScanRefreshFailures\]\s*=\s*useState\(0\)/.test(useLibraryData) ||
  !/scanRefreshFailures\s*>=\s*3/.test(scanRefreshEffectBlock) ||
  /taskPollFailures\s*>=\s*3/.test(scanRefreshEffectBlock) ||
  !/setScanRefreshFailures\(\(failures\)\s*=>\s*failures\s*\+\s*1\)/.test(scanRefreshEffectBlock)
) {
  fail("useLibraryData scan refresh must keep separate failure accounting so loadTasks success cannot reset refresh backoff");
}
if (
  !/scanRefreshInFlightRef\s*=\s*useRef\(false\)/.test(useLibraryData) ||
  !/if\s*\(\s*scanRefreshInFlightRef\.current\s*\)\s*\{\s*return;?\s*\}/.test(scanRefreshEffectBlock) ||
  !/scanRefreshInFlightRef\.current\s*=\s*true/.test(scanRefreshEffectBlock) ||
  !/finally\s*\(\(\)\s*=>\s*\{[\s\S]*?scanRefreshInFlightRef\.current\s*=\s*false/.test(scanRefreshEffectBlock)
) {
  fail("useLibraryData scan refresh loop must prevent overlapping refreshes with an in-flight guard");
}
if (/expandedFolderIds/.test(refreshCurrentScanViewBlock)) {
  fail("useLibraryData scan refresh must not fan out across expanded folders");
}
if (
  !/reloadCurrentMedia\s*=\s*useCallback[\s\S]*?catch\s*\(\s*cause\s*\)\s*\{[\s\S]*?setError\(errorMessage\(cause\)\);[\s\S]*?throw\s+cause;[\s\S]*?\}/.test(
    useLibraryData
  )
) {
  fail("reloadCurrentMedia must rethrow reload failures so scan refresh backoff can trip");
}
if (
  !/reloadCurrentMedia\(\{[\s\S]*?\}\)\s*\.catch\(\(cause\)\s*=>\s*\{[\s\S]*?setError\(errorMessage\(cause\)\)/.test(
    selectLibraryFolderBlock || selectedRootHandlerBlock
  ) ||
  !/reloadCurrentMedia\(\{[\s\S]*?\}\)\s*\.catch\(\(cause\)\s*=>\s*\{[\s\S]*?setError\(errorMessage\(cause\)\)/.test(
    selectLibraryFolderBlock || selectedFolderHandlerBlock
  )
) {
  fail("useLibraryData navigation-triggered media reloads must catch rethrown failures");
}
if (
  !/const\s+prepareNavigationMediaReload\s*=\s*useCallback\(\(\)\s*=>\s*\{[\s\S]*?scanRefreshSelectionVersionRef\.current\s*\+=\s*1[\s\S]*?setScanRefreshFailures\(0\)/.test(
    useLibraryData
  ) ||
  !/setLoadingMoreMedia\(false\)/.test(useLibraryData) ||
  !/flushThumbnailPriorityScopeSync\(\)/.test(useLibraryData) ||
  !/prepareNavigationMediaReload\(\)/.test(selectLibraryFolderBlock) ||
  !/selectLibraryFolder\(\{\s*rootId,\s*folderId:\s*rootFolderId\s*\}\)/.test(selectedRootHandlerBlock) ||
  !/selectLibraryFolder\(\{\s*rootId:\s*folder\.rootId,\s*folderId:\s*folder\.id\s*\}\)/.test(selectedFolderHandlerBlock)
) {
  fail("useLibraryData navigation changes must clear load-more state, flush thumbnail scope sync, and reset scan refresh failures");
}
if (
  !/scanRefreshActiveRootIdRef\s*=\s*useRef<number\s*\|\s*null>\(null\)/.test(useLibraryData) ||
  !/useEffect\(\(\)\s*=>\s*\{[\s\S]*?if\s*\(\s*!scanActiveRootTask\s*\)\s*\{[\s\S]*?scanRefreshActiveRootIdRef\.current\s*=\s*null[\s\S]*?return;?[\s\S]*?\}[\s\S]*?if\s*\(\s*scanRefreshActiveRootIdRef\.current\s*!==\s*selectedRootId\s*\)\s*\{[\s\S]*?scanRefreshActiveRootIdRef\.current\s*=\s*selectedRootId[\s\S]*?setScanRefreshFailures\(0\)[\s\S]*?\}[\s\S]*?\},\s*\[scanActiveRootTask,\s*selectedRootId\]\);/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must reset scan refresh failures when a new selected-root scan context begins");
}
if (!/setInterval[\s\S]*?refreshCurrentScanView\(\)[\s\S]*?\.catch/.test(scanRefreshEffectBlock)) {
  fail("useLibraryData must run the current-view refresh loop while a root scan is active");
}
if (
  !/type\s+ScanRefreshSelectionToken\s*=/.test(useLibraryData) ||
  !/scanRefreshSelectionVersionRef\s*=\s*useRef\(0\)/.test(useLibraryData) ||
  !/createScanRefreshSelectionToken\s*=\s*useCallback\(\(\)\s*:\s*ScanRefreshSelectionToken\s*\|\s*null/.test(useLibraryData) ||
  !/isCurrentScanRefreshSelection\s*=\s*useCallback\(\s*\(\s*token:\s*ScanRefreshSelectionToken\s*\)/.test(useLibraryData) ||
  !/scanRefreshSelectionToken\?:\s*ScanRefreshSelectionToken/.test(useLibraryData) ||
  !/if\s*\(\s*scanRefreshSelectionToken\s*&&\s*!isCurrentScanRefreshSelection\(scanRefreshSelectionToken\)\s*\)\s*\{\s*return;?\s*\}/.test(useLibraryData) ||
  !/if\s*\(\s*requestGeneration\s*!==\s*mediaPageGeneration\.current\s*\|\|\s*\(\s*scanRefreshSelectionToken\s*&&\s*!isCurrentScanRefreshSelection\(scanRefreshSelectionToken\)\s*\)\s*\)/.test(useLibraryData) ||
  !/const\s+selectionToken\s*=\s*createScanRefreshSelectionToken\(\)/.test(refreshCurrentScanViewBlock) ||
  !/if\s*\(\s*!selectionToken\s*\|\|\s*!isCurrentScanRefreshSelection\(selectionToken\)\s*\)\s*\{\s*return;?\s*\}/.test(refreshCurrentScanViewBlock) ||
  !/reloadCurrentMedia\(\{[\s\S]*?scanRefreshSelectionToken:\s*selectionToken/.test(refreshCurrentScanViewBlock) ||
  !/prepareNavigationMediaReload\(\)[\s\S]*?selectRoot\(entry\.rootId\)/.test(selectLibraryFolderBlock)
) {
  fail("useLibraryData scan refresh must use a selection/version token to discard stale media after root or folder navigation");
}
if (
  !/onRequestMediaWindow/.test(mediaGrid) ||
  !/visibleMediaIndexRange/.test(mediaGrid) ||
  /onRequestMore/.test(mediaGrid)
) {
  fail("MediaGrid must request viewport media windows instead of tail-only incremental pages");
}
if (!existsSync(path.join(root, mediaResourcesPath))) {
  fail("web thumbnail resource helper must live behind apps/web/src/core/mediaResources.ts");
}
if (!/import\s+type\s+{[\s\S]*ThumbnailResponse[\s\S]*}\s+from\s+"@megle\/core-client"/.test(mediaResources)) {
  fail("mediaResources must use the @megle/core-client ThumbnailResponse contract");
}
if (!/createCoreClient\(\)[\s\S]*?getThumbnail\(/.test(mediaResources)) {
  fail("mediaResources must request thumbnail state through @megle/core-client getThumbnail");
}
if (!/inFlightThumbnailRequests/.test(mediaResources) || !/thumbnailResourceCache/.test(mediaResources)) {
  fail("mediaResources must coalesce in-flight thumbnail requests and cache state by media id");
}
if (
  !/type\s+CachedThumbnailEntry/.test(mediaResources) ||
  !/export\s+function\s+mediaContentSignature/.test(mediaResources) ||
  !/mediaRecord\.mtime/.test(mediaResources) ||
  !/mediaRecord\.size/.test(mediaResources) ||
  !/entry\.mediaSignature\s*!==\s*mediaContentSignature\(mediaRecord\)/.test(mediaResources)
) {
  fail("mediaResources cached thumbnails must be tied to a media content signature");
}
const mediaFileContentSignatureBlock = sourceMatch(
  mediaResources,
  /export\s+function\s+mediaFileContentSignature\s*\([\s\S]*?\n\}/
);
if (
  !mediaFileContentSignatureBlock ||
  !/mediaRecord\.id/.test(mediaFileContentSignatureBlock) ||
  !/mediaRecord\.mtime/.test(mediaFileContentSignatureBlock) ||
  !/mediaRecord\.size/.test(mediaFileContentSignatureBlock) ||
  /thumbnailState/.test(mediaFileContentSignatureBlock)
) {
  fail("mediaResources original preview signatures must use stable file identity and exclude thumbnail state");
}
if (
  !/const\s+mediaSignature\s*=\s*mediaFileContentSignature\(mediaRecord\);/.test(
    mediaResources
  ) ||
  !/function\s+originalPreviewRequestKey\(mediaRecord:\s*MediaRecord\):\s*string\s*\{\s*return\s+\[mediaFileContentSignature\(mediaRecord\),\s*"original"\]\.join\(":\"\);\s*\}/.test(
    mediaResources
  )
) {
  fail("mediaResources original preview requests must be keyed by stable file signatures, not thumbnail state signatures");
}
if (
  !/const\s+cacheKey\s*=\s*`original:\$\{mediaFileContentSignature\(item\)\}`/.test(
    mediaGrid
  ) ||
  !/const\s+originalVersionKey\s*=\s*mediaFileContentSignature\(media\);/.test(mediaPreview) ||
  !/`folder-cover:\$\{mediaFileContentSignature\(coverMediaItem\)\}`/.test(subfolderStrip)
) {
  fail("UI original-preview object URL keys must use stable file signatures so thumbnail state polling does not restart image loads");
}
if (
  !/thumbnailStateSignaturesByMediaIdRef/.test(useLibraryData) ||
  !/mediaContentSignature\(mediaRecord\)/.test(useLibraryData) ||
  !/requestedMediaSignature/.test(useLibraryData) ||
  !/currentMediaSignature\s*!==\s*requestedMediaSignature/.test(useLibraryData) ||
  !/filterFreshThumbnailStates\([\s\S]*?thumbnailStateSignaturesByMediaIdRef\.current/.test(useLibraryData)
) {
  fail("useLibraryData must carry media signatures through React thumbnail state");
}
if (
  !/selectedMediaThumbnailRequestKey/.test(useLibraryData) ||
  !/mediaContentSignature\(selectedMedia\)/.test(useLibraryData) ||
  !/requestThumbnailStates\(\[selectedMedia\.id\],\s*"selected"\)/.test(useLibraryData)
) {
  fail("useLibraryData selected thumbnail requests must be keyed by selected media signature");
}
if (
  !/task\.kind === "root_scan"/.test(useLibraryData) ||
  !(
    /interactiveRefreshScope/.test(useLibraryData) &&
    /task\.kind === "interactive_folder_scan"/.test(useLibraryData) &&
    /reloadCurrentMedia\(\{[\s\S]*?folderId:\s*interactiveRefreshScope\.folderId/.test(useLibraryData)
  )
) {
  fail("useLibraryData task success handling must avoid full reload churn for thumbnail success and keep interactive refresh scoped");
}
if (!/MediaRecord/.test(mediaResources) || !/isFreshThumbnailForMediaRecord/.test(mediaResources)) {
  fail("mediaResources must validate cached thumbnail responses against the media record thumbnail summary");
}
if (
  !/explicitMediaThumbnailState/.test(mediaResources) ||
  !/const\s+mediaState\s*=\s*explicitMediaThumbnailState\(mediaRecord\.thumbnailState\);/.test(mediaResources)
) {
  fail("mediaResources must evaluate media-row thumbnail state when deciding whether local thumbnail state is still fresh");
}
if (!/isFreshCachedThumbnailForMediaRecord/.test(mediaResources) || !/isLiveThumbnailResponseForMediaRecord/.test(mediaResources)) {
  fail("mediaResources must separate cached thumbnail freshness from live response acceptance");
}
if (!/isLiveThumbnailResponseForMediaRecord\(mediaRecord,\s*thumbnail\)[\s\S]*?thumbnailResourceCache\.set/.test(mediaResources)) {
  fail("mediaResources must accept a fresh live ready thumbnail response even when the media row still says pending");
}
if (/thumbnailCacheKey/.test(mediaResources)) {
  fail("mediaResources must not treat transitional thumbnailCacheKey as runtime truth");
}
if (!/thumbnailResourceCache\.delete/.test(mediaResources) || !/thumbnail\.target\s*!==\s*GRID_THUMBNAIL_TARGET/.test(mediaResources)) {
  fail("mediaResources must drop stale cached thumbnails that do not match target=grid_320");
}
if (!/readCachedThumbnailStates\(\s*mediaRecords:\s*MediaRecord\[\]\s*\)/.test(mediaResources)) {
  fail("mediaResources must read cached thumbnail state through media records, not bare media ids");
}
if (
  !/const\s+mediaState\s*=\s*explicitMediaThumbnailState\(mediaRecord\.thumbnailState\);[\s\S]*?if\s*\(\s*mediaState\s*===\s*"ready"\s*&&\s*thumbnail\.state\s*!==\s*"ready"\s*\)\s*\{\s*return\s+false;?\s*\}/.test(
    mediaResources
  )
) {
  fail("mediaResources must drop stale local non-ready thumbnail state once the media row itself is already ready");
}
if (
  // Coalesce in-flight thumbnail STATE requests by CONTENT identity only, never
  // priority: the same record polled at visible-then-ahead must share one request
  // (keying priority in fetched the same thing twice and helped exhaust the
  // browser connection pool under scroll).
  !/function thumbnailRequestKey\(mediaRecord: MediaRecord\): string \{[\s\S]*?mediaContentSignature\(mediaRecord\),\s*GRID_THUMBNAIL_TARGET\s*\]\.join/.test(
    mediaResources
  ) ||
  !/inFlightThumbnailRequests\.get\(requestKey\)/.test(mediaResources)
) {
  fail("mediaResources must key in-flight thumbnail coalescing by content identity only (media signature + target), not priority");
}
if (
  /failedThumbnailState\(mediaRecord\.id,\s*cause\)/.test(useLibraryData) ||
  /state === "failed"\s*\|\|/.test(mediaResources)
) {
  fail("transient thumbnail request failures must not synthesize a local failed state that blocks retry");
}
if (
  !/requestThumbnailStates:\s*\(mediaIds:\s*number\[\],\s*priority:\s*ThumbnailRequestPriority\)\s*=>\s*void/.test(
    useLibraryData
  )
) {
  fail("useLibraryData must expose visible-range thumbnail state requests");
}
if (/currentThumbnail\?\.state === "queued"\s*\|\|\s*shouldRequestThumbnailState\(mediaRecord\)/.test(useLibraryData)) {
  fail("useLibraryData must not repeatedly request thumbnail state for already-ready foreground thumbnails");
}
if (!/thumbnailStatesByMediaId:\s*Record<number,\s*ThumbnailResponse>/.test(useLibraryData)) {
  fail("useLibraryData must expose thumbnail state by media id without leaking cache paths");
}
if (!/inFlightMediaPageKeys/.test(useLibraryData) || !/mediaPageRequestKey/.test(useLibraryData)) {
  fail("loadMoreMedia must synchronously coalesce duplicate cursor page requests by root/folder/cursor");
}
if (!/inFlightFolderChildPageKeys/.test(useLibraryData) || !/folderChildPageRequestKey/.test(useLibraryData)) {
  fail("loadMoreFolderChildren must synchronously coalesce duplicate cursor page requests by folder/cursor");
}
if (!/onRequestThumbnailStates/.test(mediaGrid) || !/visibleMediaIds/.test(mediaGrid)) {
  fail("MediaGrid must report visible/near-visible media ids for incremental thumbnail requests");
}
if (
  !/selectedMediaId/.test(mediaGrid) ||
  !/excludeMediaId:\s*selectedMediaId/.test(mediaGrid) ||
  !(
    /priority === "selected"[\s\S]*selectedMediaIdRef\.current/.test(useLibraryData) ||
    /normalizeThumbnailScopeMediaIds[\s\S]*priority === "selected"[\s\S]*selectedMediaId/.test(
      useLibraryData
    )
  )
) {
  fail("selected media must be deduped out of visible/ahead thumbnail polling paths");
}
if (!/visible(?:Priority)?Media(?:Id)?Key/.test(mediaGrid)) {
  fail("MediaGrid immediate thumbnail requests must be keyed by the stable visible media id set");
}
if (!/mediaContentSignature/.test(mediaGrid) || !/visibleMediaSignatureKey/.test(mediaGrid)) {
  fail("MediaGrid immediate thumbnail requests must include visible media content signatures");
}
for (const state of ["pending", "queued", "ready", "failed", "skipped_small"]) {
  if (!mediaGrid.includes(`"${state}"`)) {
    fail(`MediaGrid must render a stable thumbnail state branch for ${state}`);
  }
}
if (!/thumbnailStatesByMediaId\[item\.id\]/.test(mediaGrid)) {
  fail("MediaGrid tiles must receive thumbnail state from the resource map by media id");
}
if (/allowOriginalFallback=\{\s*item\.id === selectedMediaId \|\| visiblePriorityMediaIdSet\.has\(item\.id\)\s*\}/.test(mediaGrid)) {
  fail("Mounted MediaGrid tiles must be allowed to use original fallback; scheduler limits keep overscan from starving visible work");
}
if (!/previewPlaceholder/.test(mediaGrid) || !/previewPlaceholderUrl/.test(mediaGrid)) {
  fail("MediaGrid must render MediaRecord.previewPlaceholder before grid_320 bytes are ready");
}
if (!/previewPlaceholderDataUrl\(item\)/.test(mediaGrid) || /usePreviewPlaceholderUrl/.test(mediaGrid)) {
  fail("MediaGrid previewPlaceholder must be derived synchronously during render, not in an effect");
}
if (!/requestThumbnailBlob\(fileId/.test(mediaGrid) || /createCoreClient/.test(mediaGrid)) {
  fail("MediaGrid must load grid_320 bytes through the shared media resource helper");
}
if (
  !/const\s+canLoadCurrentThumbnailBlob\s*=\s*hasLiveReadyThumbnail\s*\|\|\s*rowState\s*===\s*"ready"/.test(
    mediaGrid
  ) ||
  !/thumbnailUpdatedAt=\{hasLiveReadyThumbnail\s*\?\s*thumbnail\.updatedAt\s*:\s*null\}/.test(mediaGrid)
) {
  fail("MediaGrid must load current grid_320 blobs from media-row ready state while using live updatedAt when available");
}
if (!/hasLiveThumbnailMetadata/.test(mediaGrid) || !/rowState\s*===\s*"ready"/.test(mediaGrid)) {
  fail("MediaGrid must keep requesting state for pending, queued, or ready rows without live thumbnail metadata");
}
if (!/data-preview-placeholder/.test(mediaGrid) || !/data-preview-placeholder/.test(mediaPreview)) {
  fail("Grid and preview placeholder rendering must include data-preview-placeholder markers for smoke tests");
}
if (!previewPanel) {
  fail("PreviewPanel must provide a selected media preview foundation");
}
if (!/PreviewPanel/.test(libraryView) || !/selectedMedia/.test(previewPanel) || !/thumbnail/.test(previewPanel)) {
  fail("LibraryView must render a PreviewPanel with selected media and thumbnail state");
}
if (!mediaResources.includes("getPreviewBlob") || !mediaPreview.includes("requestThumbnailBlob")) {
  fail("MediaPreview must load central previews from original media while keeping inspector previews on shared thumbnail blobs");
}
if (!/requestOriginalPreviewBlob/.test(mediaResources) || !/originalPreviewBlobCache/.test(mediaResources)) {
  fail("mediaResources must cache original preview blobs for center preview reuse");
}
if (!/prefetchOriginalPreview/.test(mediaResources) || !/inFlightOriginalPreviewRequests/.test(mediaResources)) {
  fail("mediaResources must expose neighbor original preview prefetch with in-flight coalescing");
}
if (!/requestOriginalPreviewBlob/.test(mediaPreview) || /getPreviewBlob/.test(mediaPreview)) {
  fail("MediaPreview must load central original media through the shared original preview resource cache");
}
if (!/mediaContentSignature\(media\)/.test(mediaPreview) || !/source="original"[\s\S]*?versionKey=\{originalVersionKey\}/.test(mediaPreview)) {
  fail("MediaPreview original preview must pass a media signature version key");
}
if (!/fetchOriginalPreviewBlob/.test(mediaResources) || !/version:\s*mediaSignature/.test(mediaResources)) {
  fail("mediaResources original preview cache must load Core preview blobs with media signatures");
}
if (
  /return\s+fetchOriginalPreviewBlob\(mediaRecord,\s*mediaSignature,\s*options\.signal\)/.test(
    mediaResources
  ) ||
  !/const\s+request\s*=\s*scheduleForegroundResourceRequest\(\s*resourcePriority,\s*\(signal\)\s*=>\s*fetchOriginalPreviewBlob\(mediaRecord,\s*mediaSignature,\s*signal,\s*coreRequestPriority\),\s*controller\s*\)\.finally/.test(
    mediaResources
  ) ||
  !/return\s+withSharedAbortSignal\(entry,\s*options\.signal\);/.test(mediaResources)
) {
  fail("mediaResources original preview requests must coalesce abortable consumers through shared in-flight fetches");
}
if (
  /return\s+fetchThumbnailBlob\(fileId,\s*versionKey,\s*options\.signal\)/.test(
    mediaResources
  ) ||
  !/const\s+request\s*=\s*scheduleForegroundResourceRequest\(\s*resourcePriority,\s*\(signal\)\s*=>\s*fetchThumbnailBlob\(fileId,\s*versionKey,\s*signal,\s*coreRequestPriority\),\s*controller\s*\)\.finally/.test(
    mediaResources
  ) ||
  !/return\s+withSharedAbortSignal\(entry,\s*options\.signal\);/.test(mediaResources)
) {
  fail("mediaResources thumbnail blob requests must coalesce abortable consumers through shared in-flight fetches");
}
if (
  /return\s+fetchThumbnailState\(mediaRecord,\s*priority,\s*options\.signal\)/.test(
    mediaResources
  ) ||
  !/const\s+schedulerPriority\s*=\s*thumbnailStateSchedulerPriority\(priority\);[\s\S]*?const\s+request\s*=\s*scheduleForegroundResourceRequest\(\s*schedulerPriority,\s*\(signal\)\s*=>\s*fetchThumbnailState\(mediaRecord,\s*priority,\s*signal\),\s*controller\s*\)\.finally/.test(
    mediaResources
  ) ||
  !/return\s+withSharedAbortSignal\(entry,\s*options\.signal\);/.test(
    mediaResources
  )
) {
  fail("mediaResources thumbnail state requests must coalesce abortable consumers through shared in-flight fetches");
}
if (!/thumbnailStateSchedulerPriority/.test(mediaResources)) {
  fail("Visible thumbnail state requests must have scheduler priority over original fallback blobs");
}
if (!/withSharedAbortSignal\(\s*inFlight,\s*options\.signal\s*\)/.test(mediaResources)) {
  fail("mediaResources original preview cache must apply AbortSignal through shared in-flight consumers");
}
if (!/hasLiveReadyThumbnail/.test(mediaPreview) || /thumbnail\?\.state\s*===\s*"ready"\s*\?\s*thumbnail\.fileId/.test(mediaPreview)) {
  fail("MediaPreview must not request thumbnail blobs without live ready metadata and updatedAt");
}
if (
  /rowThumbnailReady/.test(mediaPreview) &&
  (/source\s*===\s*"original"\s*&&\s*\(hasLiveReadyThumbnail\s*\|\|\s*rowThumbnailReady\)/.test(mediaPreview) ||
    /if\s*\(\s*hasLiveReadyThumbnail\s*\|\|\s*rowThumbnailReady\s*\)/.test(mediaPreview))
) {
  fail("MediaPreview must not request thumbnail blobs from media-row ready state without live metadata");
}
if (!/AbortController/.test(mediaPreview) || !/requestOriginalPreviewBlob\(\s*media,\s*\{[\s\S]*?signal:\s*controller\.signal/.test(mediaPreview)) {
  fail("MediaPreview original-media requests must use an abortable shared cache request when central preview switches");
}
if (
  !/const\s+initialNaturalSize\s*=[\s\S]*?source\s*===\s*"original"[\s\S]*?preserveNaturalFrame[\s\S]*?media\.width[\s\S]*?media\.height[\s\S]*?\?\s*\{\s*naturalHeight:\s*media\.height,\s*naturalWidth:\s*media\.width\s*\}/.test(
    mediaPreview
  ) ||
  !/if\s*\(\s*initialNaturalSize\s*\)\s*\{[\s\S]*?onNaturalSize\?\.\(initialNaturalSize\);[\s\S]*?setNaturalFrameStyle\(\{[\s\S]*?initialNaturalSize\.naturalHeight[\s\S]*?initialNaturalSize\.naturalWidth[\s\S]*?\}\);[\s\S]*?\}[\s\S]*?setSrc\(nextObjectUrl\);/.test(
    mediaPreview
  )
) {
  fail("MediaPreview central original switching must publish known dimensions before committing src to avoid scale flicker");
}
if (
  !/shouldUseBufferedSwap/.test(mediaPreview) ||
  !/pendingPreview/.test(mediaPreview) ||
  !/commitBufferedPreview/.test(mediaPreview) ||
  !/preview-image-pending/.test(mediaPreview)
) {
  fail("MediaPreview central original switching must keep the displayed image mounted while a hidden pending image loads");
}
if (
  !/import\s+\{\s*flushSync\s*\}\s+from\s+"react-dom";/.test(mediaPreview) ||
  !/function\s+decodePendingPreviewImage\(/.test(mediaPreview) ||
  !/decodePendingPreviewImage\(image\)[\s\S]*?\.then\(\(\)\s*=>\s*\{[\s\S]*?commitBufferedPreview\(pendingPreview,\s*naturalSize\);/.test(
    mediaPreview
  ) ||
  !/flushSync\(\(\)\s*=>\s*\{[\s\S]*?setNaturalFrameStyle\(naturalFrameStyleForSize\(size\)\);[\s\S]*?setSrc\(preview\.src\);[\s\S]*?onNaturalSize\?\.\(size\);[\s\S]*?\}\);/.test(
    mediaPreview
  )
) {
  fail("MediaPreview central original switching must decode the hidden pending image and atomically commit src, frame, and transform state");
}
if (
  !/const\s+effectiveFrameStyle\s*=\s*shouldUseBufferedSwap\s*\?\s*naturalFrameStyle\s*\?\?\s*frameStyle\s*:\s*frameStyle\s*\?\?\s*naturalFrameStyle;/.test(
    mediaPreview
  )
) {
  fail("MediaPreview buffered central switching must keep the displayed image frame until the pending image commits");
}
if (
  !/DEFAULT_PREVIEW_BUFFER_LIMIT_MB\s*=\s*1200/.test(previewPreferences) ||
  !/previewBufferLimitMb/.test(previewPreferences) ||
  !/readStoredPreviewPreferences/.test(app) ||
  !/storePreviewPreferences/.test(app) ||
  !/configureOriginalPreviewBuffer/.test(app) ||
  !/previewPreferences\.previewBufferLimitMb/.test(app) ||
  !/浏览缓存/.test(settingsView) ||
  !/预览缓冲区/.test(settingsView)
) {
  fail("Preview browsing must expose a persisted 1200MB default original-image buffer preference in Settings");
}
if (
  !/DEFAULT_THUMBNAIL_CACHE_LIMIT_MB\s*=\s*5120/.test(previewPreferences) ||
  !/thumbnailCacheLimitMb/.test(previewPreferences) ||
  !/PREVIEW_PREFERENCE_LIMITS[\s\S]*?thumbnailCacheLimitMb/.test(previewPreferences) ||
  !/configureThumbnailCache/.test(app) ||
  !/previewPreferences\.thumbnailCacheLimitMb/.test(app) ||
  !/缩略图缓存/.test(settingsView) ||
  !/thumbnailObjectUrlCacheBytes/.test(thumbnailObjectUrlCacheModule) ||
  !/configureThumbnailCache/.test(thumbnailObjectUrlCacheModule) ||
  /THUMBNAIL_OBJECT_URL_CACHE_LIMIT\s*=\s*512/.test(thumbnailObjectUrlCacheModule) ||
  !/rememberThumbnailObjectUrl\([\s\S]*?byteSize/.test(thumbnailObjectUrlCacheModule) ||
  !/while\s*\(\s*thumbnailObjectUrlCacheBytes\s*>\s*thumbnailObjectUrlCacheLimitBytes/.test(thumbnailObjectUrlCacheModule)
) {
  fail("Preview browsing must expose a persisted 5120MB thumbnail cache and enforce it with an LRU byte budget");
}
if (
  !/THUMBNAIL_OBJECT_URL_CACHE_MIN_ENTRIES/.test(thumbnailObjectUrlCacheModule) ||
  !/thumbnailObjectUrlCache\.size\s*>\s*THUMBNAIL_OBJECT_URL_CACHE_MIN_ENTRIES/.test(
    thumbnailObjectUrlCacheModule
  )
) {
  fail(
    "Thumbnail object-URL cache must keep a count-based retention floor so out-of-view tiles survive scrolling"
  );
}
if (
  !/persistentThumbnailCacheAutoRefresh:\s*boolean/.test(previewPreferences) ||
  !/persistentThumbnailCacheAutoRefresh:\s*false/.test(previewPreferences) ||
  !/persistentThumbnailCacheAutoRefresh\s*===\s*true/.test(previewPreferences) ||
  !/persistentThumbnailCacheAutoRefresh/.test(app) ||
  !/useLibraryData\(\{[\s\S]*?persistentThumbnailCacheAutoRefresh/.test(app) ||
  !/生成状况/.test(settingsView) ||
  !/仅生成可见缩略图/.test(settingsView) ||
  !/自动后台生成全部缩略图/.test(settingsView) ||
  !/生成当前文件夹/.test(settingsView) ||
  !/生成当前目录树/.test(settingsView) ||
  !/生成全部缩略图/.test(settingsView) ||
  !/重试失败/.test(settingsView) ||
  !/清空持久缓存/.test(settingsView) ||
  !/getThumbnailCacheStats/.test(useLibraryData) ||
  !/enqueueThumbnailCache/.test(useLibraryData) ||
  !/clearThumbnailCache/.test(useLibraryData) ||
  !/refreshThumbnailCacheStats/.test(useLibraryData)
) {
  fail("Persistent thumbnail cache settings must expose stats, bulk actions, and a persisted auto-refresh preference through core-client");
}
if (
  !/thumbnailAutoGenerateAll:\s*boolean/.test(previewPreferences) ||
  !/thumbnailAutoGenerateAll:\s*false/.test(previewPreferences) ||
  !/thumbnailAutoGenerateAll:\s*value\.thumbnailAutoGenerateAll\s*===\s*true/.test(
    previewPreferences
  ) ||
  !/thumbnailAutoGenerateAll/.test(useLibraryData) ||
  !/thumbnailAutoGenerateAll/.test(app)
) {
  fail(
    "Thumbnail generation mode must expose a persisted viewport-only-vs-auto-generate-all preference (default viewport-only) wired through useLibraryData"
  );
}
if (
  !/originalPreviewBlobCacheBytes/.test(mediaResources) ||
  !/configureOriginalPreviewBuffer/.test(mediaResources) ||
  !/blob\.size/.test(mediaResources) ||
  !/deleteOriginalPreviewCacheEntry/.test(mediaResources) ||
  !/while\s*\(\s*originalPreviewBlobCacheBytes\s*>\s*originalPreviewBlobCacheLimitBytes/.test(mediaResources)
) {
  fail("Original preview cache must be an LRU byte-budget buffer instead of a fixed item-count cache");
}
if (
  !/buildPreviewPrefetchWindow/.test(app) ||
  !/selectedIndex\s*\+\s*offset/.test(app) ||
  !/previewBufferLimitMbToBytes/.test(app) ||
  !/prefetchOriginalPreview\(neighbor/.test(app)
) {
  fail("Central preview must prefetch subsequent images up to the configured preview buffer budget");
}
if (/\},\s*\[media,\s*source,\s*versionKey\]\);/.test(mediaPreview)) {
  fail("MediaPreview original-media effect must not reset shared-preview loading for unchanged media object identity churn");
}
if (/useThumbnailFallbackUrl\(\s*thumbnail\?\.state\s*===\s*"ready"/.test(mediaPreview)) {
  fail("MediaPreview must not start the thumbnail fallback hook for thumbnail-primary rendering");
}
if (!/previewPlaceholderUrl/.test(mediaPreview) || !/fallbackThumbnail/.test(mediaPreview)) {
  fail("MediaPreview must show previewPlaceholder and thumbnail fallback while media bytes load");
}
if (
  !/readCachedThumbnailObjectUrl/.test(mediaPreview) ||
  !/thumbnailObjectUrlCacheKey/.test(mediaPreview) ||
  !/useThumbnailFallbackUrl\([\s\S]*?mediaContentSignature\(media\)/.test(mediaPreview)
) {
  fail("MediaPreview must synchronously reuse cached grid thumbnail object URLs before fetching a preview fallback blob");
}
if (!/previewPlaceholderDataUrl\(media\)/.test(mediaPreview) || /usePreviewPlaceholderUrl/.test(mediaPreview)) {
  fail("MediaPreview previewPlaceholder must be available on first render without an effect");
}
if (!mediaPreview.includes('source = "thumbnail"') || !mediaPreview.includes('source === "original"')) {
  fail("MediaPreview must require explicit original-source mode for central preview rendering");
}
if (
  !/preferOriginalWhilePending\??:\s*boolean/.test(mediaPreview) ||
  !/preferOriginalWhilePending\s*=\s*false/.test(mediaPreview)
) {
  fail("MediaPreview must expose an opt-in preferOriginalWhilePending flag for inspector fallback only");
}
if (
  !/preserveNaturalFrame\??:\s*boolean/.test(mediaPreview) ||
  !/const\s+shouldPreserveNaturalFrame\s*=/.test(mediaPreview) ||
  !/source\s*===\s*"original"\s*&&\s*preserveNaturalFrame/.test(mediaPreview) ||
  !/setNaturalFrameStyle\(\{[\s\S]*?naturalSize\.naturalHeight/.test(mediaPreview)
) {
  fail("MediaPreview must gate original natural-frame sizing separately from inspector thumbnail fallback rendering");
}
if (
  !/containedPreviewMediaStyle/.test(mediaPreview) ||
  !/setContainedMediaStyle\(containedPreviewMediaStyle\(naturalSize\)\)/.test(mediaPreview) ||
  !/style=\{containedMediaStyle\}/.test(mediaPreview)
) {
  fail("MediaPreview must apply explicit contained dimensions to fixed-frame inspector images");
}
if (
  !/source\s*===\s*"thumbnail"[\s\S]*preferOriginalWhilePending[\s\S]*media\.kind\s*===\s*"image"[\s\S]*thumbnail\?\.state\s*!==\s*"failed"[\s\S]*thumbnail\?\.state\s*!==\s*"skipped_small"/.test(
    mediaPreview
  ) ||
  !/source="original"[\s\S]*versionKey=\{originalVersionKey\}/.test(mediaPreview)
) {
  fail("MediaPreview thumbnail mode must only use original-preview fallback for pending image inspector previews");
}
if (!centralPreviewStage.includes('source="original"')) {
  fail("CentralPreviewStage must request original media bytes through MediaPreview");
}
if (
  /useLayoutEffect\(\(\)\s*=>\s*\{[\s\S]*?resetTransform\(\);[\s\S]*?\},\s*\[resetTransform,\s*selectedMedia\.id\]\);/.test(
    centralPreviewStage
  ) ||
  /\},\s*\[fitSyncTick,\s*previewReadyTick,\s*selectedMedia\.id,\s*viewMode\]\);/.test(
    centralPreviewStage
  ) ||
  !/fittedMediaIdRef/.test(centralPreviewStage) ||
  !/fittedMediaIdRef\.current\s*=\s*null/.test(centralPreviewStage) ||
  !/const\s+shouldResetTransform\s*=\s*fittedMediaIdRef\.current\s*!==\s*selectedMedia\.id/.test(
    centralPreviewStage
  ) ||
  !/setViewMode\("fit-long-edge"\);[\s\S]*?setScale\(nextScale\);[\s\S]*?setPan\(\{\s*x:\s*0,\s*y:\s*0\s*\}\);/.test(
    centralPreviewStage
  )
) {
  fail("CentralPreviewStage must keep the current image transform stable until the next image has a decoded natural size");
}
if (!/thumbnail=\{thumbnail\}/.test(centralPreviewStage)) {
  fail("CentralPreviewStage must pass thumbnail state only as fallback while original media loads");
}
if (!centralPreviewStage.includes("shouldSkipPreviewPan")) {
  fail("CentralPreviewStage must guard interactive media/control targets before starting preview pan");
}
for (const value of [
  "video",
  "audio",
  "button",
  "input",
  "select",
  "textarea",
  "[controls]",
  "[contenteditable]",
  "a",
  "[data-skip-preview-pan]"
]) {
  if (!centralPreviewStage.includes(value)) {
    fail(`CentralPreviewStage preview pan skip selector missing ${value}`);
  }
}
if (
  !/function handlePointerDown[\s\S]*?shouldSkipPreviewPan[\s\S]*?preventDefault[\s\S]*?setPointerCapture/.test(
    centralPreviewStage
  )
) {
  fail("CentralPreviewStage must skip interactive preview pan targets before preventDefault and pointer capture");
}
if (!/MediaPreview[\s\S]{0,140}thumbnail=\{thumbnail\}/.test(previewPanel)) {
  fail("PreviewPanel must keep using thumbnail state for the right inspector preview");
}
if (!/MediaPreview[\s\S]{0,180}source="thumbnail"[\s\S]{0,180}thumbnail=\{thumbnail\}/.test(previewPanel)) {
  fail("PreviewPanel must use the light thumbnail preview path, not original media");
}
if (!/MediaPreview[\s\S]{0,220}source="thumbnail"[\s\S]{0,220}preferOriginalWhilePending/.test(previewPanel)) {
  fail("PreviewPanel must opt into the inspector-only original fallback while thumbnail state is pending");
}
if (!/MediaPreview[\s\S]{0,320}source="thumbnail"[\s\S]{0,320}preserveNaturalFrame=\{false\}[\s\S]{0,320}preferOriginalWhilePending/.test(previewPanel)) {
  fail("PreviewPanel inspector thumbnail fallback must not let original natural dimensions resize the fixed right preview square");
}
if (
  !/prefetchOriginalPreview/.test(app) ||
  !/previewOpen[\s\S]*selectedMediaIndex[\s\S]*buildPreviewPrefetchWindow[\s\S]*prefetchOriginalPreview/.test(app)
) {
  fail("App must prefetch buffered subsequent original previews when center preview is open");
}
if (
  !/const\s+controller\s*=\s*new\s+AbortController\(\)/.test(app) ||
  !/prefetchOriginalPreview\(neighbor,\s*\{\s*signal:\s*controller\.signal\s*\}\)/.test(app) ||
  !/return\s+\(\)\s*=>\s+controller\.abort\(\)/.test(app)
) {
  fail("App original-preview neighbor prefetch must abort stale preview-adjacent requests");
}
if (!/orderedPreviewMedia/.test(app) || /library\.media\[selectedMediaIndex [+-] offset\]/.test(app)) {
  fail("App preview navigation must use the current sorted/windowed mediaSlots order instead of append-loaded library.media order");
}
if (
  !/previewNavigationLockedRef/.test(app) ||
  !/previewDisplayedMediaId/.test(app) ||
  !/const\s+previewNavigationReady\s*=[\s\S]*?previewDisplayedMediaId\s*===\s*library\.selectedMediaId/.test(app) ||
  !/function\s+startPreviewNavigation/.test(app) ||
  !/previewNavigationLockedRef\.current\s*=\s*true[\s\S]*?library\.setSelectedMediaId/.test(app) ||
  !/onPreviewMediaSettled=\{handlePreviewMediaSettled\}/.test(app)
) {
  fail("App preview navigation must lock wheel/key/button next/previous until the newly selected preview media has displayed");
}
if (
  !/onPreviewNext\??:\s*\(\)\s*=>\s*void/.test(useShortcuts) ||
  !/onPreviewPrevious\??:\s*\(\)\s*=>\s*void/.test(useShortcuts) ||
  /selectPreviewNeighbor\(library,\s*[-\d]+\)/.test(useShortcuts) ||
  !/onPreviewNext\?\.\(\)/.test(useShortcuts) ||
  !/onPreviewPrevious\?\.\(\)/.test(useShortcuts)
) {
  fail("Global preview shortcuts must route through App preview navigation locking instead of selecting neighbors directly");
}
if (
  !/MAX_PREVIEW_PREFETCH_CANDIDATES/.test(app) ||
  !/previewBufferLimitMbToBytes/.test(app) ||
  !/buildPreviewPrefetchWindow[\s\S]*selectedIndex\s*\+\s*offset/.test(app)
) {
  fail("App must keep center original preview prefetch bounded by buffer budget and max candidate count");
}
if (!shortcutBindings.includes("toggleSidebars") || !shortcutBindings.includes('defaultBinding: "Tab"')) {
  fail("shortcut bindings must expose editable Tab binding for toggling both sidebars");
}
if (!useShortcuts.includes("onToggleSidebars") || !useShortcuts.includes('"toggleSidebars"')) {
  fail("global shortcut handler must invoke onToggleSidebars from the editable shortcut binding");
}
if (/selectPreviewNeighbor/.test(useShortcuts) || /library\.media\.findIndex/.test(useShortcuts)) {
  fail("useShortcuts must not implement its own preview ordering because App owns locked preview navigation");
}
if (!app.includes("sidebarsHidden") || !app.includes("onToggleSidebars") || !shellTitlebar.includes("ShellSidebarToggle")) {
  fail("App must wire hidden sidebar state through shell titlebar controls");
}
if (!librarySidebar.includes("loadMoreFolderChildren") || !librarySidebar.includes("Load more")) {
  fail("LibrarySidebar must expose a load-more affordance for paginated folder children");
}
if (!/rescanRoot:\s*\(rootId:\s*number\)\s*=>\s*Promise<void>/.test(useLibraryData)) {
  fail("useLibraryData must expose a typed rescanRoot action");
}
if (!librarySidebar.includes("rescanRoot") || librarySidebar.includes("Rescan is not available from the current Core API")) {
  fail("LibrarySidebar must wire rescan to the supported Core enqueueScan capability");
}
for (const value of ["addingRoot", "scanActive"]) {
  if (!useLibraryData.includes(value)) {
    fail(`useLibraryData must keep separate ${value} state`);
  }
}
if (!/setInterval[\s\S]*?\.catch\(/.test(useLibraryData)) {
  fail("task polling interval must catch listTasks/load failures");
}

if (!packageJson.scripts?.["check:web"]) {
  fail("root package.json missing check:web");
}

for (const filePath of walk(webSrc)) {
  const relative = path.relative(root, filePath).replaceAll("\\", "/");
  const contents = readFileSync(filePath, "utf8");

  if (/\bfetch\s*\(/.test(contents)) {
    fail(`raw fetch is not allowed in web source; use @megle/core-client or document an explicit non-Core exception: ${relative}`);
  }
  if (contents.includes("node:fs") || contents.includes("electron")) {
    fail(`web source must not import filesystem/electron APIs: ${relative}`);
  }
  if (
    contents.includes("window.megleDesktop") &&
    relative !== desktopAdapterPath
  ) {
    fail(`desktop preload bridge access must stay in ${desktopAdapterPath}: ${relative}`);
  }
  if (contents.includes("thumbnailCacheKey")) {
    fail(`web must not consume retired thumbnailCacheKey runtime truth: ${relative}`);
  }

  for (const name of duplicateCoreContractNames) {
    const declarationPattern = new RegExp(`\\b(?:export\\s+)?(?:interface|type|class)\\s+${name}\\b`);
    if (declarationPattern.test(contents)) {
      fail(`web must not declare duplicate Core API contract ${name}: ${relative}`);
    }
  }
}

for (const value of ["@megle/core-client", "createCoreClient"]) {
  const found = walk(webSrc).some((filePath) => readFileSync(filePath, "utf8").includes(value));
  if (!found) {
    fail(`web core boundary must import ${value}`);
  }
}

if (!process.exitCode) {
  console.log("PASS: web client boundaries");
}

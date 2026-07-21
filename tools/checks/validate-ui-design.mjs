import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import ts from "typescript";
import { inspectNativeBrowserWindowOptions } from "./native-browser-window-options.mjs";

const root = process.cwd();

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function read(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath)) {
    fail(`required UI design file missing ${relativePath}`);
    return "";
  }
  return readFileSync(absolutePath, "utf8");
}

function readOptional(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath)) {
    return null;
  }
  return readFileSync(absolutePath, "utf8");
}

const desktopMain = read("apps/desktop/src/main.ts");
const secondaryReveal = read("apps/desktop/src/secondary-window-reveal.ts");
const preload = read("apps/desktop/src/preload.cjs");
const styles = read("apps/web/src/styles.css");
const stylesForChecks = stripCssComments(styles);
const webMain = read("apps/web/src/main.tsx");
const app = read("apps/web/src/app/App.tsx");
const appShell = read("apps/web/src/app-shell/AppShell.tsx");
const secondaryWindowFrame = read("apps/web/src/app-shell/SecondaryWindowFrame.tsx");
const secondaryWindowInterfaceStyle = read("apps/web/src/app-shell/useSecondaryWindowInterfaceStyle.ts");
const overlayScrollbars = readOptional("apps/web/src/app-shell/useOverlayScrollbars.ts") ?? "";
const shellTitlebar = read("apps/web/src/app-shell/ShellTopBar.tsx");
const shellOverlayHost = readOptional("apps/web/src/app-shell/ShellOverlayHost.tsx");
const webIndex = read("apps/web/index.html");
const webFaviconSvg = readOptional("apps/web/public/favicon.svg");
const windowChrome = read("apps/web/src/features/window-chrome/WindowChrome.tsx");
const liquidGlassSurface = read("apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx");
const liquidGlassIndex = read("apps/web/src/design/liquid-glass/index.ts");
const sortMenu = read("apps/web/src/features/library/SortMenu.tsx");
const searchBar = read("apps/web/src/features/library/SearchBar.tsx");
const inspectorMetadata = read("apps/web/src/features/preview/InspectorMetadata.tsx");
const renameDialog = read("apps/web/src/features/file-ops/RenameDialog.tsx");
const moveDialog = read("apps/web/src/features/file-ops/MoveDialog.tsx");
const deleteConfirm = read("apps/web/src/features/file-ops/DeleteConfirm.tsx");
const onboardingHero = read("apps/web/src/features/onboarding/OnboardingHero.tsx");
const libraryView = read("apps/web/src/features/library/LibraryView.tsx");
const librarySidebar = read("apps/web/src/features/library/LibrarySidebar.tsx");
const subfolderStrip = read("apps/web/src/features/library/SubfolderStrip.tsx");
const useLibraryData = read("apps/web/src/core/useLibraryData.ts");
const libraryCenterPane = readOptional("apps/web/src/features/library/LibraryCenterPane.tsx") ?? "";
const libraryInspectorPane = readOptional("apps/web/src/features/library/LibraryInspectorPane.tsx") ?? "";
const pluginsView = read("apps/web/src/features/plugins/PluginsView.tsx");
const pluginsWindow = read("apps/web/src/features/plugins/PluginsWindow.tsx");
const notificationWindow = read("apps/web/src/features/notifications/NotificationWindow.tsx");
const notificationStatusView = read("apps/web/src/features/notifications/NotificationStatusView.tsx");
const notificationHistoryView = read("apps/web/src/features/notifications/NotificationHistoryView.tsx");
const processLogView = read("apps/web/src/features/notifications/ProcessLogView.tsx");
const notificationWindowFixture = read("apps/web/tests/fixtures/notificationWindowVisualFixture.tsx");
const notificationPopover = read("apps/web/src/features/notifications/NotificationPopover.tsx");
const filterMenu = readOptional("apps/web/src/features/library/FilterMenu.tsx");
const mediaGrid = read("apps/web/src/features/media-grid/MediaGrid.tsx");
const mediaResources = read("apps/web/src/core/mediaResources.ts");
const previewPanel = read("apps/web/src/features/preview/PreviewPanel.tsx");
const mediaPreview = read("apps/web/src/features/preview/MediaPreview.tsx");
const centralPreviewStage = readOptional("apps/web/src/features/preview/CentralPreviewStage.tsx");
const shortcutBindings = readOptional("apps/web/src/features/shortcuts/shortcutBindings.ts");
const useShortcuts = read("apps/web/src/features/shortcuts/useShortcuts.ts");
const settingsView = read("apps/web/src/features/settings/settingsPanels.tsx");
const interfaceStyle = read("apps/web/src/features/settings/interfaceStyle.ts");
const contextMenu = read("apps/web/src/features/file-ops/ContextMenu.tsx");
const desktopBridge = read("apps/web/src/core/desktop.ts");
const coreClientContract = read("packages/core-client/src/generated-contract.ts");
const coreClient = read("packages/core-client/src/client.ts");
const titlebarPointerPlane = readOptional("apps/web/src/app-shell/useTitlebarPointerPlane.ts") ?? "";
const libraryFilterSources = libraryView + "\n" + (filterMenu ?? "");
const desktopMainAst = ts.createSourceFile(
  "apps/desktop/src/main.ts",
  desktopMain,
  ts.ScriptTarget.Latest,
  true,
  ts.ScriptKind.TS
);

const retiredPresentationPaths = [
  `apps/web/src/features/tasks/${["Task", "Panel"].join("")}.tsx`,
  `apps/web/src/features/tasks/${["Task", "Center"].join("")}.tsx`,
  `apps/web/src/features/tasks/${["Task", "Overlay"].join("")}.tsx`,
  `apps/web/src/features/file-ops/${["Recent", "Ops", "Panel"].join("")}.tsx`
];
for (const retiredPath of retiredPresentationPaths) {
  if (existsSync(path.join(root, retiredPath))) {
    fail(`legacy Tasks/Recent surface must stay retired: ${retiredPath}`);
  }
}

const retiredPresentationSelector = findRetiredPresentationSelector(styles);
if (retiredPresentationSelector) {
  fail(`legacy Tasks/Recent CSS selector must stay retired: ${retiredPresentationSelector}*`);
}

for (const value of ["previewPlaceholder", "previewPlaceholderFormat", "servedBy", "db_blob"]) {
  if (!coreClientContract.includes(value)) {
    fail(`Core preview pipeline contract must expose ${value}`);
  }
}
if (coreClientContract.includes("cacheKey:")) {
  fail("Core preview pipeline contract must not expose thumbnail disk cache keys");
}
if (!coreClient.includes("target: \"grid_320\"") || coreClient.includes("profile?: \"grid_320\"")) {
  fail("Core client thumbnail helpers must use target=grid_320 vocabulary");
}

const nativeMaterial = inspectNativeBrowserWindowOptions(desktopMain);
if (!nativeMaterial.browserWindowOptionsFound) {
  fail("desktop window must construct BrowserWindow with an inline options object");
}

for (const [condition, value] of [
  [/backgroundMaterial:\s*"acrylic"/.test(desktopMain), 'backgroundMaterial: "acrylic"'],
  [nativeMaterial.nonLayeredHost, "transparent: false"],
  [nativeMaterial.transparentBackgroundColor, 'backgroundColor: "#00000000"'],
  [nativeMaterial.roundedCorners, "roundedCorners: true"],
  [nativeMaterial.frameFalse, "frame: false"]
]) {
  if (!condition) {
    fail(`desktop window must keep a frameless non-layered native acrylic host so DWM can round the window: missing ${value}`);
  }
}

if (nativeMaterial.disablesNativeMaterial) {
  fail('desktop window must not set backgroundMaterial: "none"; Windows blur requires native acrylic');
}

if (/\.setOpacity\s*\(/.test(desktopMain)) {
  fail("secondary opacity reveal must stay centralized in the cancellable native-window reveal primitive");
}

if (/titleBarStyle\s*:\s*"hidden"/.test(desktopMain)) {
  fail('desktop BrowserWindow must not combine titleBarStyle: "hidden" with native acrylic; that Electron Windows path can stay opaque until resize');
}

if (/\.setShape\s*\(/.test(desktopMain)) {
  fail("desktop window must not set a custom window region; Windows DWM cannot apply rounded corners to window regions");
}

if (nativeMaterial.unsafeTopLevelSpreads.length > 0) {
  fail("desktop BrowserWindow options must not use top-level spreads that can override acrylic transparency settings");
}

if (!/show:\s*true\b/.test(desktopMain) || !/skipTaskbar:\s*true\b/.test(desktopMain)) {
  fail("desktop window must start as an offscreen skip-taskbar composition bootstrap window so Windows acrylic is active before user-visible reveal");
}

if (!/ready-to-show/.test(desktopMain)) {
  fail("desktop window must wait for ready-to-show before moving the composition-primed window onscreen");
}

for (const value of [
  "WINDOW_COMPOSITION_BOOTSTRAP_MARGIN_PX",
  "compositionBootstrapPlacement",
  "prepareWindowCompositionForReveal",
  "restoreWindowFromCompositionBootstrap",
  "primeWindowComposition",
  "windowStatePersistenceEnabled"
]) {
  if (!desktopMain.includes(value)) {
    fail(`desktop startup must use the offscreen Windows acrylic composition bootstrap: missing ${value}`);
  }
}

for (const value of [
  "SHELL_READY_FAILURE_TIMEOUT_MS",
  "armShellReadyFailureFallback",
  "revealMainWindowForLaunchFailure",
  '"did-fail-load"',
  '"render-process-gone"'
]) {
  if (!desktopMain.includes(value)) {
    fail(`desktop startup must surface renderer bootstrap failures instead of staying permanently invisible: missing ${value}`);
  }
}

for (const [source, value, message] of [
  [desktopMain, 'ipcMain.handle("megle:shell-ready"', "desktop main must expose a megle:shell-ready IPC handshake before the window can become visible"],
  [preload, "notifyShellReady", "desktop preload must own the notifyShellReady bridge surface"],
  [desktopBridge, "notifyDesktopShellReady", "desktop bridge helper must expose notifyDesktopShellReady so renderer code stays off window.megleDesktop"],
  [webMain, "notifyDesktopShellReady", "web entrypoint must notify desktop shell readiness before React effects can miss the startup timeout"],
  [app, "notifyDesktopShellReady", "App must notify desktop shell readiness after the shell mounts"]
]) {
  if (!source.includes(value)) {
    fail(message);
  }
}

const shellReadyRevealMatch = desktopMain.match(
  /async function revealMainWindowForShellReady\(window: BrowserWindow\): Promise<boolean> \{([\s\S]*?)\n\}/
);
if (!shellReadyRevealMatch) {
  fail("desktop main must keep revealMainWindowForShellReady inspectable for shell-ready deadlock checks");
} else if (/mainWindowReadyToShow|waitForRendererPaint:\s*true|waitForRendererFrame|executeJavaScript/.test(shellReadyRevealMatch[1])) {
  fail("desktop shell-ready reveal must not wait on renderer readiness or call back into the renderer while the renderer awaits the shell-ready IPC response");
}

if (!/void\s+notifyDesktopShellReady\(\);\s*[\s\S]*?ReactDOM\.createRoot/.test(webMain)) {
  fail("web entrypoint shell-ready notification must run before React mounts");
}

if (/import\s+\{\s*App\s*\}\s+from\s+"\.\/app\/App";/.test(webMain)) {
  fail("web entrypoint must not statically import App before the desktop shell-ready handshake");
}

if (!/void\s+notifyDesktopShellReady\(\);\s*[\s\S]*?import\("\.\/app\/App"\)[\s\S]*?ReactDOM\.createRoot/.test(webMain)) {
  fail("web entrypoint must load App only after the early desktop shell-ready handshake has been queued");
}

assertDesktopRevealOrderingContract();
assertDesktopRevealOrderingRegressionProbe();

if (!/Content-Security-Policy/.test(webIndex) || /unsafe-eval/.test(webIndex)) {
  fail("web index must declare a CSP without unsafe-eval so Electron startup does not emit CSP security warnings");
}

if (!/html,\s*body,\s*#root\s*\{[\s\S]*?background:\s*transparent/.test(webIndex)) {
  fail("web index must inline transparent html/body/#root styles to avoid a gray first-paint backing plate");
}

for (const value of [
  "--glass-canvas",
  "--glass-panel",
  "--glass-control",
  "--glass-elevated",
  "--glass-border",
  "--glass-blur",
  "--interactive-hover",
  "--interactive-active",
  "--glass-readable-surface"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`liquid glass token missing ${value}`);
  }
}

for (const [token, maxAlpha] of [
  ["--glass-canvas", 0.01],
  ["--glass-workbench", 0.01],
  ["--glass-panel", 0.01],
  ["--glass-elevated", 0.01],
  ["--surface-1", 0.08]
]) {
  const value = cssCustomPropertyValue(stylesForChecks, token);
  const alpha = cssRgbaAlpha(value);
  if (alpha === null || alpha > maxAlpha) {
    fail(`${token} must not create a gray backing plate over the desktop acrylic backdrop`);
  }
}

const readableSurfaceAlpha = cssRgbaAlpha(cssCustomPropertyValue(stylesForChecks, "--glass-readable-surface"));
if (
  readableSurfaceAlpha === null ||
  readableSurfaceAlpha < 0.18 ||
  readableSurfaceAlpha > 0.55
) {
  fail("--glass-readable-surface must be a minimal local translucent tint, not a transparent text washout or opaque backing plate");
}

for (const selector of [
  ".plugins-detail-pane",
  ".settings-section",
  ".plugin-card",
  ".grid-empty-search",
  ".grid-empty-folder",
  ".move-tree"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => declarationValueIncludes(block, "var(--glass-readable-surface)"))) {
    fail(`${selector} must use the local readable glass tint instead of relying on a full-window backing plate`);
  }
}
for (const requiredSharedSelector of [
  ".shell-right-actions .titlebar-icon-button,",
  ".shell-right-actions .titlebar-icon-button::after,",
  '.shell-right-actions .titlebar-icon-button[data-glass-active="true"] > .liquid-glass-backdrop'
]) {
  if (!stylesForChecks.includes(requiredSharedSelector)) {
    fail(`notification button must share center titlebar icon styling: missing ${requiredSharedSelector}`);
  }
}
if (!/\.shell-right-actions \.titlebar-icon-button,[\s\S]*?border:\s*0;/.test(stylesForChecks)) {
  fail("notification titlebar button must remove its liquid-glass border");
}

for (const [selector, maxAlpha] of [
  [".liquid-glass-tone-chrome", 0.01],
  [".liquid-glass-tone-panel", 0.01],
  [".liquid-glass.shell-titlebar", 0.01],
  [".liquid-glass.context-menu", 0.01]
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => cssRgbaAlpha(latestDeclarationValue(block, "--glass-fill")) <= maxAlpha)) {
    fail(`${selector} must not add a renderer gray fill over the native desktop backdrop`);
  }
}

const liquidGlassBackdropBlocks = cssBlocksForExactSelector(stylesForChecks, ".liquid-glass-backdrop");
if (
  liquidGlassBackdropBlocks.some((block) =>
    /linear-gradient\(\s*145deg|linear-gradient\(\s*135deg/.test(block)
  )
) {
  fail("desktop workbench surfaces must not paint a top-left to bottom-right decorative layout gradient");
}

for (const selector of [".plugins-body", ".settings-body", ".onboarding-hero"]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (blocks.some((block) => hasNonTransparentBackground(block))) {
    fail(`${selector} must not paint a bottom-layer gray background over native desktop acrylic`);
  }
}

for (const value of [
  ":where(button, [role=\"button\"], input, select, textarea)",
  ":where(button, [role=\"button\"]):hover:not(:disabled)",
  ":where(button, [role=\"button\"]):active:not(:disabled)",
  ":where(button, [role=\"button\"], input, select, textarea):focus-visible"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`global interactive control state missing ${value}`);
  }
}

for (const value of [
  ".shell-titlebar,",
  ".library-sidebar,",
  ".toolbar,",
  ".inspector-panel",
  "background: transparent"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`glass control layer styling missing ${value}`);
  }
}

for (const selector of [
  ".shell-titlebar",
  ".library-sidebar",
  ".toolbar",
  ".inspector-panel"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) => /background:\s*transparent/.test(block)
    )
  ) {
    fail(`glass control layer ${selector} must stay transparent without painting a root background plate`);
  }
  if (
    blocks.some(
      (block) =>
        hasNonNoneDeclaration(block, "backdrop-filter") ||
        hasNonNoneDeclaration(block, "-webkit-backdrop-filter")
    )
  ) {
    fail(`glass control layer ${selector} must not apply backdrop-filter on the outer container`);
  }
}

for (const value of [
  ".context-menu",
  ".dialog-panel",
  "backdrop-filter: blur(var(--glass-elevated-blur)) saturate(1.5)"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`floating glass layer styling missing ${value}`);
  }
}

if (!stylesForChecks.includes("-webkit-app-region: no-drag")) {
  fail("frameless window chrome must define no-drag regions for interactive titlebar controls");
}

assertExactSelectorHelperContracts();
assertCssCommentStrippingContracts();
assertRetiredPresentationSelectorContracts();
assertJsxStructuralHelperContracts();
assertReturnedObjectMappingHelperContracts();

const noDragBlocks = cssBlocksForExactSelector(stylesForChecks, ".no-drag");
if (
  !noDragBlocks.some(
    (block) => latestDeclarationValue(block, "-webkit-app-region") === "no-drag"
  )
) {
  fail("interactive titlebar controls must define no-drag regions");
}

for (const selector of [".titlebar-workspace-toolbar"]) {
  if (
    cssBlocksForExactSelector(stylesForChecks, selector).some(
      (block) => latestDeclarationValue(block, "-webkit-app-region") === "no-drag"
    )
  ) {
    fail(`${selector} must stay draggable; only titlebar controls should be no-drag`);
  }
}

for (const value of ["data-no-drag", "WindowChrome", "ShellRightActions", "ShellTitlebarPrimaryActions"]) {
  if (!shellTitlebar.includes(value) && !windowChrome.includes(value) && !liquidGlassSurface.includes(value)) {
    fail(`integrated titlebar no-drag/control contract missing ${value}`);
  }
}

if (!windowChrome.includes("window-chrome-button-close")) {
  fail("web chrome controls must render a distinct close target");
}

if (!shellTitlebar.includes("WindowChrome")) {
  fail("integrated titlebar must render custom window chrome controls");
}

for (const value of [
  "data-titlebar-control",
  "data-titlebar-search"
]) {
  if (
    !appShell.includes(value) &&
    !shellTitlebar.includes(value) &&
    !windowChrome.includes(value) &&
    !searchBar.includes(value)
  ) {
    fail(`titlebar interaction contract missing ${value}`);
  }
}

// Titlebar controls are unified into the single interactive-affordance pointer
// glow (data-liquid-glass="button" is an affordance owner) — no separate
// titlebar pointer plane. These pins assert the controls still exist and stay
// no-drag; the glow itself is the shared affordance system.
for (const [source, value, message] of [
  [shellTitlebar, "notifications", "unified titlebar notification control must exist"],
  [filterMenu ?? "", "library-filter", "library filter trigger must exist"],
  [sortMenu, "library-sort", "library sort trigger must exist"],
  [searchBar, "library-search", "search field must exist"],
  [windowChrome, "window-minimize", "window chrome controls must exist"],
  [stylesForChecks, ".shell-titlebar [data-titlebar-control]", "titlebar control annotations must stay no-drag in CSS"],
  [liquidGlassSurface, '[data-liquid-glass="button"]', "glass buttons must be unified into the interactive-affordance glow system"]
]) {
  if (!source.includes(value)) {
    fail(message);
  }
}

// Native window drag: the titlebar bar is `-webkit-app-region: drag` and its
// interactive children are restored to no-drag, so Windows owns the drag
// (correct cursor offset + Aero Snap + double-click maximize).
const shellTitlebarDragBlocks = cssBlocksForExactSelector(stylesForChecks, ".shell-titlebar");
if (
  !shellTitlebarDragBlocks.some(
    (block) => latestDeclarationValue(block, "-webkit-app-region") === "drag"
  )
) {
  fail("integrated titlebar must be a native OS drag region (-webkit-app-region: drag)");
}

for (const [source, value, message] of [
  [searchBar, "outlineOnly", "search bar must opt into the outline-only liquid-glass mode"],
  [searchBar, "interactiveBackground={false}", "search bar must disable inner local glass halo/background fill"],
  [windowChrome, "data-titlebar-control", "window chrome buttons must participate in the shared titlebar pointer plane"],
  [liquidGlassSurface, "outlineOnly", "liquid glass surface must expose an outline-only mode for the titlebar search rule"],
  [liquidGlassSurface, "interactiveBackground", "liquid glass surface must support disabling local interactive background halo"],
  [stylesForChecks, '[data-glass-outline-only="true"]', "styles must define the outline-only search-field surface contract"],
  [stylesForChecks, '.search-bar[data-titlebar-search="true"]::after', "titlebar search must keep exactly one outer outline rule"],
  [stylesForChecks, '.search-bar[data-titlebar-search="true"] > .liquid-glass-backdrop', "titlebar search must suppress the inner glass backdrop/lens layers"],
  [stylesForChecks, ".search-bar-input:focus-visible", "titlebar search must suppress the inner input focus ring in CSS"]
]) {
  if (!source.includes(value)) {
    fail(message);
  }
}

for (const value of [
  "notificationsOpen",
  "notification-toggle",
  "notification-popover",
  'aria-label="通知"'
]) {
  if (!app.includes(value) && !notificationPopover.includes(value) && !shellTitlebar.includes(value)) {
    fail(`notifications must use one accessible compact titlebar popover, missing ${value}`);
  }
}

if (stylesForChecks.includes('"sidebar workspace tasks"') || stylesForChecks.includes("grid-area: tasks")) {
  fail("tasks must not occupy a fixed app-shell grid column");
}

if (
  !/app\.commandLine\.appendSwitch\(\s*["']enable-features["']\s*,\s*CHROMIUM_OVERLAY_SCROLLBAR_FEATURES\s*\)/.test(desktopMain) ||
  !/CHROMIUM_OVERLAY_SCROLLBAR_FEATURES[\s\S]*OverlayScrollbar/.test(desktopMain)
) {
  fail("desktop shell must enable Chromium overlay scrollbars so scrollbars do not consume layout width");
}

for (const forbiddenFeature of [
  "OverlayScrollbarFlashAfterAnyScrollUpdate",
  "OverlayScrollbarFlashWhenMouseEnter"
]) {
  if (desktopMain.includes(forbiddenFeature)) {
    fail(`desktop shell must not enable timer/flashing scrollbar feature ${forbiddenFeature}`);
  }
}

const globalScrollbarBlocks = cssBlocksForExactSelector(stylesForChecks, "*");
if (
  !globalScrollbarBlocks.some(
    (block) =>
      /scrollbar-width:\s*none/.test(block) &&
      /scrollbar-gutter:\s*auto/.test(block)
  )
) {
  fail("global native scrollbars must be hidden so the custom overlay scrollbar owns visibility");
}

if (/scrollbar-gutter:\s*stable/.test(stylesForChecks)) {
  fail("global scrollbars must not reserve stable gutter layout space");
}

const webkitScrollbarBlocks = cssBlocksForExactSelector(stylesForChecks, "*::-webkit-scrollbar");
if (
  !webkitScrollbarBlocks.some((block) =>
    /width:\s*0/.test(block) &&
    /height:\s*0/.test(block)
  )
) {
  fail("global WebKit native scrollbars must be zero-width and replaced by the custom overlay");
}

if (/--media-scrollbar-lane-size/.test(stylesForChecks)) {
  fail("media grid scrollbar must not use a layout-reserving scrollbar lane variable");
}

if (
  !app.includes("useOverlayScrollbars") ||
  !/useOverlayScrollbars\(\)/.test(app)
) {
  fail("App must install the global custom overlay scrollbar controller");
}

for (const value of [
  "pointerdown",
  "pointerup",
  "document.addEventListener(\"scroll\", handleScroll, true)",
  "data-scrollbar-visible",
  "data-scrollbar-area-hover",
  "data-scrollbar-dragging",
  "--scrollbar-edge-inset",
  "readOverlayScrollbarInset",
  "SCROLLBAR_SCROLL_IDLE_MS",
  "scrollActiveUntil",
  "tickScrollActivity",
  "MutationObserver",
  "getComputedStyle(element)",
  "elementStyle.visibility",
  "attributeFilter",
  "requestAnimationFrame"
]) {
  if (!overlayScrollbars.includes(value)) {
    fail(`custom overlay scrollbar controller missing ${value}`);
  }
}

if (/setTimeout|clearTimeout/.test(overlayScrollbars)) {
  fail("custom overlay scrollbar controller must not use timers to hide or show scrollbars");
}

for (const forbidden of [
  "document.elementsFromPoint",
  'document.addEventListener("pointerover"',
  'document.addEventListener("pointerleave"',
  "findScrollableElementAtPoint",
  "isPointInsideElement"
]) {
  if (overlayScrollbars.includes(forbidden)) {
    fail(`custom overlay scrollbar controller must not activate from panel pointer presence: ${forbidden}`);
  }
}

for (const [selector, checks, message] of [
  [
    ".megle-overlay-scrollbar",
    [/position:\s*fixed/, /pointer-events:\s*auto/, /z-index:\s*240/, /opacity:\s*0/],
    "custom overlay scrollbar rail must be fixed, non-layout, pointer-interactive, and hidden by default"
  ],
  [
    ".megle-overlay-scrollbar-thumb",
    [/pointer-events:\s*none/],
    "custom overlay scrollbar thumb must not receive pointer input while hidden"
  ],
  [
    '.megle-overlay-scrollbar[data-scrollbar-visible="true"] .megle-overlay-scrollbar-thumb',
    [/pointer-events:\s*auto/],
    "custom overlay scrollbar thumb must receive pointer input while visible"
  ],
  [
    '.megle-overlay-scrollbar[data-scrollbar-visible="true"] .megle-overlay-scrollbar-thumb',
    [/opacity:\s*0\.5/],
    "custom overlay scrollbar thumb must show at 50 percent opacity while the pointer is in the panel"
  ],
  [
    '.megle-overlay-scrollbar[data-scrollbar-hover="true"] .megle-overlay-scrollbar-thumb',
    [/opacity:\s*0\.75/],
    "custom overlay scrollbar thumb must show at 75 percent opacity while hovered"
  ],
  [
    '.megle-overlay-scrollbar[data-scrollbar-dragging="true"] .megle-overlay-scrollbar-thumb',
    [/opacity:\s*1/],
    "custom overlay scrollbar thumb must show at full opacity only while dragged"
  ]
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => checks.every((check) => check.test(block)))) {
    fail(message);
  }
}

const rootScrollbarBlocks = cssBlocksForExactSelector(stylesForChecks, ":root");
if (
  !rootScrollbarBlocks.some(
    (block) =>
      /--scrollbar-thumb-size:\s*5px/.test(block) &&
      /--scrollbar-edge-inset:\s*5px/.test(block) &&
      /--media-scrollbar-outset:\s*16px/.test(block)
  )
) {
  fail("global scrollbar thumb must be 5 CSS px thick, inset 5 CSS px, and media overhang must match the 16px desktop content inset");
}

const gridSurfaceScrollbarBlocks = cssBlocksForExactSelector(stylesForChecks, ".grid-surface");
if (
  !gridSurfaceScrollbarBlocks.some((block) =>
    /--media-scrollbar-outset:\s*10px/.test(block)
  )
) {
  fail("responsive grid surface must reduce media scrollbar overhang to its 10px content inset");
}

const virtualGridBlocks = cssBlocksForExactSelector(stylesForChecks, ".virtual-grid");
if (
  !virtualGridBlocks.some(
    (block) =>
      /width:\s*calc\(100%\s*\+\s*var\(--media-scrollbar-outset\)\)/.test(block) &&
      /max-width:\s*none/.test(block) &&
      /padding-right:\s*var\(--media-scrollbar-outset\)/.test(block) &&
      /margin-right:\s*calc\(var\(--media-scrollbar-outset\)\s*\*\s*-1\)/.test(block) &&
      /box-sizing:\s*border-box/.test(block)
  )
) {
  fail("media grid scrollbar must overhang outside the content area without reducing layout width");
}

if (
  !mediaGrid.includes("measureVirtualGridViewport") ||
  /entry\.contentRect\.width/.test(mediaGrid) ||
  !/setViewportSize\(measureVirtualGridViewport\(element\)\)/.test(mediaGrid)
) {
  fail("MediaGrid viewport geometry must subtract scrollbar overhang padding before computing tile columns");
}

if (
  !/const\s+uniformWindowLayoutMode\s*=\s*layoutMode\s*===\s*"grid"\s*\|\|\s*layoutMode\s*===\s*"list"/.test(mediaGrid) ||
  !/const\s+justifiedLayoutMode\s*=\s*layoutMode\s*===\s*"adaptive"\s*\|\|\s*layoutMode\s*===\s*"waterfall"/.test(mediaGrid) ||
  // The fixed-row uniform window serves grid/list, plus justified/masonry folders
  // past JUSTIFIED_FULL_TOTAL_MAX_ITEMS (the O(totalCount) full-total layout would
  // freeze the renderer at that scale). At or below the cap, adaptive/waterfall
  // keep their justified full-total geometry.
  !/mediaSlots\s*!==\s*undefined\s*&&\s*\n?\s*\(uniformWindowLayoutMode\s*\|\|\s*justifiedTooLargeForFullTotal\)/.test(mediaGrid)
) {
  fail("MediaGrid must gate the fixed-row uniform window on grid/list plus over-cap justified/masonry (uniformWindowLayoutMode || justifiedTooLargeForFullTotal); at-or-below-cap adaptive/waterfall use their own justified geometry");
}

if (
  !/const\s+justifiedFullTotal\s*=/.test(mediaGrid) ||
  !/justifiedLayoutMode\s*&&/.test(mediaGrid) ||
  !/getItemAt:\s*\(index[^)]*\)\s*=>\s*mediaSlots/.test(mediaGrid) ||
  !/totalCount:\s*mediaItemCount/.test(mediaGrid) ||
  !mediaGrid.includes("MediaTileSkeleton") ||
  !mediaGrid.includes("firstMissingMediaSlotIndex")
) {
  fail("MediaGrid adaptive/waterfall must lay out over the full total (getItemAt/totalCount) with placeholder skeletons for unloaded slots; exact modes still use firstMissingMediaSlotIndex");
}

if (!/--shell-left-width:\s*292px/.test(stylesForChecks) || !/--shell-right-width:\s*270px/.test(stylesForChecks)) {
  fail("app shell must define pixel column width variables that can be overridden by resizer state");
}

for (const value of [
  "shell-column-resizer",
  "handleColumnResizePointerDown",
  "SHELL_LEFT_WIDTH_STORAGE_KEY",
  "SHELL_RIGHT_WIDTH_STORAGE_KEY",
  "--shell-left-width",
  "--shell-right-width"
]) {
  if (!appShell.includes(value) && !stylesForChecks.includes(value)) {
    fail(`resizable left/right shell column contract missing ${value}`);
  }
}

const appShellBlocks = cssBlocksForSelector(stylesForChecks, ".app-shell");
const baseAppShellBlock = appShellBlocks[0] ?? "";
if (
  !/--shell-titlebar-height:\s*50px/.test(baseAppShellBlock) ||
  !/--titlebar-control-size:\s*28px/.test(baseAppShellBlock) ||
  !/--titlebar-window-control-width:\s*32px/.test(baseAppShellBlock) ||
  !/--titlebar-window-control-height:\s*28px/.test(baseAppShellBlock) ||
  !/width:\s*100vw/.test(baseAppShellBlock) ||
  !/height:\s*100vh/.test(baseAppShellBlock) ||
  !/margin:\s*0/.test(baseAppShellBlock) ||
  !/border:\s*0/.test(baseAppShellBlock) ||
  !/border-radius:\s*var\(--radius-window\)/.test(baseAppShellBlock) ||
  !/background:\s*transparent/.test(baseAppShellBlock) ||
  !/box-shadow:\s*none/.test(baseAppShellBlock) ||
  !/overflow:\s*hidden/.test(baseAppShellBlock) ||
  !/clip-path:\s*inset\(0\s+round\s+var\(--radius-window\)\)/.test(baseAppShellBlock)
) {
  fail("app shell must use the approved 50px/28px main-titlebar density and fill the native acrylic host");
}

for (const [selector, checks, message] of [
  [
    ".titlebar-icon-button",
    [/width:\s*var\(--titlebar-control-size\)/, /height:\s*var\(--titlebar-control-size\)/],
    "main titlebar icon buttons must use the shared 28px control token"
  ],
  [
    ".filter-menu-trigger",
    [/width:\s*var\(--titlebar-control-size\)/, /height:\s*var\(--titlebar-control-size\)/],
    "main titlebar filter button must use the shared 28px control token"
  ],
  [
    ".sort-menu-trigger-icon-only",
    [/width:\s*var\(--titlebar-control-size\)/, /height:\s*var\(--titlebar-control-size\)/],
    "main titlebar sort/layout buttons must use the shared 28px control token"
  ],
  [
    ".top-action",
    [/height:\s*var\(--titlebar-control-size\)/],
    "main titlebar task and recent buttons must use the shared 28px control token"
  ],
  [
    ".search-bar",
    [/height:\s*var\(--titlebar-control-size\)/],
    "main titlebar search must align to the shared 28px control height"
  ],
  [
    ".shell-library-switcher-button",
    [/height:\s*var\(--titlebar-control-size\)/],
    "main library switcher must align to the shared 28px control height"
  ],
  [
    ".titlebar-workspace-controls",
    [/min-height:\s*var\(--titlebar-control-size\)/],
    "middle titlebar control group must not retain the old 40px minimum height"
  ],
  [
    ".shell-right-actions",
    [/min-height:\s*var\(--titlebar-control-size\)/],
    "right titlebar action group must not retain the old 40px minimum height"
  ],
  [
    ".window-chrome-button",
    [
      /width:\s*var\(--titlebar-window-control-width\)/,
      /height:\s*var\(--titlebar-window-control-height\)/
    ],
    "main window controls must use the approved 32x28px titlebar tokens"
  ]
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => checks.every((check) => check.test(block)))) {
    fail(message);
  }
}

if (stylesForChecks.includes("--radius-window-host")) {
  fail("window radius must use a single global --radius-window parameter, not separate host/window radius tokens");
}

// The CSS fallback (before interfaceStyle writes the live --radius-window) must
// default to 8px — the native Windows 11 DWM host corner. The value is
// user-adjustable at runtime (窗口圆角); a value larger than the native corner
// curves the CSS content inside it and shows a double corner.
if (!/--radius-window:\s*8px/.test(stylesForChecks)) {
  fail("global window radius fallback must default to 8px to match the native Windows 11 DWM host corner");
}

if (/\.workbench-column::after[\s\S]*?border-color:\s*transparent/.test(stylesForChecks)) {
  fail("workbench columns must keep their own liquid-glass layout strokes; do not hide global pane borders to fix the window edge");
}

if (/\.workbench-column\s*>\s*\.liquid-glass-edge[\s\S]*?display:\s*none/.test(stylesForChecks)) {
  fail("workbench columns must keep pointer-driven liquid-glass edge layers active");
}

// The window edge is the clip-path rounded corner only (html/body/#root/.app-shell
// clip to --radius-window); there is no separate masked edge-frame layer. The old
// .window-edge-frame drew a 1px mask-composite stroke at calc(--radius-window - 1px)
// whose rounded corners could not coincide with the clip on the realtime
// compositor. Guard against a pointer-follow glow regression on any reintroduced
// window frame — the mouse highlight lives only on interactive affordance targets.
if (liquidGlassSurface.includes("updateWindowEdgePointers")) {
  fail("window edge must not have a pointer-follow glow (unified highlight lives on interactive affordance targets only)");
}

if (
  !/grid-template-areas:\s*"workbench-left workbench-center workbench-right"/.test(baseAppShellBlock) ||
  !/grid-template-columns:\s*var\(--shell-left-width\)\s+minmax\(0,\s*1fr\)\s+var\(--shell-right-width\)/.test(baseAppShellBlock) ||
  !/grid-template-rows:\s*minmax\(0,\s*1fr\)/.test(baseAppShellBlock)
) {
  fail("app shell must define three continuous structural workbench columns");
}

for (const value of [
  "titlebarLeft",
  "titlebarCenter",
  "titlebarRight",
  "centerPane",
  "rightPane"
]) {
  if (!appShell.includes(value)) {
    fail(`continuous workbench AppShell render contract missing ${value}`);
  }
}

if (appShell.includes("app-workspace-slot") || app.includes("workspace={renderWorkspace()")) {
  fail("AppShell must not glue titlebars to a single workspace slot; centerPane and rightPane must be composed explicitly");
}

const continuousShellCompositionSource = [
  app,
  appShell,
  libraryCenterPane,
  libraryInspectorPane,
  stylesForChecks
].join("\n");

for (const value of [
  "centerPane",
  "rightPane",
  "workbench-column",
  "workbench-column-body",
  "workbench-column-left",
  "workbench-column-center",
  "workbench-column-right"
]) {
  if (!continuousShellCompositionSource.includes(value)) {
    fail(`continuous shell composition missing ${value}`);
  }
}

for (const forbidden of [
  "margin-top: calc(var(--shell-titlebar-height) * -1)",
  "padding-top: calc(var(--shell-titlebar-height) + 16px)"
]) {
  if (stylesForChecks.includes(forbidden)) {
    fail(`continuous shell composition must remove seam-hiding layout hack ${forbidden}`);
  }
}

for (const selector of [
  ".workbench-column",
  ".workbench-column-left",
  ".workbench-column-center",
  ".workbench-column-right",
  ".shell-titlebar",
  ".library-sidebar",
  ".grid-surface",
  ".inspector-panel"
]) {
  const structuralBlocks = cssBlocksForSelector(stylesForChecks, selector);
  if (structuralBlocks.some((block) => /linear-gradient\(\s*145deg|linear-gradient\(\s*135deg/.test(block))) {
    fail(`structural shell surface ${selector} must not use a decorative diagonal gradient`);
  }
}

for (const value of [
  "shell-titlebar-left",
  "shell-titlebar-center",
  "shell-titlebar-right"
]) {
  if (!appShell.includes(value)) {
    fail(`integrated titlebar AppShell render contract missing ${value}`);
  }
}

for (const [selector, gridArea] of [
  [".workbench-column-left", "workbench-left"],
  [".workbench-column-center", "workbench-center"],
  [".workbench-column-right", "workbench-right"]
]) {
  const blocks = cssBlocksForSelector(stylesForChecks, selector);
  if (blocks.length === 0) {
    fail(`continuous workbench selector missing ${selector}`);
  } else if (!blocks.some((block) => latestDeclarationValue(block, "grid-area") === gridArea)) {
    fail(`continuous workbench selector ${selector} must map to grid-area ${gridArea}`);
  }
}

if (app.includes("<ShellTopBar")) {
  fail("old full-width ShellTopBar JSX must not be rendered");
}

if (/className=["'][^"']*\btopbar\b/.test(app) || /className=["'][^"']*\btopbar\b/.test(shellTitlebar)) {
  fail("old full-width topbar class must not be used");
}

if (!/html,\s*body,\s*#root/.test(stylesForChecks)) {
  fail("web root transparency contract must include #root so no gray page halo can leak around the shell");
}

for (const selector of ["html", "body", "#root"]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        latestBackgroundValue(block) === "transparent" &&
        latestDeclarationValue(block, "width") === "100%" &&
        latestDeclarationValue(block, "height") === "100%"
    )
  ) {
    fail("web root transparency contract must set html/body/#root to transparent full-height surfaces");
  }
}

for (const selector of ["html", "body", "#root", ".app-shell"]) {
  assertSelectorNeverPaintsFullWindow(selector, {
    backgroundMessage: `${selector} must not paint a full-window non-transparent background`,
    backdropMessage: `${selector} must not apply backdrop-filter over the transparent Electron window`
  });
}

if (!/rel="icon"/.test(webIndex) || !/href="\/favicon\.svg"/.test(webIndex)) {
  fail("web index must declare a static favicon to prevent browser /favicon.ico 404 noise");
}

if (!webFaviconSvg || !/<svg\b/.test(webFaviconSvg)) {
  fail("web public favicon.svg must exist and contain SVG icon markup");
}

const narrowShellRules = extractAtRuleBodies(stylesForChecks, "@media (max-width: 720px)");
const narrowShellRule = narrowShellRules[narrowShellRules.length - 1];
const narrowShell = narrowShellRule?.body ?? "";
if (!narrowShell) {
  fail("narrow viewport shell contract missing @media (max-width: 720px)");
} else {
  const narrowAppShellBlocks = cssBlocksForSelector(narrowShell, ".app-shell");
  if (
    !narrowAppShellBlocks.some(
      (block) =>
        /grid-template-areas:\s*"workbench-left workbench-center workbench-right"/.test(block) &&
        /grid-template-columns:\s*auto\s+minmax\(0,\s*1fr\)\s+auto/.test(block)
    )
  ) {
    fail("narrow viewport shell must preserve the structural workbench columns");
  }

  const narrowSideBodyBlocks = [
    ...cssBlocksForSelector(narrowShell, ".workbench-column-body-left"),
    ...cssBlocksForSelector(narrowShell, ".workbench-column-body-right")
  ];
  if (!narrowSideBodyBlocks.some((block) => /display:\s*none/.test(block))) {
    fail("narrow viewport shell must hide persistent side column bodies while preserving titlebar controls");
  }

  const narrowWorkspaceBlocks = cssBlocksForSelector(narrowShell, ".workspace");
  if (
    !narrowWorkspaceBlocks.some(
      (block) =>
        /grid-template-columns:\s*minmax\(0,\s*1fr\)/.test(block) &&
        /grid-template-rows:\s*minmax\(0,\s*1fr\)/.test(block)
    )
  ) {
    fail("narrow viewport workspace must stay a single center content pane");
  }

  const narrowCenterTitlebarBlocks = cssBlocksForSelector(narrowShell, ".shell-titlebar-center");
  if (!narrowCenterTitlebarBlocks.some((block) => /overflow-x:\s*auto/.test(block))) {
    fail("narrow viewport titlebar center controls must be horizontally scrollable instead of overlapping");
  }

  const narrowTopActionLabelBlocks = cssBlocksForSelector(narrowShell, ".top-action-label");
  if (!narrowTopActionLabelBlocks.some((block) => /display:\s*none/.test(block))) {
    fail("narrow viewport titlebar actions must compress labels to icon controls");
  }

  for (const [selector, property, message] of [
    [".toolbar-library", "flex-wrap", "narrow viewport library toolbar nowrap rule must not be overridden later"],
    [".toolbar-controls", "flex-wrap", "narrow viewport toolbar controls nowrap rule must not be overridden later"],
    [".search-bar", "min-width", "narrow viewport search bar min-width rule must not be overridden later"]
  ]) {
    if (hasLaterDeclaration(stylesForChecks, narrowShellRule.end, selector, property)) {
      fail(message);
    }
  }
}

for (const value of [
  "ShellTitlebarPrimaryActions",
  "ShellRightActions",
  "LibraryTitlebarToolbar",
  "PreviewTitlebarToolbar",
  'data-titlebar-control="library-switcher"',
  'data-titlebar-control="open-plugins"',
  'data-titlebar-control="toggle-sidebars"',
  'data-titlebar-control="open-settings"',
  'aria-label="Open plugins"',
  'aria-label="Open settings"',
  "notification-toggle",
  'aria-label="通知"',
  "WindowChrome"
]) {
  if (!shellTitlebar.includes(value)) {
    fail(`integrated titlebar pieces must expose accessible navigation and chrome controls: missing ${value}`);
  }
}

for (const legacy of ["ShellPrimaryNav", "ShellWorkspaceView", "shell-primary-nav", "shell-nav-button", "shell-nav-caption"]) {
  if (shellTitlebar.includes(legacy) || stylesForChecks.includes(legacy)) {
    fail(`titlebar must not retain the retired Library/Plugins workspace navigation: ${legacy}`);
  }
}
if (app.includes("activeView") || app.includes("<PluginsView")) {
  fail("Plugins must not remain a main-window activeView/content branch");
}

for (const value of ["chrome-title", "chrome-subtitle", "Local media workbench"]) {
  if (shellTitlebar.includes(value) || app.includes(value)) {
    fail(`active integrated titlebar path must not render old brand text/block: found ${value}`);
  }
}

if (app.includes('activeView === "tasks"')) {
  fail("tasks must not render as a permanent workspace view");
}

if (libraryView.includes("<PreviewDialog") || previewPanel.includes("preview-dialog-backdrop")) {
  fail("library preview must be inline, not a modal dialog/backdrop");
}

if (!cssBlocksForExactSelector(stylesForChecks, ".tile-label").some((block) => latestDeclarationValue(block, "text-align") === "center")) {
  fail("middle grid thumbnail labels must be centered");
}

for (const value of ["preview-panel-heading", "preview-panel-open", "Open preview", "panel-title"]) {
  if (previewPanel.includes(value)) {
    fail(`right preview chrome must not render the old preview heading row: found ${value}`);
  }
}

const mediaGridOnClickBodies = extractJsxAttributeBodies(mediaGrid, "onClick");
if (mediaGridOnClickBodies.some((body) => body.includes("onOpenPreview("))) {
  fail("media tile single click must select only; preview opens on double click or keyboard");
}

const mediaGridOnKeyDownBodies = extractJsxAttributeBodies(mediaGrid, "onKeyDown");
const mediaGridHandleKeyDownBody = extractFunctionBody(mediaGrid, "handleKeyDown");

if (
  !mediaGridOnKeyDownBodies.some((body) => handlerSupportsKeyboardPreview(body)) &&
  !handlerSupportsKeyboardPreview(mediaGridHandleKeyDownBody)
) {
  fail("media grid keyboard preview accessibility must open preview from keyboard interaction");
}

if (!mediaGrid.includes("onDoubleClick")) {
  fail("global shell/overlay contract missing onDoubleClick in MediaGrid");
}

if (!app.includes("ShellOverlayHost")) {
  fail("global shell/overlay contract missing ShellOverlayHost in App");
}

if (!shellOverlayHost) {
  fail("global shell/overlay contract missing ShellOverlayHost file");
} else if (!shellOverlayHost.includes("NotificationPopover")) {
  fail("global shell/overlay contract missing NotificationPopover reference in ShellOverlayHost");
}

for (const value of ["onPreviewPrevious", "onPreviewNext"]) {
  if (!libraryView.includes(value) && !previewPanel.includes(value) && !(centralPreviewStage?.includes(value))) {
    fail(`global shell/overlay contract missing ${value} in LibraryView, PreviewPanel, or CentralPreviewStage`);
  }
}

if (previewPanel.includes("preview-panel-inline")) {
  fail("PreviewPanel must not own the large inline preview; double-click preview belongs in the center grid area");
}

if (!previewPanel.includes("showPreviewImage") || !libraryView.includes("showPreviewImage={!previewOpen}")) {
  fail("right inspector must hide its preview image while the central preview is open");
}

if (previewPanel.includes('source="original"')) {
  fail("right inspector selected preview must stay on thumbnail source and must not request original media");
}

if (!previewPanel.includes('source="thumbnail"')) {
  fail("right inspector selected preview must render the light grid_320 thumbnail path");
}

if (!/MediaPreview[\s\S]{0,320}source="thumbnail"[\s\S]{0,320}preserveNaturalFrame=\{false\}[\s\S]{0,320}preferOriginalWhilePending/.test(previewPanel)) {
  fail("right inspector thumbnail fallback must explicitly disable natural-frame sizing so tall originals cannot crop the fixed preview area");
}

if (!mediaResources.includes("getPreviewBlob") || !mediaPreview.includes("requestThumbnailBlob")) {
  fail("MediaPreview must separate central original preview loading from shared thumbnail blob loading");
}

if (
  !/preserveNaturalFrame\??:\s*boolean/.test(mediaPreview) ||
  !/const\s+shouldPreserveNaturalFrame\s*=/.test(mediaPreview) ||
  !/preserveNaturalFrame=\{shouldPreserveNaturalFrame\}/.test(mediaPreview) ||
  !/source\s*===\s*"original"\s*&&\s*preserveNaturalFrame/.test(mediaPreview) ||
  !/setNaturalFrameStyle\(\{[\s\S]*?naturalSize\.naturalHeight/.test(mediaPreview)
) {
  fail("MediaPreview must gate original natural-frame inline sizing so inspector thumbnails stay fixed and contained");
}

if (
  !/containedPreviewMediaStyle/.test(mediaPreview) ||
  !/const\s+\[containedMediaStyle,\s*setContainedMediaStyle\]/.test(mediaPreview) ||
  !/setContainedMediaStyle\(containedPreviewMediaStyle\(naturalSize\)\)/.test(mediaPreview) ||
  !/className="preview-image"[\s\S]{0,1800}onLoad=\{\(event\)\s*=>[\s\S]{0,1800}src=\{src\}[\s\S]{0,160}style=\{containedMediaStyle\}/.test(mediaPreview)
) {
  fail("right inspector preview images must receive an explicit contained media size so portrait images center without cropping");
}

if (!/requestOriginalPreviewBlob/.test(mediaPreview) || !/originalPreviewBlobCache/.test(mediaResources)) {
  fail("MediaPreview central original loading must use the shared original preview cache");
}

if (/getPreviewBlob\(mediaRecord\.id,\s*\{[\s\S]{0,120}signal:/.test(mediaResources)) {
  fail("MediaPreview original-preview cache must not let one AbortSignal cancel shared in-flight reuse");
}

if (!/mediaContentSignature\(media\)/.test(mediaPreview) || !/source="original"[\s\S]*?versionKey=\{originalVersionKey\}/.test(mediaPreview)) {
  fail("MediaPreview central original preview must reload on media signature changes");
}

if (!/hasLiveReadyThumbnail/.test(mediaPreview) || /thumbnail\?\.state\s*===\s*"ready"\s*\?\s*thumbnail\.fileId/.test(mediaPreview)) {
  fail("MediaPreview must wait for live ready thumbnail metadata before requesting fallback thumbnail blobs");
}

if (!/AbortController/.test(mediaPreview) || !/requestOriginalPreviewBlob\(\s*media,\s*\{[\s\S]*?signal:\s*controller\.signal/.test(mediaPreview)) {
  fail("MediaPreview central original requests must use AbortController cleanup");
}

if (/\},\s*\[media,\s*source,\s*versionKey\]\);/.test(mediaPreview)) {
  fail("MediaPreview central original requests must not reset for unchanged media object identity churn");
}

if (/useThumbnailFallbackUrl\(\s*thumbnail\?\.state\s*===\s*"ready"/.test(mediaPreview)) {
  fail("MediaPreview must not fetch thumbnail fallback when thumbnail is already the primary source");
}

if (
  !mediaPreview.includes('source?: "thumbnail" | "original"') ||
  !mediaPreview.includes('source = "thumbnail"')
) {
  fail("MediaPreview must make thumbnail rendering the default and require explicit original preview mode");
}

if (!mediaPreview.includes("previewPlaceholderUrl") || !mediaPreview.includes("fallbackThumbnail")) {
  fail("MediaPreview must support placeholder-first display and thumbnail fallback while original media loads");
}

if (!/previewPlaceholderDataUrl\(media\)/.test(mediaPreview) || /usePreviewPlaceholderUrl/.test(mediaPreview)) {
  fail("MediaPreview placeholder-first path must be synchronous on first render");
}

if (!mediaGrid.includes("previewPlaceholder") || !mediaGrid.includes("previewPlaceholderUrl")) {
  fail("MediaGrid tiles must render previewPlaceholder immediately before grid_320 loads");
}

if (!/previewPlaceholderDataUrl\(item\)/.test(mediaGrid) || /usePreviewPlaceholderUrl/.test(mediaGrid)) {
  fail("MediaGrid placeholder-first path must be synchronous on first render");
}

if (!mediaGrid.includes("requestThumbnailBlob") || mediaGrid.includes("createCoreClient")) {
  fail("MediaGrid must use db_blob thumbnail resources instead of ad hoc client/cache-key loading");
}

if (/thumbnailUpdatedAt=\{thumbnail\?\.updatedAt\s*\?\?\s*null\}/.test(mediaGrid) || !/hasLiveReadyThumbnail/.test(mediaGrid)) {
  fail("MediaGrid must wait for live ready thumbnail metadata before requesting versioned thumbnail blobs");
}

if (!/data-preview-placeholder/.test(mediaGrid) || !/data-preview-placeholder/.test(mediaPreview)) {
  fail("placeholder-first UI must expose data-preview-placeholder markers for smoke validation");
}

if (!centralPreviewStage) {
  fail("central preview stage component missing for middle-area double-click preview");
} else {
  for (const value of [
    "onPreviewWheel",
    "closeOrReturn",
    "central-preview",
    "central-preview-stage",
    "onPreviewPrevious",
    "onPreviewNext",
    "onClosePreview"
  ]) {
    if (!centralPreviewStage.includes(value)) {
      fail(`central preview stage contract missing ${value}`);
    }
  }

  for (const value of [
    "central-preview-header",
    "central-preview-title",
    "central-preview-actions",
    "ChevronLeft",
    "ChevronRight",
    "central-preview-control",
    "lucide-react"
  ]) {
    if (centralPreviewStage.includes(value)) {
      fail(`central preview must not render title/nav/close controls inside the stage: found ${value}`);
    }
  }

  for (const value of [
    "event.ctrlKey",
    "onDoubleClick",
    "onPointerDown",
    "onPointerMove",
    "onPointerUp",
    "setPointerCapture",
    "translate(",
    "scale(",
    "fit-long-edge",
    "fitLongEdgeScale",
    "actualSizeScale",
    "ResizeObserver"
  ]) {
    if (!centralPreviewStage.includes(value)) {
      fail(`central preview interaction contract missing ${value}`);
    }
  }

  if (!centralPreviewStage.includes('source="original"')) {
    fail("central preview must request original media bytes instead of displaying the generated thumbnail");
  }

  if (!centralPreviewStage.includes("onViewStateChange") || !centralPreviewStage.includes("onCommandChange")) {
    fail("central preview must expose state and commands for the integrated center titlebar");
  }

  if (!/MediaPreview[\s\S]{0,220}source="original"[\s\S]{0,220}thumbnail=\{thumbnail\}/.test(centralPreviewStage)) {
    fail("central preview must request original media while passing thumbnail only as a loading fallback");
  }

  if (!centralPreviewStage.includes("translate(-50%, -50%)")) {
    fail("central preview transform must center intrinsic media before applying fit/actual zoom");
  }
}

for (const value of [
  "CentralPreviewStage",
  "previewOpen && selectedMedia",
  "grid-surface-preview"
]) {
  if (!libraryView.includes(value)) {
    fail(`LibraryView must render center preview in the media grid area: missing ${value}`);
  }
}

if (libraryView.includes("<FilterChips")) {
  fail("library filters must be collapsed into one toolbar filter menu button, not a permanent chip row");
}

for (const [label, source] of [
  ["LibraryView", libraryView],
  ["PluginsView", pluginsView],
  ["SettingsView", settingsView]
]) {
  if (/<LiquidGlassSurface[\s\S]{0,220}as="header"[\s\S]{0,220}className="[^"]*\btoolbar\b/.test(source)) {
    fail(`${label} must not render a separate per-view toolbar/header row`);
  }
}

if (libraryView.includes("library-content-toolbar")) {
  fail("LibraryView must not keep the old content toolbar after controls move into the integrated titlebar");
}

if (!pluginsView.includes("plugins-body-actions")) {
  fail("PluginsView actions must live in the page body instead of a header toolbar");
}

for (const value of ["toolbar-titles", "toolbar-title"]) {
  if (libraryView.includes(value)) {
    fail(`library toolbar must not render a standalone title/header block: found ${value}`);
  }
}

for (const value of [
  "ArrowLeft",
  "SearchBar",
  "SortMenu",
  "library-toolbar-back",
  "titlebar-library-summary",
  "PreviewTitlebarToolbar",
  "LibraryTitlebarToolbar"
]) {
  if (!shellTitlebar.includes(value)) {
    fail(`integrated center titlebar toolbar contract missing ${value}`);
  }
}

const titlebarToolButtons = extractJsxElements(shellTitlebar, "LiquidGlassButton").filter((element) =>
  /className=\{?["'`][^"'`]*(?:titlebar-icon-button|titlebar-tool-button)/.test(element)
);
if (titlebarToolButtons.length === 0) {
  fail("middle titlebar must render icon-only tool buttons");
}

for (const button of titlebarToolButtons) {
  if (!/\baria-label=/.test(button)) {
    fail("middle titlebar tool buttons must expose accessible labels with aria-label");
  }
  if (!/\btitle=/.test(button)) {
    fail("middle titlebar tool buttons must expose tooltip text with title");
  }
  const visibleSpans = [...button.matchAll(/<span\b([^>]*)>/g)].filter(
    ([, attributes]) => !attributes.includes("notification-status-dot")
  );
  if (visibleSpans.length > 0 || /top-action-label/.test(button)) {
    fail("middle titlebar tool buttons must stay icon-only with no visible text label inside the button");
  }
}

for (const [label, source] of [
  ["filter titlebar button", filterMenu ?? ""],
  ["sort titlebar button", sortMenu]
]) {
  if (!/\baria-label=/.test(source) || !/\btitle=/.test(source)) {
    fail(`${label} must expose aria-label and title instead of a visible middle-titlebar label`);
  }
}

if (!sortMenu.includes("{iconOnly ? null : <span>{selectedLabel}</span>}")) {
  fail("sort titlebar button must suppress its visible label when rendered icon-only in the middle titlebar");
}

const middleTitlebarToolSelectors = [
  ".shell-titlebar-center .titlebar-tool-button",
  ".shell-titlebar-center .titlebar-icon-button",
  ".shell-titlebar-center .filter-menu-trigger",
  ".shell-titlebar-center .sort-menu-trigger"
];

const middleTitlebarToolClassSelectors = [
  ".titlebar-tool-button",
  ".titlebar-icon-button",
  ".filter-menu-trigger",
  ".sort-menu-trigger",
  ".sort-menu-trigger-open"
];

assertMiddleTitlebarToolSelectorContracts();

for (const selector of middleTitlebarToolSelectors) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (blocks.length === 0) {
    fail(`middle titlebar transparent tool style missing ${selector}`);
  }
  if (
    !blocks.some(
      (block) =>
        latestBackgroundValue(block) === "transparent" &&
        latestDeclarationValue(block, "border") === "0" &&
        latestDeclarationValue(block, "box-shadow") === "none"
    )
  ) {
    fail(`${selector} must declare background: transparent, border: 0, and box-shadow: none`);
  }
}

const middleTitlebarToolRootBlocks = cssRuleBlocksForMatchingSelectors(stylesForChecks, (selector) =>
  isMiddleTitlebarToolRootSelector(selector)
);
assertMiddleTitlebarToolRulesDoNotFrame(middleTitlebarToolRootBlocks, "middle titlebar tool state");
assertMiddleTitlebarButtonsUseUnifiedGlow();
assertMiddleTitlebarFocusUsesInsetOutline();

const middleTitlebarToolGuardEnd = Math.max(
  0,
  ...middleTitlebarToolRootBlocks.map(({ end }) => end ?? 0)
);
const laterMiddleTitlebarToolRootBlocks = cssRuleBlocksForMatchingSelectors(
  stylesForChecks.slice(middleTitlebarToolGuardEnd),
  (selector) => isMiddleTitlebarToolRootSelector(selector, { includeUnscoped: true })
);
assertMiddleTitlebarToolRulesDoNotFrame(
  laterMiddleTitlebarToolRootBlocks,
  "later middle titlebar tool state"
);

const titlebarSearchBlocks = cssBlocksForExactSelector(stylesForChecks, ".titlebar-library-search");
if (!titlebarSearchBlocks.some((block) => latestDeclarationValue(block, "margin-left") === "auto")) {
  fail("middle titlebar search must stay right-aligned with margin-left: auto");
}

if (!libraryFilterSources.includes("SlidersHorizontal")) {
  fail("compact library toolbar contract missing SlidersHorizontal filter icon");
}

if (!filterMenu) {
  fail("collapsed library filter menu component missing");
} else {
  for (const value of [
    "SlidersHorizontal",
    "filter-menu-popover",
    "Kind",
    "Rating",
    "Favorite",
    "Tags",
    "Clear filters"
  ]) {
    if (!filterMenu.includes(value)) {
      fail(`filter menu must expose collapsed filter sub-items: missing ${value}`);
    }
  }
}

for (const value of [
  'type CompactPopover = "notifications" | "filter" | "sort" | null',
  "activeCompactPopover",
  "Escape",
  "pointerdown",
  "data-compact-popover-root",
  "filterMenuOpen",
  "sortMenuOpen"
]) {
  if (!app.includes(value)) {
    fail(`compact popover coordination contract missing ${value}`);
  }
}

for (const [label, source, values] of [
  [
    "filter menu",
    filterMenu ?? "",
    ["data-compact-popover-root", "filter-menu-popover", "floating-popover", "LiquidGlassSurface", 'tone="elevated"']
  ],
  [
    "sort menu",
    sortMenu,
    ["data-compact-popover-root", "sort-menu-popover", "floating-popover", "LiquidGlassSurface", 'tone="elevated"']
  ],
  [
    "notification popover",
    notificationPopover,
    [
      'data-compact-popover-root="notifications"',
      "notification-popover",
      "popup-surface",
      "floating-popover",
      "LiquidGlassSurface",
      'tone="elevated"'
    ]
  ]
]) {
  for (const value of values) {
    if (!source.includes(value)) {
      fail(`${label} compact popover contract missing ${value}`);
    }
  }
}
for (const value of [
  "data-notification-window-fixture",
  "activeLogSubscriptions",
  "logUnsubscriptions",
  "droppedBeforeSequence",
  "retry-error",
  "thumbnailSummaries"
]) {
  if (!notificationWindowFixture.includes(value)) {
    fail(`notification full-window renderer fixture missing ${value}`);
  }
}

const shellRightActionsBlock =
  shellTitlebar.match(/export function ShellRightActions[\s\S]*?\n}\n/)?.[0] ?? "";
for (const value of [
  'data-titlebar-control="notifications"',
  'data-compact-popover-trigger="notifications"',
  "titlebar-icon-button",
  "notification-status-dot",
  "Bell"
]) {
  if (!shellRightActionsBlock.includes(value)) {
    fail(`notification titlebar control missing ${value}`);
  }
}
const retiredTaskSurfaceName = ["tasks", "palette"].join("-");
const retiredRecentSurfaceName = ["recent", "operations"].join("-");
for (const retiredValue of [retiredTaskSurfaceName, retiredRecentSurfaceName, "top-action-label"]) {
  if (shellRightActionsBlock.includes(retiredValue)) {
    fail(`notification titlebar replacement must remove ${retiredValue}`);
  }
}
const shellPresentationSource = [app, appShell, shellTitlebar, shellOverlayHost ?? ""].join("\n");
for (const [label, pattern] of [
  ["Tasks titlebar surface", new RegExp(`data-titlebar-control=["'](?:tasks|${retiredTaskSurfaceName})["']`)],
  ["Recent titlebar surface", new RegExp(`data-titlebar-control=["'](?:recent|${retiredRecentSurfaceName})["']`)],
  ["Tasks compact popover", /data-compact-popover-(?:trigger|root)=["']tasks["']/],
  ["Recent compact popover", /data-compact-popover-(?:trigger|root)=["']recent["']/]
]) {
  if (pattern.test(shellPresentationSource)) {
    fail(`${label} must not be reintroduced beside the notification center`);
  }
}
if ((shellRightActionsBlock.match(/data-titlebar-control="notifications"/g) ?? []).length !== 1) {
  fail("titlebar must render exactly one notification control");
}
for (const value of [
  "notification-active-list",
  "notification-progress-track",
  "notification-progress-fill",
  "notification-progress-indeterminate",
  "notification-thumbnail-summary",
  "notification-thumbnail-summary-list",
  "data-notification-thumbnail-library-count",
  "data-overlay-scroll-container",
  "notification-result-list",
  "notification-popover-empty",
  "notification-popover-degraded",
  "notification-open-center"
]) {
  if (!notificationPopover.includes(value)) {
    fail(`notification popover visual contract missing ${value}`);
  }
}
if (
  !/\.notification-popover-body\s*\{[^}]*min-height:\s*0;[^}]*overflow:\s*auto;/s.test(
    stylesForChecks
  ) ||
  !/\.notification-popover-footer\s*\{[^}]*flex:\s*0 0 auto;/s.test(stylesForChecks)
) {
  fail("notification popover must scroll its content while keeping the full-center footer reachable");
}
if (/\b(?:cancel|retry|onCancel|onRetry)\b/i.test(notificationPopover)) {
  fail("compact notification UI must not contain task actions");
}
for (const selector of [
  ".notification-popover",
  ".notification-status-dot",
  ".notification-progress-track",
  ".notification-progress-fill",
  ".notification-progress-indeterminate"
]) {
  if (cssBlocksForExactSelector(stylesForChecks, selector).length === 0) {
    fail(`notification compact UI style missing ${selector}`);
  }
}

for (const selector of [".floating-popover", ".filter-menu-popover", ".sort-menu-popover"]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (blocks.length === 0) {
    fail(`compact popover material selector missing ${selector}`);
  }
}

const floatingPopoverBlocks = cssBlocksForExactSelector(stylesForChecks, ".floating-popover");
if (
  !floatingPopoverBlocks.some(
    (block) =>
      /--glass-fill:\s*var\(--glass-elevated\)/.test(block) &&
      /--glass-blur-current:\s*var\(--glass-elevated-blur\)/.test(block) &&
      hasNonNoneDeclaration(block, "box-shadow")
  )
) {
  fail("floating popovers must use elevated glass material instead of flat content styling");
}

for (const value of [
  "megle.interfaceStyle",
  "DEFAULT_INTERFACE_STYLE",
  "windowCornerRadius",
  "surfaceCornerRadius",
  "controlCornerRadius",
  "contentCornerRadius",
  "sideBlur",
  "sideOpacity",
  "sideOverlayStrength",
  "sideOverlayColor",
  "sideSaturation",
  "sideStrokeOpacity",
  "centerBlur",
  "centerOpacity",
  "centerOverlayStrength",
  "centerOverlayColor",
  "centerSaturation",
  "centerStrokeOpacity",
  "ditherOpacity",
  "backdropGradientStrength",
  "edgeHighlightBrightness",
  "haloBrightness",
  "pointerResponseRadius",
  "pointerSensingDistance",
  "pointerFalloff",
  "dialogBlur",
  "dialogOpacity",
  "dialogOverlayStrength",
  "dialogBackdropDim",
  "interfaceStyleToCssVariables",
  "applyInterfaceStyleVariables",
  "useInterfaceStyle"
]) {
  if (!interfaceStyle.includes(value)) {
    fail(`interface style model missing ${value}`);
  }
}

for (const value of [
  "--radius-window",
  "--radius-surface",
  "--radius-control",
  "--radius-content",
  "--glass-side-blur",
  "--glass-side-fill",
  "--glass-side-stroke",
  "--glass-center-blur",
  "--glass-center-fill",
  "--glass-center-stroke",
  "--glass-dither-opacity",
  "--glass-backdrop-gradient-opacity",
  "--glass-pointer-sensing-distance",
  "--glass-pointer-glow-radius",
  "--glass-edge-highlight-brightness",
  "--glass-dialog-blur",
  "--glass-dialog-fill",
  "--dialog-blur",
  "--dialog-backdrop-dim",
  "--glass-blur",
  "--glass-elevated-blur",
  "--glass-control-blur",
  "--glass-saturation",
  "--glass-control",
  "--glass-readable-surface"
]) {
  if (!stylesForChecks.includes(value) && !interfaceStyle.includes(value)) {
    fail(`interface style CSS variable contract missing ${value}`);
  }
}

const defaultInterfaceStyleBody = extractObjectBody(interfaceStyle, "DEFAULT_INTERFACE_STYLE");
if (!/windowCornerRadius:\s*8\b/.test(defaultInterfaceStyleBody)) {
  fail("default window corner radius must default to 8 to match Windows DWM's native host radius (user-adjustable at runtime)");
}

if (!/haloBrightness:\s*(?:1\.[2-9]\d*|[2-9](?:\.\d+)?)\b/.test(defaultInterfaceStyleBody)) {
  fail("default halo brightness must be visibly stronger than the old 1x baseline");
}

if (!/edgeHighlightBrightness:\s*(?:6|7|8)(?:\.\d+)?\b/.test(defaultInterfaceStyleBody)) {
  fail("default edge highlight brightness must be visibly stronger than the old 5x baseline");
}

if (!settingsView.includes("恢复默认外观") || !settingsView.includes("AppearanceTab")) {
  fail("settings must expose Interface style controls and reset");
}

for (const value of [
  "圆角",
  "侧栏材质",
  "中央工作区材质",
  "玻璃平滑度",
  "交互高光",
  "对话框材质",
  'id="window-corner-radius"',
  'id="surface-corner-radius"',
  'id="control-corner-radius"',
  'id="content-corner-radius"',
  'id="side-blur"',
  'id="side-opacity"',
  'id="side-overlay-strength"',
  'id="side-overlay-color"',
  'id="side-saturation"',
  'id="side-stroke-opacity"',
  'id="center-blur"',
  'id="center-opacity"',
  'id="center-overlay-strength"',
  'id="center-overlay-color"',
  'id="center-saturation"',
  'id="center-stroke-opacity"',
  'id="dither-opacity"',
  'id="backdrop-gradient-strength"',
  'id="edge-highlight-brightness"',
  'id="halo-brightness"',
  'id="pointer-response-radius"',
  'id="pointer-sensing-distance"',
  'id="pointer-falloff"',
  'id="dialog-blur"',
  'id="dialog-opacity"',
  'id="dialog-overlay-strength"',
  'id="dialog-backdrop-dim"'
]) {
  if (!settingsView.includes(value)) {
    fail(`settings Interface style must expose approved material-family controls: missing ${value}`);
  }
}

const settingsWindow = read("apps/web/src/features/settings/SettingsWindow.tsx");
if (
  !app.includes("useInterfaceStyle") ||
  !settingsWindow.includes("interfaceStyle={interfaceStyle}")
) {
  fail("App must wire local Interface style settings into the renderer and the settings window must pass it to the panels");
}
for (const [label, source] of [
  ["SettingsWindow", settingsWindow],
  ["PluginsWindow", pluginsWindow],
  ["NotificationWindow", notificationWindow]
]) {
  if (!source.includes("SecondaryWindowFrame")) {
    fail(`${label} must share the frameless secondary-window titlebar/material frame`);
  }
}
for (const value of [
  "secondary-window-titlebar",
  "secondary-window-close",
  "LiquidGlassLayer",
  "LiquidGlassSurface",
  "windowControls",
  "onPointerDown",
  "notifySecondaryWindowReady",
  "secondaryRendererReady"
]) {
  if (!secondaryWindowFrame.includes(value)) {
    fail(`shared secondary-window frame missing ${value}`);
  }
}
for (const value of ["useInterfaceStyle", "subscribePreference", "broadcastChanges", "lastObservedValue"]) {
  if (!secondaryWindowInterfaceStyle.includes(value)) {
    fail(`shared secondary-window material synchronization missing ${value}`);
  }
}
for (const [propertyName, identifierName] of [
  ["--secondary-window-pane-dark", "secondaryWindowPaneDark"],
  ["--secondary-window-pane-light", "secondaryWindowPaneLight"],
  ["--secondary-window-pane-end", "secondaryWindowPaneEnd"]
]) {
  if (
    !functionDirectReturnObjectMapsIdentifiers(interfaceStyle, "interfaceStyleToCssVariables", [
      [propertyName, identifierName]
    ])
  ) {
    fail(
      `interfaceStyleToCssVariables missing derived secondary-window mapping "${propertyName}": ${identifierName}`
    );
  }
}
if (
  !jsxElementContainsClassDescendants(
    pluginsView,
    "apps/web/src/features/plugins/PluginsView.tsx",
    "plugins-sidebar",
    ["plugins-body-actions", "plugins-list"]
  )
) {
  fail("PluginsView must group sidebar controls and the plugin list in .plugins-sidebar");
}
if (
  !settingsWindow.includes("useSecondaryWindowInterfaceStyle({ broadcastChanges: true })") ||
  !pluginsWindow.includes("useSecondaryWindowInterfaceStyle()")
) {
  fail("Settings and Plugins must share side-material initialization and live synchronization");
}
for (const value of ["applyInterfaceStyleVariables(readInterfaceStyle())", "preloadSecondaryWindowRenderers"]) {
  if (!webMain.includes(value)) {
    fail(`web entry must prepare secondary material/chunks before first visible frame: ${value}`);
  }
}
if (!webMain.includes('windowRole === "plugins"') || !webMain.includes('import("./features/plugins/PluginsWindow")')) {
  fail("web entry must render PluginsWindow as a standalone renderer mode");
}
if (!webMain.includes('windowRole === "notifications"') || !webMain.includes('import("./features/notifications/NotificationWindow")')) {
  fail("web entry must render NotificationWindow as a standalone renderer mode");
}

for (const value of [
  'role="notifications"',
  'role="tablist"',
  "NotificationStatusView",
  "NotificationHistoryView",
  "ProcessLogView",
  "notification-window-nav",
  "notification-window-content"
]) {
  if (!notificationWindow.includes(value)) {
    fail(`full notification center window missing ${value}`);
  }
}
if (/\b(?:unread|readAt|markRead|未读|已读)\b/i.test(notificationWindow)) {
  fail("notification center must not introduce read or unread state");
}
for (const [label, source, values] of [
  ["status", notificationStatusView, ["notification-status-active", "notification-status-thumbnails", "notification-status-results", "notification-status-health", "cancellable", "cancel"]],
  ["history", notificationHistoryView, ["useVirtualizer", "cancellable", "retryable", "loadMoreHistory", "getHistoryFacets", "origins", "kinds"]],
  ["process logs", processLogView, ["useVirtualizer", "openLogSubscription", "unsubscribe", "processLogGapSequence", "clearLogs", "exportLogs", "LOG_PAGE_SIZE = 200", "加载更早日志"]]
]) {
  for (const value of values) {
    if (!source.includes(value)) fail(`notification ${label} view missing ${value}`);
  }
}
if (/new Blob|URL\.createObjectURL|anchor\.download/.test(processLogView)) {
  fail("notification process log export must use the narrow Desktop save-dialog bridge");
}
if (/MAX_CLIENT_FILTER_PAGES|visibleItems\s*=\s*useMemo/.test(notificationHistoryView)) {
  fail("notification history source/kind filters must remain in the Desktop paged query");
}

if (!shortcutBindings) {
  fail("editable shortcut binding model missing");
} else {
  for (const value of [
    "megle.shortcutBindings.v1",
    "focusSearch",
    "renameSelected",
    "recycleDelete",
    "permanentDelete",
    "closeOrReturn",
    "previewNext",
    "previewPrevious",
    "zoomIn",
    "zoomOut",
    "toggleSidebars",
    "Tab",
    "defaultBinding",
    "normalizeShortcutEvent",
    "matchShortcut",
    "resetShortcutBindings"
  ]) {
    if (!shortcutBindings.includes(value)) {
      fail(`editable shortcut binding model missing ${value}`);
    }
  }
}

for (const value of [
  "ShortcutsTab",
  "useShortcutBindings",
  "onReset",
  "shortcut-binding-capture",
  "恢复默认快捷键"
]) {
  if (!settingsView.includes(value)) {
    fail(`settings must include editable shortcut bindings: missing ${value}`);
  }
}

for (const value of [
  "useShortcutBindings",
  "matchShortcut",
  "focusSearch",
  "renameSelected",
  "recycleDelete",
  "permanentDelete",
  "closeOrReturn",
  "previewNext",
  "previewPrevious",
  "zoomIn",
  "zoomOut",
  "toggleSidebars",
  "onToggleSidebars"
]) {
  if (!useShortcuts.includes(value)) {
    fail(`global shortcuts must read editable bindings: missing ${value}`);
  }
}

for (const [source, value, message] of [
  [app, "sidebarsHidden", "App must own sidebarsHidden state so hiding sidebars preserves Library state"],
  [app, "onToggleSidebars", "App must wire sidebar hiding into titlebar controls"],
  [appShell, "sidebarsHidden", "AppShell must receive hidden-sidebar shell state"],
  [appShell, "data-sidebars-hidden", "AppShell must expose hidden-sidebar state to CSS"],
  [shellTitlebar, "ShellSidebarToggle", "titlebar must expose the sidebar toggle control"],
  [shellTitlebar, "shell-titlebar-primary-actions", "library/plugins/settings/sidebar controls must share one titlebar action group"],
  [shellTitlebar, "shell-titlebar-utility-group", "Plugins/Settings/sidebar controls must use a dedicated right-aligned group"],
  [shellTitlebar, "shell-library-switcher-button", "active library selector must move into the titlebar"],
  [shellTitlebar, "shell-plugins-button", "Plugins must have a standalone titlebar utility button"],
  [shellTitlebar, "shell-settings-button", "settings must have a standalone utility button beside the sidebar toggle"],
  [shellTitlebar, "titlebar-shell-actions", "center titlebar must host right shell actions when sidebars are hidden"],
  [stylesForChecks, ".app-shell[data-sidebars-hidden=\"true\"]", "styles must define the hidden-sidebar shell layout"],
  [stylesForChecks, ".shell-sidebar-toggle", "styles must define the sidebar toggle affordance"],
  [stylesForChecks, ".titlebar-shell-actions", "styles must place shell/window actions inside the center titlebar"]
]) {
  if (!source.includes(value)) {
    fail(message);
  }
}

if (/\{\s*id:\s*"settings"[\s\S]*?caption:\s*"Settings"/.test(shellTitlebar)) {
  fail("settings must not remain duplicated inside the primary workspace tab group");
}

if (/data-pointer-glow="off"/.test(shellTitlebar)) {
  fail("library selector, Plugins, settings, and sidebar-toggle controls must use the unified pointer highlight");
}

const titlebarUtilityGroupBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".shell-titlebar-utility-group"
);
if (
  !titlebarUtilityGroupBlocks.some(
    (block) =>
      latestDeclarationValue(block, "display") === "inline-flex" &&
      latestDeclarationValue(block, "margin-inline-start") === "auto"
  )
) {
  fail("titlebar utility group must use normal-flow flex auto alignment on the right");
}
const librarySwitcherSizingBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".shell-library-switcher-button"
);
if (
  !librarySwitcherSizingBlocks.some(
    (block) =>
      latestDeclarationValue(block, "flex") === "0 1 auto" &&
      latestDeclarationValue(block, "max-width") === "168px"
  ) ||
  !librarySwitcherSizingBlocks.some(
    (block) => latestDeclarationValue(block, "width") === "auto"
  )
) {
  fail("library selector must size to icon/name/chevron content with a bounded long-name width");
}

const secondaryMaterialBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".liquid-glass.secondary-window"
);
for (const [property, expected] of [
  ["--glass-fill", "var(--glass-side-fill)"],
  ["--glass-blur-current", "var(--glass-side-blur)"],
  ["--glass-border-current", "var(--glass-side-stroke)"],
  ["--glass-saturation", "var(--glass-side-saturation)"]
]) {
  if (!secondaryMaterialBlocks.some((block) => latestDeclarationValue(block, property) === expected)) {
    fail(`secondary windows must reuse the sidebar LiquidGlass material mapping: ${property}`);
  }
}

const centerTitlebarMatch = app.match(
  /function renderCenterTitlebar\(\) \{([\s\S]*?)\n  \}\n\n  function renderPrimaryTitlebarActions/
);
if (
  !centerTitlebarMatch ||
  !centerTitlebarMatch[1].includes("ShellSidebarToggle") ||
  centerTitlebarMatch[1].includes("renderPrimaryTitlebarActions") ||
  !/export function ShellSidebarToggle\(/.test(shellTitlebar)
) {
  fail("collapsed Library and Preview titlebars must inject only the shared SidebarToggle, never the full primary action group");
}

const secondaryWarmupMatch = desktopMain.match(
  /async function warmSecondaryWindowNativeHost\([\s\S]*?\n\}/
);
const secondaryWarmupBody = secondaryWarmupMatch?.[0] ?? "";
const invisibleShowIndex = secondaryWarmupBody.indexOf("window.showInactive()");
const warmupIndex = secondaryWarmupBody.indexOf("await waitForSecondaryNativeWindowWarmup(");
const nativeRevealTimestampIndex = secondaryWarmupBody.indexOf("nativeWindowRevealedAt = Date.now()");
const nativeRevealIndex = secondaryWarmupBody.indexOf("revealSecondaryNativeWindow(");
if (
  invisibleShowIndex < 0 ||
  warmupIndex <= invisibleShowIndex ||
  nativeRevealTimestampIndex <= warmupIndex ||
  nativeRevealIndex <= nativeRevealTimestampIndex ||
  !secondaryWarmupBody.includes("NATIVE_WINDOW_WARMUP_MS") ||
  !secondaryReveal.includes("export const NATIVE_WINDOW_WARMUP_MS = 100") ||
  !/opacity:\s*0/.test(desktopMain)
) {
  fail("secondary windows must start transparent, warm the shown native host for 100ms, then reveal it once");
}
for (const [source, value, message] of [
  [desktopMain, "nativeWarmupStartedAt", "desktop secondary-window readiness must record native warm-up start"],
  [desktopMain, "nativeWindowRevealedAt", "desktop secondary-window readiness must record opacity reveal"],
  [desktopBridge, "nativeWindowRevealedAt", "web desktop bridge must carry the native opacity reveal result"],
  [secondaryWindowFrame, "data-secondary-content-visible", "secondary UI must have an explicit content-visible stage after native reveal"]
]) {
  if (!source.includes(value)) fail(message);
}
for (const [source, value, message] of [
  [desktopMain, "forceSecondaryWindowReveal", "secondary windows must retain a force-reveal path for the 4s failure fallback"],
  [desktopMain, "fallback: false", "normal secondary warm-up must report that it did not use the failure fallback"],
  [desktopBridge, "fallback: boolean", "secondary reveal results must distinguish normal composition from the failure fallback"],
  [secondaryWindowFrame, "data-secondary-reveal-fallback", "secondary window evidence must expose whether the 4s failure fallback was used"]
]) {
  if (!source.includes(value)) fail(message);
}
const readyResultIndex = secondaryWindowFrame.indexOf("notifySecondaryWindowReady(role).then((result)");
const contentRevealFrameIndex = secondaryWindowFrame.indexOf(
  "contentRevealFrame = window.requestAnimationFrame"
);
const contentVisibleIndex = secondaryWindowFrame.indexOf(
  'revealFrame.setAttribute("data-secondary-content-visible", "true")'
);
if (
  readyResultIndex < 0 ||
  contentRevealFrameIndex <= readyResultIndex ||
  contentVisibleIndex <= contentRevealFrameIndex ||
  !secondaryWindowFrame.includes("contentRevealFrame") ||
  !secondaryWindowFrame.includes("window.requestAnimationFrame")
) {
  fail("secondary UI must switch visibility once on the renderer frame after native opacity reveal");
}

for (const selector of [".secondary-window-titlebar", ".secondary-window-body"]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        latestDeclarationValue(block, "visibility") === "hidden" &&
        latestDeclarationValue(block, "content-visibility") === "hidden"
    )
  ) {
    fail(`${selector} must stay non-visible while the opacity-zero native window warms up`);
  }
  const revealSelector = `.secondary-window[data-secondary-content-visible="true"] ${selector}`;
  const revealBlocks = cssBlocksForExactSelector(stylesForChecks, revealSelector);
  if (
    !revealBlocks.some(
      (block) =>
        latestDeclarationValue(block, "visibility") === "visible" &&
        latestDeclarationValue(block, "content-visibility") === "visible"
    )
  ) {
    fail(`${selector} must reveal only from the explicit secondary content-visible stage`);
  }
}
const secondaryWindowTitlebarBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".secondary-window-titlebar"
);
const secondaryWindowCloseBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".secondary-window-close"
);
if (
  !secondaryWindowTitlebarBlocks.some((block) => /height:\s*40px/.test(block)) ||
  !secondaryWindowCloseBlocks.some(
    (block) => /width:\s*30px/.test(block) && /height:\s*30px/.test(block)
  )
) {
  fail("Settings and Plugins windows must retain their independent 40px titlebar and 30px close button sizing");
}
if (
  latestDeclarationValueAcrossBlocks(secondaryWindowTitlebarBlocks, "border-bottom") !==
  "1px solid var(--line)"
) {
  fail("secondary window titlebar must retain its bottom border above the split body material");
}
const settingsDialogNavBlocks = cssBlocksForExactSelector(stylesForChecks, ".settings-dialog-nav");
if (
  latestDeclarationValueAcrossBlocks(settingsDialogNavBlocks, "border-right") !== "0" ||
  latestDeclarationValueAcrossBlocks(settingsDialogNavBlocks, "background") !==
    "var(--secondary-window-pane-dark)"
) {
  fail("Settings navigation must use the dark secondary-window pane without a vertical divider");
}
const settingsDialogMainBlocks = cssBlocksForExactSelector(stylesForChecks, ".settings-dialog-main");
const settingsDialogMainBackground = latestDeclarationValueAcrossBlocks(
  settingsDialogMainBlocks,
  "background"
);
if (
  !settingsDialogMainBackground?.includes("linear-gradient") ||
  !settingsDialogMainBackground.includes("var(--secondary-window-pane-light)") ||
  !settingsDialogMainBackground.includes("var(--secondary-window-pane-end)")
) {
  fail("Settings content must use the semantic light-to-dark secondary-window pane gradient");
}
const pluginsSidebarBlocks = cssBlocksForExactSelector(stylesForChecks, ".plugins-sidebar");
if (
  latestDeclarationValueAcrossBlocks(pluginsSidebarBlocks, "background") !==
  "var(--secondary-window-pane-dark)"
) {
  fail("Plugins sidebar must use the dark secondary-window pane material");
}
const secondaryWindowBlocks = cssBlocksForExactSelector(stylesForChecks, ".secondary-window");
if (
  !secondaryWindowBlocks.some(
    (block) =>
      latestDeclarationValue(block, "background") === "transparent" &&
      latestDeclarationValue(block, "backdrop-filter") === null &&
      latestDeclarationValue(block, "-webkit-backdrop-filter") === null
  )
) {
  fail("secondary window root must not hard-code a second material over the shared LiquidGlass surface");
}

const sidebarSpacingBlocks = cssBlocksForExactSelector(stylesForChecks, ".library-sidebar");
const treeListBlocks = cssBlocksForExactSelector(stylesForChecks, ".tree-list");
if (
  !sidebarSpacingBlocks.some((block) => latestDeclarationValue(block, "padding") === "0 16px 16px") ||
  !treeListBlocks.some((block) => latestDeclarationValue(block, "margin-top") === "0")
) {
  fail("folder tree scrollport must start at the first pixel while retaining side/bottom padding");
}
if (
  stylesForChecks.includes("--tree-sticky-scrollport-inset") ||
  librarySidebar.includes("--tree-sticky-scrollport-inset")
) {
  fail("folder tree sticky coordinates must not retain the retired top-padding scrollport inset compensation");
}

for (const selector of [
  ".shell-library-switcher-button",
  ".shell-titlebar-utility-button",
  ".shell-sidebar-toggle"
]) {
  const controlBlocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !controlBlocks.some(
      (block) =>
        latestDeclarationValue(block, "border") === "0" &&
        latestDeclarationValue(block, "box-shadow") === "none"
    )
  ) {
    fail(`${selector} must remove only its static border/shadow while retaining the LiquidGlass control structure`);
  }

  const edgeBlocks = cssBlocksForExactSelector(stylesForChecks, `${selector}::after`);
  if (
    !edgeBlocks.some(
      (block) =>
        latestDeclarationValue(block, "border") === "0" &&
        latestDeclarationValue(block, "box-shadow") === "none" &&
        latestDeclarationValue(block, "opacity") === null &&
        latestDeclarationValue(block, "background") === null
    )
  ) {
    fail(`${selector}::after must remove the static LiquidGlass stroke without suppressing the unified pointer edge`);
  }

  const focusBlocks = cssBlocksForExactSelector(stylesForChecks, `${selector}:focus-visible`);
  if (!focusBlocks.some((block) => latestDeclarationValue(block, "outline")?.startsWith("2px solid"))) {
    fail(`${selector} must retain a visible keyboard-focus outline after static border removal`);
  }
}

for (const value of [
  "--radius-window",
  "--radius-overlay",
  "--radius-panel",
  "--radius-control"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`global shell/overlay contract missing ${value} in styles.css`);
  }
}

if (!stylesForChecks.includes("--glass-dither-opacity")) {
  fail("global glass material contract missing --glass-dither-opacity for anti-banding control");
}

if (/fractalNoise/.test(stylesForChecks)) {
  fail("glass dither texture must not use fractalNoise; it creates low-frequency clouding on acrylic gradients");
}

if (
  !stylesForChecks.includes("megle-blue-noise-dither") ||
  !stylesForChecks.includes("feConvolveMatrix") ||
  !stylesForChecks.includes("type='turbulence'") ||
  !stylesForChecks.includes("numOctaves='1'")
) {
  fail("glass dither texture must use a high-pass blue-noise-style turbulence mask for smoother acrylic gradients");
}

if (!liquidGlassSurface.includes("liquid-glass-dither")) {
  fail("liquid glass material primitive must include an unfiltered anti-banding dither layer");
}

const glassDitherBlocks = cssBlocksForExactSelector(stylesForChecks, ".liquid-glass-dither");
if (
  !glassDitherBlocks.some(
    (block) =>
      /mix-blend-mode:\s*(?:overlay|soft-light)/.test(block) &&
      /opacity:\s*var\(--glass-dither-opacity\)/.test(block) &&
      /filter:\s*none/.test(block)
  )
) {
  fail("liquid glass dither must be an unfiltered blend layer above blurred/acrylic material to reduce color banding");
}

const shellBackdropCanvasBlocks = cssBlocksForExactSelector(stylesForChecks, ".shell-backdrop-canvas");
const shellBackdropDitherBlocks = cssBlocksForExactSelector(stylesForChecks, ".shell-backdrop-canvas::after");
if (
  !shellBackdropCanvasBlocks.some(
    (block) =>
      block.includes("--glass-backdrop-gradient-opacity") &&
      /calc\(var\(--glass-backdrop-gradient-opacity\)\s*\*/.test(block)
  )
) {
  fail("shell backdrop gradients must be intensity-controlled by --glass-backdrop-gradient-opacity to reduce banding globally");
}

if (
  !shellBackdropDitherBlocks.some(
    (block) =>
      /background-image:\s*var\(--glass-dither-texture\)/.test(block) &&
      /opacity:\s*calc\(var\(--glass-dither-opacity\)\s*\*/.test(block)
  )
) {
  fail("shell backdrop must share the global anti-banding dither layer");
}

const interactivePointerBackgroundBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  '[data-interactive-pointer-target="true"]::before'
);
if (
  !interactivePointerBackgroundBlocks.some(
    (block) =>
      block.includes("--glass-pointer-fill-stops") &&
      block.includes("--glass-pointer-glow-radius")
  )
) {
  fail("interactive pointer background glow must draw the single screen-space circle (centre-decaying falloff stops within the glow radius, centred on the real cursor)");
}

const interactivePointerEdgeBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  '[data-interactive-pointer-target="true"]::after'
);
if (
  !interactivePointerEdgeBlocks.some(
    (block) =>
      block.includes("--interactive-pointer-x") &&
      block.includes("--interactive-pointer-y")
  )
) {
  fail("interactive pointer edge highlights must use the same real cursor coordinates as the background glow");
}

for (const [token, minValue] of [
  ["--glass-pointer-fill-opacity", 0.024],
  ["--glass-border-highlight-opacity", 0.24]
]) {
  const value = cssVariableNumber(stylesForChecks, token);
  if (value === null || value < minValue) {
    fail(`${token} must be high enough for visible desktop-through-window liquid-glass pointer highlights`);
  }
}

for (const value of [
  "--glass-pointer-glow-radius",
  "--glass-pointer-sensing-distance",
  "--glass-pointer-fill-opacity",
  "--glass-border-highlight-opacity",
  "liquid-glass-edge",
  "mask-composite: exclude"
]) {
  if (!stylesForChecks.includes(value) && !liquidGlassSurface.includes(value)) {
    fail(`liquid glass pointer highlight must use shared global/local-border tokens: missing ${value}`);
  }
}

const glassEdgeBlocks = cssBlocksForExactSelector(stylesForChecks, ".liquid-glass-edge");
if (!liquidGlassSurface.includes("clientX - rect.left")) {
  fail("liquid glass pointer routing must write the real cursor position in element-local pixels (single screen-space circle model)");
}
if (!liquidGlassSurface.includes("style.setProperty(\"--interactive-pointer-x\"")) {
  fail("affordance pointer routing must write real cursor coordinates: missing --interactive-pointer-x");
}

if (
  liquidGlassSurface.includes("'input:not([data-liquid-glass])'") ||
  liquidGlassSurface.includes('"input:not([data-liquid-glass])"')
) {
  fail("interactive affordance routing must not target native inputs wholesale; range sliders need a stylable visual owner");
}

for (const value of [
  "INTERACTIVE_AFFORDANCE_OWNER_SELECTOR",
  ".tree-item",
  '[data-liquid-glass="button"]',
  ".settings-style-slider-control",
  'input:not([type="range"]):not([data-liquid-glass])',
  "interactiveAffordanceTargets"
]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`interactive affordance routing must resolve pointer highlights to visual owner elements: missing ${value}`);
  }
}

const legacyButtonHoverBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ':where(button, [role="button"]):hover:not(:disabled)'
);
if (
  legacyButtonHoverBlocks.some(
    (block) =>
      hasNonTransparentBackground(block) ||
      hasNonNoneDeclaration(block, "background-image") ||
      hasNonzeroBorderDeclaration(block, "border") ||
      hasVisibleBorderDeclaration(block, "border-color")
  )
) {
  fail("generic button hover must not paint a full-control background or border over the unified pointer highlight");
}
if (!legacyButtonHoverBlocks.some((block) => latestDeclarationValue(block, "transform") === "translateY(-1px)")) {
  fail("generic button hover must preserve the existing one-pixel lift feedback");
}

const sidebarPointerScopeClasses = [
  ".library-sidebar",
  ".tree-item",
  ".tree-label",
  ".tree-disclosure",
  ".tree-load-more",
  ".library-switcher",
  ".library-switcher-search",
  ".library-switcher-row",
  ".library-switcher-icon",
  ".library-switcher-main",
  ".library-switcher-actions",
  ".library-icon-picker-cell",
  ".library-switcher-remove-menu-item",
  ".library-switcher-footer",
  ".shell-titlebar-primary-actions",
  ".shell-library-switcher-button",
  ".shell-titlebar-utility-button",
  ".shell-sidebar-toggle"
];
const sidebarHoverPaintBlocks = cssRuleBlocksForMatchingSelectors(stylesForChecks, (selector) =>
  selector.includes(":hover") &&
  sidebarPointerScopeClasses.some((classSelector) => cssSelectorContainsClass(selector, classSelector))
);
for (const { block, selectorList } of sidebarHoverPaintBlocks) {
  if (
    hasNonTransparentBackground(block) ||
    hasNonzeroBorderDeclaration(block, "border") ||
    hasVisibleBorderDeclaration(block, "border-color") ||
    hasNonNoneDeclaration(block, "background-image")
  ) {
    fail(`sidebar hover must defer full-control paint to the unified pointer highlight: ${selectorList}`);
  }
}

const treePointerPseudoBlocks = cssRuleBlocksForMatchingSelectors(stylesForChecks, (selector) =>
  cssSelectorContainsClass(selector, ".tree-item") &&
  selector.includes('[data-interactive-pointer-target="true"]') &&
  (selector.includes("::before") || selector.includes("::after"))
);
if (treePointerPseudoBlocks.some(({ block }) => latestDeclarationValue(block, "filter") !== null)) {
  fail("folder rows must not alter the baseline unified pointer pseudo-elements with an extra filter");
}

const treeHoverContentSelectors = [
  ".tree-item:hover > .tree-disclosure",
  ".tree-item:hover > .tree-label",
  ".tree-item:hover > .tree-media-count"
];
for (const selector of treeHoverContentSelectors) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        latestDeclarationValue(block, "color") ===
          "color-mix(in srgb, var(--text-soft) 85%, var(--text-primary))" &&
        latestBackgroundValue(block) === null &&
        latestDeclarationValue(block, "filter") === null &&
        latestDeclarationValue(block, "box-shadow") === null &&
        latestDeclarationValue(block, "border") === null &&
        latestDeclarationValue(block, "border-color") === null
    )
  ) {
    fail(`${selector} must provide only a restrained foreground-color hover without adding another highlight surface`);
  }
}

for (const selector of [
  ".tree-item .tree-disclosure:active:not(:disabled)",
  ".tree-item .tree-label:active:not(:disabled)"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        latestBackgroundValue(block) === "transparent" &&
        latestDeclarationValue(block, "transform") === null
    )
  ) {
    fail(`${selector} must suppress only the pressed background while inheriting global press scale motion`);
  }
}

if (!settingsView.includes("settings-style-slider-control")) {
  fail("Interface style sliders must wrap native range inputs in a stylable pointer-highlight owner");
}

if (!settingsView.includes("data-settings-slider-control={id}")) {
  fail("Interface style slider visual owners must be addressable by slider id for pointer-highlight verification");
}

const settingsSliderControlBlocks = cssBlocksForExactSelector(stylesForChecks, ".settings-style-slider-control");
if (
  !settingsSliderControlBlocks.some(
    (block) =>
      /border:\s*1px\s+solid/.test(block) &&
      block.includes("var(--radius-pill)") &&
      block.includes("var(--glass-control)")
  )
) {
  fail("Interface style slider controls must reserve a 1px border box (transparent by default) so the local pointer highlight has a stroke to trace");
}

for (const functionName of [
  "updateInteractiveAffordancePointers"
]) {
  const body = extractFunctionBody(liquidGlassSurface, functionName);
  if (
    /const\s+edgePoint\s*=\s*nearestPointOnGlassEdge/.test(body) ||
    /\(\(edgePoint\.x\s*-\s*rect\.left\)/.test(body) ||
    /\(\(edgePoint\.y\s*-\s*rect\.top\)/.test(body)
  ) {
    fail(`${functionName} must not snap highlight coordinates to the nearest edge; use the real cursor position`);
  }
}

const glassBackdropBlocks = cssBlocksForExactSelector(stylesForChecks, ".liquid-glass-backdrop");
// The glass backdrop composites exactly like the settings window: a translucent
// fill plus a real backdrop-filter blur over the Electron acrylic.
if (!glassBackdropBlocks.some((block) => hasNonNoneDeclaration(block, "backdrop-filter"))) {
  fail("liquid glass backdrop must apply a real backdrop-filter blur over the window backdrop");
}

// It must NOT carry an isolating `filter:` (e.g. the retired SVG refraction). A
// filter renders the layer into an offscreen buffer and neutralizes
// backdrop-filter, so the frost stops sampling the acrylic backdrop — that is
// the "not layered the normal way" bug this replaced. (backdrop-filter and
// -webkit-backdrop-filter are allowed; a bare filter: is not.)
if (glassBackdropBlocks.some((block) => /(?<![\w-])filter\s*:\s*url\(/.test(block))) {
  fail("liquid glass backdrop must not apply an isolating filter: url() (breaks backdrop-filter compositing over acrylic)");
}

const glassStructureBorderBlocks = cssBlocksForExactSelector(stylesForChecks, ".liquid-glass::after");
if (
  !glassStructureBorderBlocks.some(
    (block) =>
      /border:\s*1px\s+solid\s+var\(--glass-border-current\)/.test(block) &&
      /inset\s+0\s+1px\s+0\s+var\(--glass-inner-light\)/.test(block) &&
      /inset\s+0\s+-1px\s+0\s+rgba\(255,\s*255,\s*255,\s*0\.06\)/.test(block)
  )
) {
  fail("liquid glass structural edge highlights must be drawn on the unfiltered border layer");
}

const glassLensBlocks = cssBlocksForExactSelector(stylesForChecks, ".liquid-glass-lens");
if (!glassLensBlocks.some((block) => /filter:\s*url\("#megle-liquid-glass-edge"\)/.test(block))) {
  fail("liquid glass lens layer must keep SVG edge/refraction filter active");
}

for (const value of [
  'id="megle-liquid-glass-refraction"',
  'id="megle-liquid-glass-edge"',
  "feDisplacementMap"
]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`liquid glass SVG refraction definitions missing ${value}`);
  }
}

const pointerHideBody = extractFirstRequiredFunctionBody(
  liquidGlassSurface,
  ["hideInteractiveAffordancePointers", "resetPointer"],
  "interactive affordance pointer hide/reset"
);
if (pointerHideBody.includes("--interactive-pointer-x") || pointerHideBody.includes("--interactive-pointer-y")) {
  fail("affordance pointer leave must hide opacity without resetting x/y to a visible center drift");
}

// The pointer highlight is a SINGLE global proximity system: one pointermove
// listener drives updateInteractiveAffordancePointers over affordance targets.
// (The former per-surface window-edge and liquid-glass-surface sweeps were
// unified away — the mouse glow lives only on interactive affordance targets.)
for (const value of [
  "useEffect",
  "window.addEventListener(\"pointermove\"",
  "window.addEventListener(\"mousemove\"",
  "GLASS_POINTER_EDGE_PROXIMITY_PX",
  "updateInteractiveAffordancePointers",
  "hideInteractiveAffordancePointers"
]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`liquid glass pointer highlight must be global/proximity-based: missing ${value}`);
  }
}

if (/onPointerMove\(event:[\s\S]*?activatePointer/.test(liquidGlassSurface)) {
  fail("liquid glass pointer tracking must not depend on per-surface pointer move handlers");
}

if (/onPointerLeave\(event:[\s\S]*?resetPointer/.test(liquidGlassSurface)) {
  fail("liquid glass pointer leave must not hide nearby surface highlights independently of global proximity");
}

if (/liquid-glass-tone-(primary|danger)[\s\S]*?--glass-pointer-light/.test(stylesForChecks)) {
  fail("liquid glass pointer highlight color/intensity must stay global instead of per-tone");
}

for (const value of [
  "ExternalLink",
  "Eye",
  "Pencil",
  "FolderInput",
  "Trash2",
  "RefreshCw",
  "Copy",
  "FolderOpen",
  "context-menu-separator",
  "Preview",
  "Rename",
  "Move to",
  "Move to recycle bin",
  "Delete permanently",
  "Refresh folder",
  "Copy path",
  "Reveal in Explorer"
]) {
  if (!app.includes(value) && !contextMenu.includes(value)) {
    fail(`context menu must expose Explorer-style management coverage: missing ${value}`);
  }
}

for (const value of ["copyText", "revealPath", "openPath", "DesktopShellActions"]) {
  if (!desktopBridge.includes(value)) {
    fail(`desktop bridge must expose safe shell action boundaries where available: missing ${value}`);
  }
}

for (const value of [
  "LiquidGlassLayer",
  "LiquidGlassSurface",
  "LiquidGlassButton",
  "scrollable?: boolean",
  "pressable?: boolean",
  "data-liquid-glass",
  "liquid-glass-backdrop",
  "liquid-glass-lens",
  "liquid-glass-content",
  "sharp child content layer"
]) {
  if (!liquidGlassSurface.includes(value) && !liquidGlassIndex.includes(value)) {
    fail(`liquid glass primitive source missing ${value}`);
  }
}

for (const value of [
  "liquid-glass-scrollable",
  ".liquid-glass.liquid-glass-scrollable",
  "overflow: hidden",
  ".liquid-glass.liquid-glass-scrollable > .liquid-glass-content",
  "overflow: auto"
]) {
  if (!stylesForChecks.includes(value) && !liquidGlassSurface.includes(value)) {
    fail(`scrollable liquid glass surfaces must keep chrome fixed and scroll inner content: missing ${value}`);
  }
}

for (const value of [
  "<filter",
  "feDisplacementMap",
  "feTurbulence",
  "feColorMatrix",
  "megle-liquid-glass-refraction"
]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`liquid glass primitive must define SVG refraction/displacement filter: missing ${value}`);
  }
}

for (const value of [
  "onPointerMove",
  "onPointerDown",
  "onPointerUp",
  "onPointerLeave",
  "data-glass-pressed",
  "target === currentTarget",
  "pressable"
]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`liquid glass primitive must handle pointer-driven illumination/refraction: missing ${value}`);
  }
}

for (const value of [
  'variant?: "regular" | "clear"',
  "liquid-glass-dim",
  'data-glass-variant="clear"',
  'tone?: "chrome" | "panel" | "elevated" | "control" | "primary" | "danger"'
]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`liquid glass primitive must encode regular/clear variants and selective tone usage: missing ${value}`);
  }
}

for (const value of [
  "prefers-reduced-transparency: reduce",
  "prefers-reduced-motion: reduce",
  "prefers-contrast: more",
  "forced-colors: active",
  ".liquid-glass-backdrop",
  ".liquid-glass[data-glass-interactive=\"true\"]"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`liquid glass CSS must include accessibility and interaction fallback: missing ${value}`);
  }
}

if (stylesForChecks.includes("animation-duration: 1ms")) {
  fail("reduced motion must disable nonessential animations instead of shortening them to 1ms");
}

for (const value of [
  "animation: none !important",
  "transition: none !important",
  "transform: none !important",
  ".spin"
]) {
  if (!stylesForChecks.includes(value)) {
    fail(`reduced motion must prevent shimmer/spin/liquid flicker: missing ${value}`);
  }
}

for (const token of ["--glass-refraction-scale", "--glass-chromatic-aberration"]) {
  if (stylesForChecks.includes(token)) {
    fail(`unused liquid glass SVG token must be wired or removed: ${token}`);
  }
}

for (const value of [
  'from "../design/liquid-glass"',
  'from "../../design/liquid-glass"'
]) {
  if (
    !app.includes(value) &&
    !windowChrome.includes(value) &&
    !read("apps/web/src/features/library/LibraryView.tsx").includes(value) &&
    !read("apps/web/src/features/library/LibrarySidebar.tsx").includes(value)
  ) {
    fail(`app surfaces must consume liquid-glass primitives: missing import ${value}`);
  }
}

if (sortMenu.includes('LiquidGlassSurface') && sortMenu.includes('as="ul"')) {
  fail('LiquidGlassSurface must not render as ul because injected material layers are invalid list children');
}

for (const value of [
  "LiquidGlassSurface",
  'className="inspector-tag-suggestions"',
  'role="listbox"',
  'role="option"'
]) {
  if (!inspectorMetadata.includes(value)) {
    fail(`tag suggestions popup must use LiquidGlassSurface and preserve listbox semantics: missing ${value}`);
  }
}

const ctaFiles = [
  ["RenameDialog", renameDialog, ["primary"]],
  ["MoveDialog", moveDialog, ["primary"]],
  ["DeleteConfirm", deleteConfirm, ["primary", "danger"]],
  ["OnboardingHero", onboardingHero, ["primary"]]
];

for (const [label, source, tones] of ctaFiles) {
  if (!source.includes("LiquidGlassButton")) {
    fail(`${label} primary/danger CTAs must use LiquidGlassButton`);
  }
  for (const tone of tones) {
    if (!source.includes(`tone="${tone}"`) && !source.includes(`"${tone}"`)) {
      fail(`${label} CTA must set LiquidGlassButton tone ${tone}`);
    }
  }
  const rawButtonTags = source.match(/<button\b[^>]*>/gs) ?? [];
  if (rawButtonTags.some((tag) => /(dialog-button-primary|dialog-button-danger|onboarding-hero-cta|grid-empty-action)/.test(tag))) {
    fail(`${label} must not render primary/danger CTA classes on raw button elements`);
  }
}

if (onboardingHero.includes("Welcome to Megle")) {
  fail("Onboarding empty state must not render the boxed Welcome to Megle header");
}

if (onboardingHero.includes("LiquidGlassSurface") || onboardingHero.includes("onboarding-hero-card")) {
  fail("Onboarding empty state must be direct center content, not a boxed LiquidGlassSurface card");
}

if (!onboardingHero.includes("Add a folder to get started")) {
  fail("Onboarding empty state must keep the Add a folder to get started prompt");
}

for (const selector of [
  ".app-shell",
  ".workspace",
  ".virtual-grid",
  ".media-tile",
  ".tile-thumb",
  ".preview-stage",
  ".central-preview-stage"
]) {
  const blocks = selector === ".app-shell"
    ? cssBlocksForExactSelector(stylesForChecks, selector)
    : cssBlocksForSelector(stylesForChecks, selector);
  if (blocks.length === 0) {
    fail(`content-layer selector missing ${selector}`);
    continue;
  }
  for (const block of blocks) {
    if (block.includes("backdrop-filter")) {
      fail(`${selector} must not use persistent backdrop-filter; keep media content sharp`);
    }
    if (
      [".app-shell", ".workspace", ".virtual-grid", ".central-preview-stage"].includes(selector) &&
      hasNonTransparentBackground(block)
    ) {
      fail(`${selector} must not paint a global gray backing plate`);
    }
    if (block.includes("liquid-glass-backdrop") || block.includes("data-liquid-glass")) {
      fail(`${selector} must not become a glass surface; glass belongs to controls/navigation`);
    }
  }
}

const gridSurfaceBlocks = cssBlocksForExactSelector(stylesForChecks, ".grid-surface");
if (gridSurfaceBlocks.length === 0) {
  fail("content-layer selector missing .grid-surface");
}

if (
  !gridSurfaceBlocks.some(
    (block) =>
      (/background:\s*transparent/.test(block) || /background:\s*var\(--content-stage\)/.test(block)) &&
      /border-radius:\s*0/.test(block) &&
      /padding:\s*0\s+16px/.test(block)
  )
) {
  fail("middle grid/workspace surface must keep a flat stage background with zero vertical padding and no rounded frame");
}

if (
  gridSurfaceBlocks.some(
    (block) =>
      hasNonTransparentBackground(block) &&
      !/background:\s*var\(--content-stage\)/.test(block)
  )
) {
  fail("middle grid/workspace surface must only use the shared content-stage background");
}

if (gridSurfaceBlocks.some((block) => /backdrop-filter\s*:/.test(block))) {
  fail("middle grid/workspace root must not use persistent backdrop-filter; material blur belongs on the LiquidGlass backdrop layer");
}

if (
  !libraryView.includes('className={previewMedia ? "grid-surface grid-surface-preview-active" : "grid-surface"}') ||
  !libraryView.includes('className="central-preview-overlay"')
) {
  fail("LibraryView center content column must keep the browsing grid mounted and use a preview overlay");
}
if (
  !/\.central-preview-overlay\s*\{[\s\S]*?background:\s*transparent;/.test(stylesForChecks) ||
  !/\.library-browser-layout--preview-covered\s*\{[\s\S]*?visibility:\s*hidden;/.test(stylesForChecks)
) {
  fail("Central preview overlay must preserve the transparent preview background while hiding the mounted browsing grid");
}

if (libraryView.includes("<LiquidGlassSurface") && libraryView.includes("grid-surface")) {
  fail("LibraryView center content stage must stay plain; the AppShell structural center column owns the LiquidGlass surface");
}

const workbenchColumnBlocks = cssBlocksForExactSelector(stylesForChecks, ".workbench-column");
if (
  !workbenchColumnBlocks.some(
    (block) =>
      /grid-template-rows:\s*var\(--shell-titlebar-height\)\s+minmax\(0,\s*1fr\)/.test(block) &&
      /overflow:\s*hidden/.test(block)
  )
) {
  fail("structural workbench columns must own the titlebar/body join with a single titlebar + body grid");
}

for (const selector of [
  ".workbench-column-left",
  ".workbench-column-center",
  ".workbench-column-right"
]) {
  if (
    !cssBlocksForSelector(stylesForChecks, selector).some((block) =>
      /--glass-fill:\s*var\(--glass-(?:side|center)-fill\)/.test(block) &&
      /--glass-blur-current:\s*var\(--glass-(?:side|center)-blur\)/.test(block)
    )
  ) {
    fail(`structural shell surface ${selector} must own a local Liquid Glass material`);
  }
}

const shellTitlebarBlocks = cssBlocksForExactSelector(stylesForChecks, ".shell-titlebar");
if (
  !shellTitlebarBlocks.some(
    (block) =>
      /border:\s*0/.test(block) &&
      /box-shadow:\s*none/.test(block) &&
      /background:\s*transparent/.test(block)
  )
) {
  fail("titlebar rows must be plain content inside the structural workbench columns, not separate glass surfaces");
}

const gridSurfaceAfterBlocks = cssBlocksForExactSelector(stylesForChecks, ".grid-surface::after");
if (gridSurfaceAfterBlocks.some((block) => latestDeclarationValue(block, "display") !== "none")) {
  fail("center content stage must not draw pseudo-borders around the preview area");
}

for (const selector of [".shell-titlebar", ".library-sidebar", ".grid-surface", ".inspector-panel"]) {
  const blocks = cssBlocksForSelector(stylesForChecks, selector);
  if (
    blocks.some(
      (block) =>
        block.includes("var(--shell-titlebar-height)") &&
        (/margin-top\s*:/.test(block) || /padding-top\s*:/.test(block))
    )
  ) {
    fail(`structural shell surface ${selector} must not use titlebar-height offset hacks`);
  }
  if (
    blocks.some(
      (block) =>
        /border-(?:top|bottom)-(?:color|width)\s*:/.test(block) ||
        hasVisibleBorderDeclaration(block, "border-top") ||
        hasVisibleBorderDeclaration(block, "border-bottom")
    )
  ) {
    fail(`structural shell surface ${selector} must not rely on artificial border suppression at the titlebar/body join`);
  }
}

const flatLibraryToolbarBlocks = cssBlocksForSelector(stylesForChecks, ".toolbar.toolbar-library");
if (!flatLibraryToolbarBlocks.some((block) => /border-radius:\s*0/.test(block))) {
  fail("compact library toolbar must stay flat instead of a rounded inner title bar");
}

const workspaceBlocks = cssBlocksForSelector(stylesForChecks, ".workspace");
if (workspaceBlocks.some((block) => /grid-template-rows:\s*56px/.test(block))) {
  fail("workspace layout must not reserve a horizontal per-view toolbar row");
}
if (
  workspaceBlocks.some(
    (block) => hasNonTransparentBackground(block) || hasNonNoneDeclaration(block, "box-shadow")
  )
) {
  fail("workspace layout must not paint a full-window gray backing plate");
}

const simpleWorkspaceBlocks = cssBlocksForSelector(stylesForChecks, ".simple-workspace");
if (simpleWorkspaceBlocks.some((block) => /grid-template-rows:\s*56px/.test(block))) {
  fail("simple workspace layout must not reserve a horizontal per-view toolbar row");
}
if (
  simpleWorkspaceBlocks.some(
    (block) => hasNonTransparentBackground(block) || hasNonNoneDeclaration(block, "box-shadow")
  )
) {
  fail("simple workspace layout must not paint a full-window gray backing plate");
}

const inspectorBlocks = cssBlocksForSelector(stylesForChecks, ".inspector-panel");
if (
  !inspectorBlocks.some(
    (block) =>
      /height:\s*100%/.test(block) &&
      /padding:\s*0\s+16px\s+16px/.test(block) &&
      /overflow-y:\s*auto/.test(block) &&
      /overflow-x:\s*hidden/.test(block) &&
      /background:\s*transparent/.test(block)
  )
) {
  fail("right inspector panel must fill the right structural column body, start preview at the top, scroll only vertically, and never expose horizontal scrolling");
}

for (const selector of [
  ".workbench-column-right",
  ".workbench-column-body-right",
  ".preview-panel",
  ".preview-stage",
  ".metadata-list",
  ".inspector-metadata",
  ".inspector-metadata-row",
  ".inspector-tag-stack",
  ".inspector-tag-input-row",
  ".inspector-note-stack",
  ".inspector-note"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        /min-width:\s*0/.test(block) &&
        /max-width:\s*100%/.test(block)
    )
  ) {
    fail(`${selector} must shrink within the right inspector column without causing horizontal scroll`);
  }
}

for (const selector of [".inspector-tag-input", ".inspector-note"]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => /width:\s*100%/.test(block) && /box-sizing:\s*border-box/.test(block))) {
    fail(`${selector} must size to the available inspector width`);
  }
}

const tagInputRowBlocks = cssBlocksForExactSelector(stylesForChecks, ".inspector-tag-input-row");
if (
  !tagInputRowBlocks.some(
    (block) =>
      /display:\s*grid/.test(block) &&
      /grid-template-columns:\s*minmax\(0,\s*1fr\)\s+auto/.test(block)
  )
) {
  fail("inspector tag input row must use a shrinking grid column for the input plus an auto action column");
}

const previewStageBlocks = cssBlocksForSelector(stylesForChecks, ".preview-stage");
if (
  !previewStageBlocks.some(
    (block) =>
      /background:\s*transparent/.test(block) &&
      /border:\s*0/.test(block) &&
      /border-radius:\s*var\(--radius-content\)/.test(block) &&
      /height:\s*auto/.test(block) &&
      /aspect-ratio:\s*1\s*\/\s*1/.test(block) &&
      /container-type:\s*size/.test(block) &&
      /place-items:\s*center/.test(block) &&
      /overflow:\s*hidden/.test(block) &&
      /padding:\s*0/.test(block)
  )
) {
  fail("right inspector preview stage must stay square, centered, transparent, rounded, borderless, and padding-free");
}

if (previewStageBlocks.some((block) => hasNonNoneDeclaration(block, "box-shadow"))) {
  fail("right inspector preview stage must not draw a visible box-shadow or frame");
}

if (/previewStageStyle|style=\{previewStageStyle/.test(previewPanel)) {
  fail("right inspector preview stage must not derive its container ratio from selected media dimensions");
}

for (const [source, value, message] of [
  [useLibraryData, 'sort: "name_asc"', "Library browsing must default to name A-Z sorting"],
  [useLibraryData, "canNavigateFolderBack", "Library data state must expose folder back-navigation availability"],
  [useLibraryData, "canNavigateFolderForward", "Library data state must expose folder forward-navigation availability"],
  [useLibraryData, "navigateFolderBack", "Library data state must expose a folder back-navigation action"],
  [useLibraryData, "navigateFolderForward", "Library data state must expose a folder forward-navigation action"],
  [shellTitlebar, "library-folder-back", "middle library titlebar must render a folder back button"],
  [shellTitlebar, "library-folder-forward", "middle library titlebar must render a folder forward button"],
  [app, "onNavigateFolderBack={library.navigateFolderBack}", "App must wire the titlebar folder back button to LibraryState"],
  [app, "onNavigateFolderForward={library.navigateFolderForward}", "App must wire the titlebar folder forward button to LibraryState"]
]) {
  if (!source.includes(value)) {
    fail(message);
  }
}

if (!/const\s+contentHeader\s*=\s*contentCount\s*>\s*0\s*\?/.test(libraryView)) {
  fail("LibraryView must hide the content section header when the current content area has no files");
}

const libraryBrowserContentHeaderBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".library-browser-content-header"
);
if (
  !libraryBrowserContentHeaderBlocks.some(
    (block) =>
      /min-height:\s*20px/.test(block) &&
      /padding:\s*0/.test(block)
  )
) {
  fail("Library content title header must use the same 20px text row spacing as the subfolder title header");
}

if (
  !mediaGrid.includes("CONTENT_HEADER_TOP_GAP") ||
  !/folderSectionVisible\s*\?\s*CONTENT_SECTION_HEADER_HEIGHT\s*:\s*SECTION_HEADER_HEIGHT/.test(mediaGrid) ||
  !/paddingTop:\s*folderSectionVisible\s*\?\s*CONTENT_HEADER_TOP_GAP\s*:\s*0/.test(mediaGrid)
) {
  fail("MediaGrid content title must reserve an expanded top gap after the subfolder area instead of crowding the folders");
}

if (/emptyContent=\{library\.loading\s*\?\s*null\s*:\s*renderEmptyState/.test(libraryView)) {
  fail("LibraryView must not render an empty content hint row inside the media browser");
}

if (!/subfolder-strip-title[\s\S]*subfolder-strip-heading-icon/.test(subfolderStrip)) {
  fail("Subfolder strip heading must place the collapse icon to the right of the title text");
}

const subfolderHeaderButtonBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".subfolder-strip-header-button"
);
if (
  !subfolderHeaderButtonBlocks.some(
    (block) =>
      /all:\s*unset/.test(block) &&
      /border:\s*0/.test(block) &&
      /background:\s*transparent/.test(block) &&
      /box-shadow:\s*none/.test(block) &&
      /outline:\s*0/.test(block)
  )
) {
  fail("Subfolder header controls must reset native/global button framing before applying text-only interaction");
}

const subfolderHeaderBlocks = cssBlocksForExactSelector(stylesForChecks, ".subfolder-strip-header");
if (
  !subfolderHeaderBlocks.some(
    (block) =>
      /border:\s*0/.test(block) &&
      /background:\s*transparent/.test(block) &&
      /box-shadow:\s*none/.test(block)
  )
) {
  fail("Subfolder strip header row must be text-only and never use a framed panel layout");
}

const virtualGridFocusBlocks = cssBlocksForExactSelector(stylesForChecks, ".virtual-grid:focus-visible");
if (!virtualGridFocusBlocks.some((block) => /box-shadow:\s*none/.test(block))) {
  fail("Media grid focus must not draw an inset outline that reads as a subfolder-row hover frame");
}

for (const selector of [".virtual-grid-section-header", ".virtual-grid-folder-header"]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        /border:\s*0/.test(block) &&
        /background:\s*transparent/.test(block) &&
        /box-shadow:\s*none/.test(block) &&
        /outline:\s*0/.test(block)
    )
  ) {
    fail(`${selector} must not paint a framed row around the subfolder/content header`);
  }
}

if (
  !stylesForChecks.includes(
    ".subfolder-strip .subfolder-strip-header-button:hover:not(:disabled)"
  )
) {
  fail("Subfolder strip header controls need a late scoped override so theme button hover styles cannot add frames");
}

for (const selector of [
  ".subfolder-strip-header-button:hover:not(:disabled)",
  ".subfolder-strip-header-button:active:not(:disabled)",
  ".subfolder-strip-header-button:focus-visible",
  ".subfolder-strip .subfolder-strip-header-button:hover:not(:disabled)",
  ".subfolder-strip .subfolder-strip-header-button:active:not(:disabled)",
  ".subfolder-strip .subfolder-strip-header-button:focus-visible"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        /border:\s*0/.test(block) &&
        /background:\s*transparent/.test(block) &&
        /border-radius:\s*0/.test(block) &&
        /box-shadow:\s*none/.test(block) &&
        /outline:\s*0/.test(block) &&
        /transform:\s*none/.test(block)
    )
  ) {
    fail(`${selector} must cancel the global button frame/scale and stay text-only`);
  }
}

for (const [selector, expectedWeight] of [
  [".subfolder-strip-title", "520"],
  [".library-browser-content-title", "520"]
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => latestDeclarationValue(block, "font-weight") === expectedWeight)) {
    fail(`${selector} must not use bold weight for Eagle-like section headings`);
  }
}

const subfolderTitleBlocks = cssBlocksForExactSelector(stylesForChecks, ".subfolder-strip-title");
if (!subfolderTitleBlocks.some((block) => /color:\s*var\(--text-muted\)/.test(block))) {
  fail("Subfolder heading text must start muted so hover has visible highlight feedback");
}

const subfolderHeadingHoverBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".subfolder-strip-heading-button:hover:not(:disabled) .subfolder-strip-title"
);
if (!subfolderHeadingHoverBlocks.some((block) => /color:\s*var\(--text-soft\)/.test(block))) {
  fail("Subfolder heading text must highlight on hover like the show-child-content label");
}

if (/subfolder-strip-heading-button:active\s+\.subfolder-strip-heading-icon[\s\S]*?transform:\s*scale/.test(stylesForChecks)) {
  fail("Subfolder collapse icon must not scale when toggling recursive child contents");
}

if (/library-browser-content-toggle:active\s+\.library-browser-content-toggle-indicator[\s\S]*?transform:\s*scale/.test(stylesForChecks)) {
  fail("Show child-folder-content checkbox must not scale when toggled");
}

for (const selector of [
  ".subfolder-strip-header-button",
  ".library-browser-content-toggle",
  ".library-browser-content-toggle-label"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => /line-height:\s*1/.test(block))) {
    fail(`${selector} must use a fixed line-height so checkbox and text stay vertically aligned`);
  }
}

if (/\.tile-thumb,\s*\.preview-stage\s*{[\s\S]*?background:/.test(stylesForChecks)) {
  fail("right inspector preview stage must not share the opaque tile thumbnail background");
}

if (!/className=\{selected \? `media-tile media-tile--\$\{layoutMode\} selected` : `media-tile media-tile--\$\{layoutMode\}`\}[\s\S]*data-interactive-pointer-target-selector="\.tile-thumb"/.test(mediaGrid)) {
  fail("content media tiles must route pointer hover outlines to the thumbnail surface instead of the full button/label box");
}

const rootBlocks = cssBlocksForExactSelector(stylesForChecks, ":root");
const libraryThumbnailInteractionTokens = [
  "--library-thumbnail-idle-ring",
  "--library-thumbnail-hover-ring",
  "--library-thumbnail-selected-ring",
  "--library-thumbnail-hover-shadow",
  "--library-thumbnail-selected-shadow",
  "--library-item-name-color",
  "--library-item-name-hover-color",
  "--library-item-name-selected-color"
];
for (const token of libraryThumbnailInteractionTokens) {
  if (!rootBlocks.some((block) => block.includes(token))) {
    fail(`library media and folder thumbnails must share the global ${token} style token`);
  }
}

if (!mediaGrid.includes("library-thumbnail-interaction-ring")) {
  fail("content media tiles must render the shared library-thumbnail-interaction-ring inside every thumbnail surface");
}

if (!subfolderStrip.includes("library-thumbnail-interaction-ring")) {
  fail("folder tiles must render the shared library-thumbnail-interaction-ring inside every folder cover surface");
}

if (
  !cssBlocksForExactSelector(stylesForChecks, ".library-thumbnail-interaction-ring").some(
    (block) =>
      /position:\s*absolute/.test(block) &&
      /inset:\s*0/.test(block) &&
      /border:\s*var\(--library-thumbnail-idle-ring\)/.test(block) &&
      /border-radius:\s*inherit/.test(block) &&
      /pointer-events:\s*none/.test(block)
  )
) {
  fail("library thumbnail interaction ring must be the single shared painted ring for media and folder thumbnails");
}

for (const selector of [
  ".media-tile:hover:not(:disabled)",
  ".media-tile:focus-visible",
  ".media-tile.selected"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (
    !blocks.some(
      (block) =>
        /border-color:\s*transparent/.test(block) &&
        /background:\s*transparent/.test(block) &&
        /box-shadow:\s*none/.test(block) &&
        /transform:\s*none/.test(block)
    )
  ) {
    fail(`${selector} must not draw hover or selected chrome around the media filename area`);
  }
}

if (
  !cssBlocksForExactSelector(stylesForChecks, ".media-tile.selected .library-thumbnail-interaction-ring").some(
    (block) => /border:\s*var\(--library-thumbnail-selected-ring\)/.test(block)
  )
) {
  fail("selected media tiles must draw the selected highlight only on the thumbnail surface");
}

if (
  !cssBlocksForExactSelector(
    stylesForChecks,
    ".media-tile.selected:hover:not(:disabled) .library-thumbnail-interaction-ring"
  ).some(
    (block) => /border:\s*var\(--library-thumbnail-selected-ring\)/.test(block)
  )
) {
  fail("selected media thumbnails must keep the selected ring when the selected tile is also hovered");
}

if (
  !cssBlocksForExactSelector(stylesForChecks, ".media-tile:hover:not(:disabled) .library-thumbnail-interaction-ring").some(
    (block) => /border:\s*var\(--library-thumbnail-hover-ring\)/.test(block)
  )
) {
  fail("hovered media tiles must use the shared library thumbnail hover tokens on the thumbnail surface");
}

if (
  !cssBlocksForExactSelector(
    stylesForChecks,
    ".subfolder-card-main:hover .library-thumbnail-interaction-ring"
  ).some(
    (block) => /border:\s*var\(--library-thumbnail-hover-ring\)/.test(block)
  )
) {
  fail("hovered subfolder tiles must use the shared library thumbnail hover tokens on the folder cover, not the name area");
}

if (
  !cssBlocksForExactSelector(stylesForChecks, ".subfolder-card.selected .subfolder-card-cover").some(
    (block) => /z-index:\s*3/.test(block) && /opacity:\s*1/.test(block)
  ) ||
  !cssBlocksForExactSelector(stylesForChecks, ".subfolder-card.selected .library-thumbnail-interaction-ring").some(
    (block) => /border:\s*var\(--library-thumbnail-selected-ring\)/.test(block)
  )
) {
  fail("selected subfolder tiles must draw the selected highlight only on the folder cover surface");
}

if (
  !cssBlocksForExactSelector(
    stylesForChecks,
    ".subfolder-card.selected .subfolder-card-main:hover .library-thumbnail-interaction-ring"
  ).some(
    (block) => /border:\s*var\(--library-thumbnail-selected-ring\)/.test(block)
  )
) {
  fail("selected folder thumbnails must keep the selected ring when the selected folder is also hovered");
}

for (const selector of [
  ".media-tile:hover:not(:disabled) .tile-thumb",
  ".media-tile.selected .tile-thumb",
  ".subfolder-card-main:hover .subfolder-card-cover",
  ".subfolder-card.selected .subfolder-card-cover"
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (blocks.some((block) => /border-color:\s*var\(--library-thumbnail/.test(block))) {
    fail(`${selector} must not paint ring width with surface border-color; use library-thumbnail-interaction-ring instead`);
  }
}

for (const [selector, expectedToken] of [
  [".tile-label", "--library-item-name-color"],
  [".subfolder-card-name", "--library-item-name-color"],
  [".media-tile.selected .tile-label", "--library-item-name-selected-color"],
  [".subfolder-card.selected .subfolder-card-name", "--library-item-name-selected-color"]
]) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
  if (!blocks.some((block) => new RegExp(`color:\\s*var\\(${expectedToken}\\)`).test(block))) {
    fail(`${selector} must use ${expectedToken} so thumbnail names are color-only and globally consistent`);
  }
}

if (
  !liquidGlassSurface.includes("INTERACTIVE_POINTER_TARGET_SELECTOR_ATTRIBUTE") ||
  !liquidGlassSurface.includes("interactiveAffordanceVisualTarget") ||
  !liquidGlassSurface.includes("querySelector<HTMLElement>")
) {
  fail("interactive pointer routing must support explicit visual targets for controls whose hit box differs from the painted surface");
}

const inspectorReadyPreviewBlocks = cssBlocksForSelector(stylesForChecks, ".preview-stage .preview-placeholder.ready");
if (!inspectorReadyPreviewBlocks.some((block) => /background:\s*transparent/.test(block))) {
  fail("right inspector ready preview must leave letterbox areas transparent");
}

if (!shellTitlebar.includes("titlebar-preview-title")) {
  fail("center preview titlebar must render the selected media name in titlebar-preview-title");
}

const centralPreviewStageBlocks = cssBlocksForSelector(stylesForChecks, ".central-preview-stage");
if (
  !centralPreviewStageBlocks.some(
    (block) =>
      /background:\s*transparent/.test(block) &&
      /border:\s*0/.test(block) &&
      /min-height:\s*0/.test(block) &&
      /place-items:\s*center/.test(block) &&
      /padding:\s*0/.test(block)
  )
) {
  fail("center preview stage must be a fixed transparent, centered, padding-free containment area with no black letterbox or border");
}

const centralPreviewStageFocusBlocks = cssBlocksForSelector(stylesForChecks, ".central-preview-stage:focus-visible");
if (
  centralPreviewStageFocusBlocks.some(
    (block) =>
      hasNonNoneDeclaration(block, "box-shadow") ||
      hasNonNoneDeclaration(block, "outline")
  )
) {
  fail("center preview stage focus must not draw a border-like inset ring over the image area");
}

const centralPreviewImageBlocks = cssBlocksForSelector(stylesForChecks, ".central-preview-stage .preview-image");
if (
  !centralPreviewImageBlocks.some(
    (block) =>
      /width:\s*auto/.test(block) &&
      /height:\s*auto/.test(block) &&
      /max-width:\s*none/.test(block) &&
      /max-height:\s*none/.test(block) &&
      /object-fit:\s*contain/.test(block)
  )
) {
  fail("center preview image must render at natural CSS size so transforms can switch accurately between 100% and fit-long-edge");
}

const centralPreviewTransformBlocks = cssBlocksForSelector(stylesForChecks, ".central-preview-transform");
if (centralPreviewTransformBlocks.length === 0) {
  fail("center preview transform wrapper must exist");
} else {
  if (centralPreviewTransformBlocks.some((block) => /(?:^|;)\s*width:\s*100%/.test(block) || /(?:^|;)\s*height:\s*100%/.test(block))) {
    fail("center preview transform wrapper must not use 100% stage dimensions; it must size to the media");
  }
  if (
    !centralPreviewTransformBlocks.some(
      (block) =>
        /position:\s*absolute/.test(block) &&
        /top:\s*50%/.test(block) &&
        /left:\s*50%/.test(block) &&
        /width:\s*(?:max-content|fit-content|auto)/.test(block) &&
        /height:\s*(?:max-content|fit-content|auto)/.test(block) &&
        /transform-origin:\s*top\s+left/.test(block)
    )
  ) {
    fail("center preview transform wrapper must be intrinsic-sized and explicitly anchored at the stage center");
  }
}

const centralReadyPreviewBlocks = cssBlocksForSelector(stylesForChecks, ".central-preview-stage .preview-placeholder.ready");
if (
  centralReadyPreviewBlocks.some((block) => /(?:^|;)\s*width:\s*100%/.test(block) || /(?:^|;)\s*height:\s*100%/.test(block))
) {
  fail("center ready preview wrapper must not stretch to 100% of the stage before transform");
}

const previewPanelImageBlocks = cssBlocksForSelector(stylesForChecks, ".preview-panel .preview-image");
const previewPanelStageImageBlocks = cssBlocksForSelector(
  stylesForChecks,
  ".preview-panel .preview-stage .preview-placeholder.ready > .preview-image"
);
if (
  !previewPanelImageBlocks.some(
    (block) =>
      /width:\s*auto/.test(block) &&
      /height:\s*auto/.test(block) &&
      /max-width:\s*100cqw/.test(block) &&
      /max-height:\s*100cqh/.test(block) &&
      /object-fit:\s*contain/.test(block) &&
      hasNonzeroBorderRadius(block)
  )
) {
  fail("right inspector preview media must size intrinsically, contain fully, stay centered, and use rounded media corners");
}

if (
  previewPanelStageImageBlocks.some(
    (block) =>
      /position:\s*absolute/.test(block) ||
      /inset:\s*0/.test(block) ||
      /width:\s*100%/.test(block) ||
      /height:\s*100%/.test(block)
  )
) {
  fail("right inspector preview media must not stretch absolutely across the square letterbox area");
}

const inspectorReadyPreviewExactBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".preview-panel .preview-stage .preview-placeholder.ready"
);
if (
  !inspectorReadyPreviewExactBlocks.some(
    (block) =>
      /display:\s*grid/.test(block) &&
      /place-items:\s*center/.test(block) &&
      /width:\s*100%/.test(block) &&
      /height:\s*100%/.test(block)
    )
) {
  fail("right inspector ready preview wrapper must center the intrinsic preview media inside the square stage");
}

if (!liquidGlassSurface.includes("GLASS_POINTER_EDGE_PROXIMITY_PX")) {
  fail("liquid glass pointer proximity must be based on nearby edges, not whole-surface distance");
}

for (const value of ["nearestPointOnGlassEdge", "distanceToGlassEdge"]) {
  if (!liquidGlassSurface.includes(value)) {
    fail(`liquid glass local edge highlight missing ${value}`);
  }
}

const glassPointerActiveBlocks = cssBlocksForSelector(stylesForChecks, '.liquid-glass[data-glass-pointer="active"]');
if (glassPointerActiveBlocks.some((block) => /--glass-border-current/.test(block))) {
  fail("liquid glass pointer hover must not switch the entire surface border to the strong color");
}

if (!process.exitCode) {
  console.log("PASS: UI liquid glass design boundaries");
}

function cssBlocksForSelector(source, selector) {
  const cssSource = stripCssComments(source);
  const blocks = [];
  let searchIndex = 0;
  while (searchIndex < cssSource.length) {
    const selectorIndex = cssSource.indexOf(selector, searchIndex);
    if (selectorIndex === -1) break;
    const blockStart = cssSource.indexOf("{", selectorIndex);
    if (blockStart === -1) break;
    const previousClose = cssSource.lastIndexOf("}", selectorIndex);
    const previousOpen = cssSource.lastIndexOf("{", selectorIndex);
    if (previousOpen > previousClose) {
      searchIndex = selectorIndex + selector.length;
      continue;
    }
    let depth = 0;
    for (let index = blockStart; index < cssSource.length; index += 1) {
      const char = cssSource[index];
      if (char === "{") depth += 1;
      if (char === "}") depth -= 1;
      if (depth === 0) {
        blocks.push(cssSource.slice(blockStart + 1, index));
        searchIndex = index + 1;
        break;
      }
    }
  }
  return blocks;
}

function cssBlocksForExactSelector(source, selector) {
  const cssSource = stripCssComments(source);
  const blocks = [];
  let searchIndex = 0;

  while (searchIndex < cssSource.length) {
    const blockStart = cssSource.indexOf("{", searchIndex);
    if (blockStart === -1) break;

    const blockEnd = findMatchingBraceIndex(cssSource, blockStart);
    if (blockEnd === -1) break;

    const selectorList = cssSource.slice(searchIndex, blockStart).trim();
    const blockBody = cssSource.slice(blockStart + 1, blockEnd);
    if (selectorList) {
      if (selectorList.startsWith("@")) {
        blocks.push(...cssBlocksForExactSelector(blockBody, selector));
      } else if (selectorListIncludesExactSelector(selectorList, selector)) {
        blocks.push(blockBody);
      }
    }

    searchIndex = blockEnd + 1;
  }

  return blocks;
}

function cssRuleBlocksForMatchingSelectors(source, predicate) {
  const cssSource = stripCssComments(source);
  const blocks = [];
  let searchIndex = 0;

  while (searchIndex < cssSource.length) {
    const blockStart = cssSource.indexOf("{", searchIndex);
    if (blockStart === -1) break;

    const blockEnd = findMatchingBraceIndex(cssSource, blockStart);
    if (blockEnd === -1) break;

    const selectorList = cssSource.slice(searchIndex, blockStart).trim();
    const blockBody = cssSource.slice(blockStart + 1, blockEnd);
    if (selectorList) {
      if (selectorList.startsWith("@")) {
        blocks.push(...cssRuleBlocksForMatchingSelectors(blockBody, predicate));
      } else if (splitCssSelectorList(selectorList).some((selector) => predicate(selector.trim()))) {
        blocks.push({ block: blockBody, selectorList, start: searchIndex, end: blockEnd + 1 });
      }
    }

    searchIndex = blockEnd + 1;
  }

  return blocks;
}

function assertSelectorNeverPaintsFullWindow(selector, { backgroundMessage, backdropMessage }) {
  const blocks = cssBlocksForExactSelector(stylesForChecks, selector);

  if (blocks.some((block) => hasNonTransparentBackground(block))) {
    fail(backgroundMessage);
  }

  if (
    blocks.some(
      (block) =>
        hasNonNoneDeclaration(block, "backdrop-filter") ||
        hasNonNoneDeclaration(block, "-webkit-backdrop-filter")
    )
  ) {
    fail(backdropMessage);
  }
}

function selectorListIncludesExactSelector(selectorList, selector) {
  return splitCssSelectorList(selectorList).some((candidate) => candidate.trim() === selector);
}

function assertMiddleTitlebarToolRulesDoNotFrame(blocks, messagePrefix) {
  for (const { block, selectorList } of blocks) {
    if (hasNonTransparentBackground(block)) {
      fail(`${messagePrefix} must not paint a background: ${selectorList}`);
    }
    if (hasNonzeroBorderDeclaration(block, "border")) {
      fail(`${messagePrefix} must use border: 0 instead of transparent borders: ${selectorList}`);
    }
    if (
      hasVisibleBorderDeclaration(block, "border-color") ||
      hasVisibleBorderDeclaration(block, "border-top") ||
      hasVisibleBorderDeclaration(block, "border-right") ||
      hasVisibleBorderDeclaration(block, "border-bottom") ||
      hasVisibleBorderDeclaration(block, "border-left")
    ) {
      fail(`${messagePrefix} must not draw a border: ${selectorList}`);
    }
    if (hasNonNoneDeclaration(block, "box-shadow")) {
      fail(`${messagePrefix} must not draw a box-shadow: ${selectorList}`);
    }
  }
}

function assertMiddleTitlebarFocusUsesInsetOutline() {
  for (const selector of [
    ".shell-titlebar-center .titlebar-tool-button:focus-visible",
    ".shell-titlebar-center .titlebar-icon-button:focus-visible",
    ".shell-titlebar-center .filter-menu-trigger:focus-visible",
    ".shell-titlebar-center .sort-menu-trigger:focus-visible"
  ]) {
    const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
    if (blocks.length === 0) {
      fail(`middle titlebar focus outline style missing ${selector}`);
    }

    const hasInsetOutline = blocks.some((block) => {
      const outlineOffset = latestDeclarationValue(block, "outline-offset");
      return outlineOffset !== null && Number.parseFloat(outlineOffset) < 0;
    });

    if (!hasInsetOutline) {
      fail(`${selector} must use a negative outline-offset so focus is not clipped by the titlebar toolbar`);
    }
  }
}

function assertMiddleTitlebarButtonsUseUnifiedGlow() {
  // Middle titlebar tool buttons are LiquidGlassButtons (data-liquid-glass="button")
  // and therefore affordance targets of the single circular pointer-distance glow —
  // they must NOT reintroduce a per-button pointer-glow plane (the retired
  // --glass-pointer-*/data-glass-pointer system).
  if (!liquidGlassSurface.includes('[data-liquid-glass="button"]')) {
    fail("titlebar tool buttons must be unified affordance targets: INTERACTIVE_AFFORDANCE_OWNER_SELECTOR must include [data-liquid-glass=\"button\"]");
  }
  if (stylesForChecks.includes('data-glass-pointer')) {
    fail("the retired data-glass-pointer surface-glow plane must not be referenced; titlebar buttons glow via the unified [data-interactive-pointer-target] system");
  }
  for (const deadVar of ["--glass-pointer-x", "--glass-pointer-y", "--glass-pointer-opacity"]) {
    if (stylesForChecks.includes(deadVar)) {
      fail(`retired surface-glow variable ${deadVar} must be removed from styles.css`);
    }
  }

  // The open menu / active tool keeps a steady readable backdrop (opacity 1 + a
  // real backdrop-filter blur) so it reads as selected even when the cursor's
  // glow is elsewhere. It composites like the settings frost — no isolating
  // filter: url() (that neutralized backdrop-filter over the acrylic).
  for (const selector of [
    '.shell-titlebar-center .titlebar-tool-button[data-glass-active="true"] > .liquid-glass-backdrop',
    '.shell-titlebar-center .sort-menu-trigger-open > .liquid-glass-backdrop'
  ]) {
    const blocks = cssBlocksForExactSelector(stylesForChecks, selector);
    if (
      !blocks.some(
        (block) =>
          latestDeclarationValue(block, "opacity") === "1" &&
          hasNonNoneDeclaration(block, "backdrop-filter")
      )
    ) {
      fail(`${selector} must keep a steady readable backdrop for the active/open titlebar tool`);
    }
  }
}

function isMiddleTitlebarToolRootSelector(selector, { includeUnscoped = false } = {}) {
  const trimmedSelector = selector.trim();
  if (!includeUnscoped && !cssSelectorContainsClass(trimmedSelector, ".shell-titlebar-center")) {
    return false;
  }

  const rootCompound = lastCssSelectorCompound(trimmedSelector);
  return middleTitlebarToolClassSelectors.some((classSelector) =>
    cssSelectorContainsClass(rootCompound, classSelector)
  );
}

function stripCssComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, "");
}

function findRetiredPresentationSelector(source) {
  const sourceWithoutComments = stripCssComments(source);
  const retiredSelectorPrefixes = [
    ".task-panel",
    ".task-card",
    ".task-drawer-",
    ".task-overlay-",
    ".task-center",
    ".task-row",
    ".task-table",
    ".task-filter-",
    ".task-progress",
    ".task-action",
    ".task-counters",
    ".task-list",
    ".task-copy",
    ".task-error",
    ".recent-ops-",
    ".recent-op-"
  ];
  for (const match of sourceWithoutComments.matchAll(/(?:^|[{}])\s*([^@{}][^{}]*)\{/g)) {
    const selectorPrelude = match[1];
    const retiredPrefix = retiredSelectorPrefixes.find((prefix) => selectorPrelude.includes(prefix));
    if (retiredPrefix) return retiredPrefix;
  }
  return null;
}

function lastCssSelectorCompound(selector) {
  let compoundStart = 0;
  let bracketDepth = 0;
  let parenDepth = 0;
  let quote = null;

  for (let index = 0; index < selector.length; index += 1) {
    const char = selector[index];
    const previous = selector[index - 1];

    if (quote) {
      if (char === quote && previous !== "\\") {
        quote = null;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }

    if (char === "[") bracketDepth += 1;
    if (char === "]") bracketDepth = Math.max(0, bracketDepth - 1);
    if (char === "(") parenDepth += 1;
    if (char === ")") parenDepth = Math.max(0, parenDepth - 1);

    if (bracketDepth === 0 && parenDepth === 0 && (/\s/.test(char) || char === ">" || char === "+" || char === "~")) {
      compoundStart = index + 1;
    }
  }

  return selector.slice(compoundStart).trim();
}

function cssSelectorContainsClass(selector, classSelector) {
  const pattern = new RegExp(`${escapeRegExp(classSelector)}(?=$|[^_a-zA-Z0-9-])`);
  return pattern.test(selector);
}

function splitCssSelectorList(selectorList) {
  const selectors = [];
  let current = "";
  let bracketDepth = 0;
  let parenDepth = 0;
  let quote = null;

  for (let index = 0; index < selectorList.length; index += 1) {
    const char = selectorList[index];
    const previous = selectorList[index - 1];

    if (quote) {
      current += char;
      if (char === quote && previous !== "\\") {
        quote = null;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      quote = char;
      current += char;
      continue;
    }

    if (char === "[") bracketDepth += 1;
    if (char === "]") bracketDepth = Math.max(0, bracketDepth - 1);
    if (char === "(") parenDepth += 1;
    if (char === ")") parenDepth = Math.max(0, parenDepth - 1);

    if (char === "," && bracketDepth === 0 && parenDepth === 0) {
      selectors.push(current);
      current = "";
      continue;
    }

    current += char;
  }

  selectors.push(current);
  return selectors;
}

function assertExactSelectorHelperContracts() {
  const probe = `
    .shell-drag * { -webkit-app-region: drag; }
    .shell-drag:hover { -webkit-app-region: drag; }
    .shell-drag .child { -webkit-app-region: drag; }
    .no-drag * { -webkit-app-region: no-drag; }
    .no-drag, [data-no-drag="true"] { -webkit-app-region: no-drag; }
    .split-pane { background: var(--secondary-window-pane-dark); border-right: 1px solid var(--line); }
    .split-pane { color: white; }
    .split-pane { border-right: 0; }
  `;

  if (
    cssBlocksForExactSelector(probe, ".shell-drag").some(
      (block) => latestDeclarationValue(block, "-webkit-app-region") === "drag"
    )
  ) {
    fail("exact selector helper must not accept descendant or pseudo-class .shell-drag rules");
  }

  if (
    !cssBlocksForExactSelector(probe, ".no-drag").some(
      (block) => latestDeclarationValue(block, "-webkit-app-region") === "no-drag"
    )
  ) {
    fail("exact selector helper must accept .no-drag as a standalone selector in a selector list");
  }

  const splitPaneBlocks = cssBlocksForExactSelector(probe, ".split-pane");
  if (
    latestDeclarationValueAcrossBlocks(splitPaneBlocks, "background") !==
      "var(--secondary-window-pane-dark)" ||
    latestDeclarationValueAcrossBlocks(splitPaneBlocks, "border-right") !== "0"
  ) {
    fail("cross-block declaration helper must preserve the latest non-null value in source order");
  }
}

function assertCssCommentStrippingContracts() {
  const probe = `
    /*
      --glass-readable-surface: rgb(255 255 255 / 0.3);
      --glass-pointer-fill-opacity: 0.2;
    */
    .comment-only-tint {
      /* background: var(--glass-readable-surface); */
      color: currentColor;
    }
    .comment-only-declarations {
      /* ignored; background: rgb(0 0 0 / 0.6); backdrop-filter: blur(16px); */
      color: currentColor;
    }
  `;

  if (cssCustomPropertyValue(probe, "--glass-readable-surface") !== null) {
    fail("CSS custom property helper must ignore comment-only token declarations");
  }

  if (cssVariableNumber(probe, "--glass-pointer-fill-opacity") !== null) {
    fail("CSS numeric variable helper must ignore comment-only token declarations");
  }

  if (
    cssBlocksForExactSelector(probe, ".comment-only-tint").some((block) =>
      declarationValueIncludes(block, "var(--glass-readable-surface)")
    )
  ) {
    fail("CSS block helpers must not return comment-only local tint evidence");
  }

  const declarationBlocks = cssBlocksForExactSelector(probe, ".comment-only-declarations");
  if (declarationBlocks.some((block) => latestBackgroundValue(block) !== null)) {
    fail("CSS declaration helpers must ignore commented background declarations");
  }

  if (declarationBlocks.some((block) => hasNonNoneDeclaration(block, "backdrop-filter"))) {
    fail("CSS declaration helpers must ignore commented backdrop-filter declarations");
  }
}

function assertRetiredPresentationSelectorContracts() {
  const allowed = `
    /* .task-card { color: red; } .recent-op-row { color: red; } */
    .notification-task-card { color: green; }
    [data-task-kind="scan"] { color: green; }
    .allowed-copy::before { content: ".task-card"; }
  `;
  if (findRetiredPresentationSelector(allowed) !== null) {
    fail("retired CSS selector guard must ignore comments and notification/task data classes");
  }
  for (const selector of [
    ".task-card",
    ".task-drawer-panel",
    ".task-overlay-backdrop",
    ".task-center-list",
    ".task-progress-bar",
    ".recent-ops-drawer",
    ".recent-op-row"
  ]) {
    if (findRetiredPresentationSelector(`${selector} { color: red; }`) === null) {
      fail(`retired CSS selector guard must reject ${selector}`);
    }
  }
}

function assertJsxStructuralHelperContracts() {
  const groupedSource = `
    const grouped = (
      <div className="plugins-sidebar compact">
        <div className="plugins-body-actions" />
        <div className="plugins-list" />
      </div>
    );
  `;
  const misplacedSource = `
    const misplaced = (
      <>
        <div className="plugins-sidebar"></div>
        <div className="plugins-body-actions" />
        <div className="plugins-list" />
      </>
    );
  `;
  const dataClassNameSource = `
    const dataClassName = (
      <div data-className="plugins-sidebar">
        <div className="plugins-body-actions" />
        <div className="plugins-list" />
      </div>
    );
  `;
  const requiredClassNames = ["plugins-body-actions", "plugins-list"];

  if (!jsxElementContainsClassDescendants(groupedSource, "grouped.tsx", "plugins-sidebar", requiredClassNames)) {
    fail("JSX structural helper must accept static className token lists");
  }
  if (
    jsxElementContainsClassDescendants(misplacedSource, "misplaced.tsx", "plugins-sidebar", requiredClassNames) ||
    jsxElementContainsClassDescendants(
      dataClassNameSource,
      "data-class-name.tsx",
      "plugins-sidebar",
      requiredClassNames
    )
  ) {
    fail("JSX structural helper must require plugin controls and list inside the sidebar element");
  }
}

function assertReturnedObjectMappingHelperContracts() {
  const requiredMappings = [
    ["--secondary-window-pane-dark", "secondaryWindowPaneDark"],
    ["--secondary-window-pane-light", "secondaryWindowPaneLight"],
    ["--secondary-window-pane-end", "secondaryWindowPaneEnd"]
  ];
  const validSource = `
    function interfaceStyleToCssVariables() {
      return {
        "--secondary-window-pane-dark": secondaryWindowPaneDark,
        "--secondary-window-pane-light": secondaryWindowPaneLight,
        "--secondary-window-pane-end": secondaryWindowPaneEnd
      };
    }
  `;
  const commentOnlySource = `
    function interfaceStyleToCssVariables() {
      // "--secondary-window-pane-dark": secondaryWindowPaneDark
      // "--secondary-window-pane-light": secondaryWindowPaneLight
      // "--secondary-window-pane-end": secondaryWindowPaneEnd
      return {};
    }
  `;
  const deadObjectSource = `
    function interfaceStyleToCssVariables() {
      const unused = {
        "--secondary-window-pane-dark": secondaryWindowPaneDark,
        "--secondary-window-pane-light": secondaryWindowPaneLight,
        "--secondary-window-pane-end": secondaryWindowPaneEnd
      };
      return {
        "--secondary-window-pane-dark": unrelatedPaneDark,
        "--secondary-window-pane-light": unrelatedPaneLight,
        "--secondary-window-pane-end": unrelatedPaneEnd
      };
    }
  `;

  if (
    !functionDirectReturnObjectMapsIdentifiers(
      validSource,
      "interfaceStyleToCssVariables",
      requiredMappings
    )
  ) {
    fail("returned-object mapping helper must accept identifier mappings in a direct object return");
  }
  if (
    functionDirectReturnObjectMapsIdentifiers(
      commentOnlySource,
      "interfaceStyleToCssVariables",
      requiredMappings
    ) ||
    functionDirectReturnObjectMapsIdentifiers(
      deadObjectSource,
      "interfaceStyleToCssVariables",
      requiredMappings
    )
  ) {
    fail("returned-object mapping helper must ignore comments and unused object literals");
  }
}

function assertMiddleTitlebarToolSelectorContracts() {
  for (const selector of [
    ".titlebar-tool-button:focus-visible",
    ".titlebar-icon-button[data-glass-active=\"true\"]",
    ".filter-menu-trigger:hover:not(:disabled)",
    ".titlebar-library-controls .sort-menu-trigger:hover",
    ".sort-menu-trigger:hover",
    ".sort-menu-trigger-open"
  ]) {
    if (!isMiddleTitlebarToolRootSelector(selector, { includeUnscoped: true })) {
      fail(`middle titlebar tool selector helper must catch unscoped matching selector ${selector}`);
    }
  }
}

function cssVariableNumber(source, variableName) {
  const cssSource = stripCssComments(source);
  const pattern = new RegExp(`${escapeRegExp(variableName)}:\\s*([0-9.]+)`);
  const match = cssSource.match(pattern);
  return match ? Number(match[1]) : null;
}

function cssCustomPropertyValue(source, variableName) {
  const cssSource = stripCssComments(source);
  const pattern = new RegExp(`${escapeRegExp(variableName)}:\\s*([^;]+)`);
  const match = cssSource.match(pattern);
  return match ? normalizeDeclarationValue(match[1]) : null;
}

function cssRgbaAlpha(value) {
  if (!value) return null;
  const normalized = value.trim().toLowerCase();
  if (normalized === "transparent" || normalized === "none") return 0;
  const commaMatch = normalized.match(/rgba?\(\s*[\d.]+\s*,\s*[\d.]+\s*,\s*[\d.]+\s*,\s*([\d.]+)\s*\)/);
  if (commaMatch) return Number(commaMatch[1]);
  const slashMatch = normalized.match(/rgb(?:a)?\([^)]*\/\s*([\d.]+%?)\s*\)/);
  if (slashMatch) {
    const raw = slashMatch[1];
    return raw.endsWith("%") ? Number(raw.slice(0, -1)) / 100 : Number(raw);
  }
  return null;
}

function positiveCssNumber(value) {
  return value !== null && Number.parseFloat(value) > 0;
}

function extractObjectBody(source, objectName) {
  const objectIndex = source.indexOf(objectName);
  if (objectIndex === -1) {
    return "";
  }

  const bodyStart = source.indexOf("{", objectIndex);
  if (bodyStart === -1) {
    return "";
  }

  const bodyEnd = findMatchingBraceIndex(source, bodyStart);
  if (bodyEnd === -1) {
    return "";
  }

  return source.slice(bodyStart + 1, bodyEnd);
}

function extractAtRuleBody(source, atRulePrefix) {
  const bodies = extractAtRuleBodies(source, atRulePrefix);
  return bodies[0]?.body ?? "";
}

function extractAtRuleBodies(source, atRulePrefix) {
  const bodies = [];
  let searchIndex = 0;

  while (searchIndex < source.length) {
    const start = source.indexOf(atRulePrefix, searchIndex);
    if (start === -1) {
      break;
    }

    const openBrace = source.indexOf("{", start);
    if (openBrace === -1) {
      break;
    }

    let depth = 0;
    for (let index = openBrace; index < source.length; index += 1) {
      const char = source[index];
      if (char === "{") depth += 1;
      if (char === "}") depth -= 1;
      if (depth === 0) {
        bodies.push({
          body: source.slice(openBrace + 1, index),
          start,
          end: index + 1
        });
        searchIndex = index + 1;
        break;
      }
    }

    if (searchIndex <= start) {
      break;
    }
  }

  return bodies;
}

function hasLaterDeclaration(source, startIndex, selector, property) {
  const laterSource = stripCssComments(source).slice(startIndex);
  return cssBlocksForSelector(laterSource, selector).some((block) =>
    propertyDeclarationPattern(property).test(block)
  );
}

function propertyDeclarationPattern(property) {
  return new RegExp(`(?:^|;)\\s*${escapeRegExp(property)}\\s*:`);
}

function hasNonTransparentBackground(block) {
  const value = latestBackgroundValue(block);
  if (value === null) {
    return false;
  }
  return !isTransparentBackgroundValue(value);
}

function latestBackgroundValue(block) {
  const body = stripCssComments(block);
  let latestValue = null;
  const pattern = /(?:^|;)\s*background(?:-color)?\s*:\s*([^;]+)/gi;
  let match;

  while ((match = pattern.exec(body)) !== null) {
    latestValue = normalizeDeclarationValue(match[1]);
  }

  return latestValue;
}

function latestDeclarationValue(block, property) {
  const body = stripCssComments(block);
  let latestValue = null;
  const pattern = new RegExp(`(?:^|;)\\s*${escapeRegExp(property)}\\s*:\\s*([^;]+)`, "gi");
  let match;

  while ((match = pattern.exec(body)) !== null) {
    latestValue = normalizeDeclarationValue(match[1]);
  }

  return latestValue;
}

function latestDeclarationValueAcrossBlocks(blocks, property) {
  let latestValue = null;
  for (const block of blocks) {
    const blockValue = latestDeclarationValue(block, property);
    if (blockValue !== null) {
      latestValue = blockValue;
    }
  }
  return latestValue;
}

function declarationValueIncludes(block, expectedValue) {
  const body = stripCssComments(block);
  const pattern = /(?:^|;)\s*[-_a-zA-Z0-9]+\s*:\s*([^;]+)/gi;
  let match;

  while ((match = pattern.exec(body)) !== null) {
    if (normalizeDeclarationValue(match[1]).includes(expectedValue)) {
      return true;
    }
  }

  return false;
}

function normalizeDeclarationValue(value) {
  return value.trim().replace(/\s*!important$/i, "");
}

function isTransparentBackgroundValue(value) {
  const normalized = value.trim().toLowerCase();
  return (
    normalized === "transparent" ||
    normalized === "none" ||
    normalized === "rgba(0, 0, 0, 0)" ||
    normalized === "rgb(0 0 0 / 0)" ||
    normalized === "#0000" ||
    normalized === "#00000000"
  );
}

function hasNonNoneDeclaration(block, property) {
  const value = latestDeclarationValue(block, property);
  if (value === null) {
    return false;
  }
  return value !== "none";
}

function hasNonzeroBorderDeclaration(block, property) {
  const value = latestDeclarationValue(block, property);
  if (value === null) {
    return false;
  }
  return !isZeroBorderValue(value) && value !== "none";
}

function isZeroBorderValue(value) {
  return /^0(?:\s|$)/.test(value.trim().toLowerCase());
}

function hasVisibleBorderDeclaration(block, property) {
  const body = stripCssComments(block);
  const pattern = new RegExp(`(?:^|;)\\s*${escapeRegExp(property)}\\s*:\\s*([^;]+)`, "i");
  const match = body.match(pattern);
  if (!match) {
    return false;
  }

  const value = normalizeDeclarationValue(match[1]).toLowerCase();
  return (
    !/^0(?:\s|$)/.test(value) &&
    value !== "none" &&
    !value.includes("transparent") &&
    !value.includes("#0000") &&
    !value.includes("#00000000") &&
    !/rgba\([^)]*,\s*0\s*\)/.test(value) &&
    !/rgb\([^)]*\/\s*0\s*\)/.test(value)
  );
}

function hasNonzeroBorderRadius(block) {
  const value = latestDeclarationValue(block, "border-radius");
  if (value === null) {
    return false;
  }
  return value !== "0";
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function extractJsxAttributeBodies(source, attributeName) {
  const bodies = [];
  const pattern = `${attributeName}={`;
  let searchIndex = 0;

  while (searchIndex < source.length) {
    const attributeIndex = source.indexOf(pattern, searchIndex);
    if (attributeIndex === -1) break;

    const bodyStart = attributeIndex + pattern.length;
    const bodyEnd = findMatchingBraceIndex(source, bodyStart - 1);
    if (bodyEnd === -1) break;

    bodies.push(source.slice(bodyStart, bodyEnd));
    searchIndex = bodyEnd + 1;
  }

  return bodies;
}

function extractJsxElements(source, componentName) {
  const elements = [];
  const openPattern = `<${componentName}`;
  const closePattern = `</${componentName}>`;
  let searchIndex = 0;

  while (searchIndex < source.length) {
    const start = source.indexOf(openPattern, searchIndex);
    if (start === -1) break;

    const openTagEnd = source.indexOf(">", start);
    if (openTagEnd === -1) break;

    const openTag = source.slice(start, openTagEnd + 1);
    if (/\/>\s*$/.test(openTag)) {
      elements.push(openTag);
      searchIndex = openTagEnd + 1;
      continue;
    }

    const end = source.indexOf(closePattern, openTagEnd + 1);
    if (end === -1) break;

    elements.push(source.slice(start, end + closePattern.length));
    searchIndex = end + closePattern.length;
  }

  return elements;
}

function jsxElementContainsClassDescendants(source, fileName, containerClassName, requiredClassNames) {
  const sourceFile = ts.createSourceFile(
    fileName,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX
  );
  let containsRequiredClasses = false;

  function hasStaticClassName(node, className) {
    const openingElement = ts.isJsxElement(node) ? node.openingElement : node;
    const classNameAttribute = openingElement.attributes.properties.find(
      (attribute) =>
        ts.isJsxAttribute(attribute) &&
        attribute.name.getText(sourceFile) === "className"
    );
    return Boolean(
      classNameAttribute?.initializer &&
        ts.isStringLiteral(classNameAttribute.initializer) &&
        classNameAttribute.initializer.text.split(/\s+/).includes(className)
    );
  }

  function visit(node) {
    if (containsRequiredClasses) return;
    if (ts.isJsxElement(node) && hasStaticClassName(node, containerClassName)) {
      const foundClassNames = new Set();
      function collect(descendant) {
        if (ts.isJsxElement(descendant) || ts.isJsxSelfClosingElement(descendant)) {
          for (const className of requiredClassNames) {
            if (hasStaticClassName(descendant, className)) {
              foundClassNames.add(className);
            }
          }
        }
        ts.forEachChild(descendant, collect);
      }
      ts.forEachChild(node, collect);
      containsRequiredClasses = requiredClassNames.every((className) => foundClassNames.has(className));
    }
    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
  return containsRequiredClasses;
}

function functionDirectReturnObjectMapsIdentifiers(source, functionName, requiredMappings) {
  const sourceFile = ts.createSourceFile(
    `${functionName}.ts`,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS
  );
  const functionDeclaration = sourceFile.statements.find(
    (statement) =>
      ts.isFunctionDeclaration(statement) &&
      statement.name?.text === functionName &&
      statement.body
  );
  if (!functionDeclaration?.body) return false;

  const returnStatement = functionDeclaration.body.statements.find(ts.isReturnStatement);
  if (!returnStatement?.expression || !ts.isObjectLiteralExpression(returnStatement.expression)) {
    return false;
  }

  const returnedMappings = new Map();
  for (const property of returnStatement.expression.properties) {
    if (
      ts.isPropertyAssignment(property) &&
      ts.isStringLiteral(property.name) &&
      ts.isIdentifier(property.initializer)
    ) {
      returnedMappings.set(property.name.text, property.initializer.text);
    }
  }
  return requiredMappings.every(
    ([propertyName, identifierName]) => returnedMappings.get(propertyName) === identifierName
  );
}

function extractFunctionBody(source, functionName) {
  const functionIndex = source.indexOf(`function ${functionName}`);
  if (functionIndex === -1) {
    return "";
  }

  const bodyStart = source.indexOf("{", functionIndex);
  if (bodyStart === -1) {
    return "";
  }

  const bodyEnd = findMatchingBraceIndex(source, bodyStart);
  if (bodyEnd === -1) {
    return "";
  }

  return source.slice(bodyStart + 1, bodyEnd);
}

function assertDesktopRevealOrderingContract() {
  for (const message of collectDesktopRevealOrderingFailures(desktopMainAst)) {
    fail(message);
  }
}

function assertDesktopRevealOrderingRegressionProbe() {
  const wrapperFallbackProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      const armFallback = () => armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const rearmFallback = () => armShellReadyFailureFallback(window);
        rearmFallback();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
      armShellReadyFailureFallback(window);
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const wrapperFallbackFailures = collectDesktopRevealOrderingFailures(wrapperFallbackProbe);
  if (wrapperFallbackFailures.length > 0) {
    fail("desktop reveal-order validator must treat const/arrow fallback wrappers as valid reachability paths");
  }

  const wrapperRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const revealLater = () => window.show();
        revealLater();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const wrapperRevealFailures = collectDesktopRevealOrderingFailures(wrapperRevealProbe);
  if (
    !wrapperRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch const/arrow reveal wrappers inside ready-to-show");
  }

  const objectWrapperFallbackProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      const helpers = { armFallback: () => armShellReadyFailureFallback(window) };
      window.once("ready-to-show", () => {
        const readyHelpers = {
          rearmFallback() {
            armShellReadyFailureFallback(window);
          }
        };
        readyHelpers.rearmFallback();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
      armShellReadyFailureFallback(window);
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const objectWrapperFallbackFailures = collectDesktopRevealOrderingFailures(objectWrapperFallbackProbe);
  if (objectWrapperFallbackFailures.length > 0) {
    fail("desktop reveal-order validator must treat object-member fallback wrappers as valid reachability paths");
  }

  const objectPropertyRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const helpers = { revealLater: () => window.show() };
        helpers.revealLater();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const objectPropertyRevealFailures = collectDesktopRevealOrderingFailures(objectPropertyRevealProbe);
  if (
    !objectPropertyRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch object-property reveal wrappers inside ready-to-show");
  }

  const objectMethodRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const helpers = {
          revealLater() {
            window.show();
          }
        };
        helpers.revealLater();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const objectMethodRevealFailures = collectDesktopRevealOrderingFailures(objectMethodRevealProbe);
  if (
    !objectMethodRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch object-method reveal wrappers inside ready-to-show");
  }

  const iifeRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        (() => window.show())();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const iifeRevealFailures = collectDesktopRevealOrderingFailures(iifeRevealProbe);
  if (
    !iifeRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch inline IIFE reveal wrappers inside ready-to-show");
  }

  const functionIifeRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        (function () {
          window.show();
        })();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const functionIifeRevealFailures = collectDesktopRevealOrderingFailures(functionIifeRevealProbe);
  if (
    !functionIifeRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch function IIFE reveal wrappers inside ready-to-show");
  }

  const deadNestedHandlerProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      const fakeReadyToShow = () => {
        window.once("ready-to-show", () => {
          armShellReadyFailureFallback(window);
        });
      };
      const fakeFailures = () => {
        window.webContents.on("did-fail-load", () => {
          revealMainWindowForLaunchFailure(window);
        });
        window.webContents.on("render-process-gone", () => {
          revealMainWindowForLaunchFailure(window);
        });
      };
      void fakeReadyToShow;
      void fakeFailures;
      armShellReadyFailureFallback(window);
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const deadNestedHandlerFailures = collectDesktopRevealOrderingFailures(deadNestedHandlerProbe);
  if (
    !deadNestedHandlerFailures.includes(
      "desktop startup must re-arm a bounded shell-ready failure fallback from the ready-to-show path"
    ) ||
    !deadNestedHandlerFailures.includes(
      "desktop startup failure handlers must reveal the window on did-fail-load"
    ) ||
    !deadNestedHandlerFailures.includes(
      "desktop startup failure handlers must reveal the window on render-process-gone"
    )
  ) {
    fail("desktop reveal-order validator must ignore dead nested handler registrations");
  }

  const bracketRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const reveal = { later: () => window.show() };
        reveal["later"]();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const bracketRevealFailures = collectDesktopRevealOrderingFailures(bracketRevealProbe);
  if (
    !bracketRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch bracket-notation object-member reveal wrappers inside ready-to-show");
  }

  const plainAliasFallbackProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      const armLater = armShellReadyFailureFallback;
      window.once("ready-to-show", () => {
        const rearmLater = armShellReadyFailureFallback;
        rearmLater(window);
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
      armLater(window);
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const plainAliasFallbackFailures = collectDesktopRevealOrderingFailures(plainAliasFallbackProbe);
  if (plainAliasFallbackFailures.length > 0) {
    fail("desktop reveal-order validator must treat plain const fallback aliases as valid reachability paths");
  }

  const plainAliasRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const revealLater = revealMainWindow;
        revealLater(window);
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindow(window) {
      window.show();
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const plainAliasRevealFailures = collectDesktopRevealOrderingFailures(plainAliasRevealProbe);
  if (
    !plainAliasRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch plain const reveal aliases inside ready-to-show");
  }

  const showInactiveAliasRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const revealLater = () => window.showInactive();
        revealLater();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const showInactiveAliasRevealFailures = collectDesktopRevealOrderingFailures(
    showInactiveAliasRevealProbe
  );
  if (
    !showInactiveAliasRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch showInactive reveal wrappers inside ready-to-show");
  }

  const bracketRevealMethodProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        const helpers = {
          revealLater() {
            window["showInactive"]();
          }
        };
        helpers["revealLater"]();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const bracketRevealMethodFailures = collectDesktopRevealOrderingFailures(
    bracketRevealMethodProbe
  );
  if (
    !bracketRevealMethodFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must catch static bracket-notation reveal-method calls inside ready-to-show");
  }

  const maximizeRevealProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      armShellReadyFailureFallback(window);
      window.once("ready-to-show", () => {
        (() => {
          window.maximize();
        })();
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const maximizeRevealFailures = collectDesktopRevealOrderingFailures(maximizeRevealProbe);
  if (
    !maximizeRevealFailures.includes(
      "ready-to-show handlers must not reveal the desktop window directly or through a helper"
    )
  ) {
    fail("desktop reveal-order validator must treat maximize as part of the reveal-method family");
  }

  const indirectHandlerRegistrationProbe = createSourceFileForDesktopRevealProbe(`
    async function createWindow() {
      const window = createMockWindow();
      const registerHandlers = () => {
        window.once("ready-to-show", () => {
          armShellReadyFailureFallback(window);
        });
        window.webContents.on("did-fail-load", () => {
          revealMainWindowForLaunchFailure(window);
        });
        window.webContents.on("render-process-gone", () => {
          revealMainWindowForLaunchFailure(window);
        });
      };
      registerHandlers();
      await window.loadURL("http://127.0.0.1:5173");
      armShellReadyFailureFallback(window);
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const indirectHandlerRegistrationFailures = collectDesktopRevealOrderingFailures(
    indirectHandlerRegistrationProbe
  );
  if (indirectHandlerRegistrationFailures.length > 0) {
    fail("desktop reveal-order validator must count handlers registered through called local helpers");
  }

  const lexicalShadowingProbe = createSourceFileForDesktopRevealProbe(`
    function revealMainWindow(window) {
      window.show();
    }
    async function createWindow() {
      const window = createMockWindow();
      window.once("ready-to-show", () => {
        armShellReadyFailureFallback(window);
        {
          const revealMainWindow = () => {
            return window;
          };
          revealMainWindow(window);
        }
      });
      window.webContents.on("did-fail-load", () => {
        revealMainWindowForLaunchFailure(window);
      });
      window.webContents.on("render-process-gone", () => {
        revealMainWindowForLaunchFailure(window);
      });
      await window.loadURL("http://127.0.0.1:5173");
      armShellReadyFailureFallback(window);
    }
    function armShellReadyFailureFallback(window) {
      return window;
    }
    function revealMainWindowForLaunchFailure(window) {
      window.show();
    }
  `);
  const lexicalShadowingFailures = collectDesktopRevealOrderingFailures(lexicalShadowingProbe);
  if (lexicalShadowingFailures.length > 0) {
    fail("desktop reveal-order validator must resolve same-name local shadowing lexically");
  }
}

function createSourceFileForDesktopRevealProbe(source) {
  return ts.createSourceFile(
    "desktop-reveal-probe.ts",
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS
  );
}

function collectDesktopRevealOrderingFailures(sourceFile) {
  const analysis = buildStartupValidatorAnalysis(sourceFile);
  if (!analysis.createWindowBinding) {
    return ["desktop startup contract missing createWindow"];
  }

  const failures = [];
  const { createWindowBinding, fallbackArmBinding, launchFailureBinding } = analysis;
  const parentMap = buildParentMap(createWindowBinding.node);
  const createWindowExecution = collectReachableExecutionData(
    createWindowBinding.body,
    createWindowBinding.closureScope,
    "createWindow",
    [],
    analysis
  );
  const loadUrlCalls = createWindowExecution.callRecords.filter(({ call }) =>
    isPropertyAccessCall(call, "loadURL")
  );
  const readyToShowHandlers = createWindowExecution.eventHandlers.filter(
    (handler) => handler.eventName === "ready-to-show"
  );
  const didFailLoadHandlers = createWindowExecution.eventHandlers.filter(
    (handler) => handler.eventName === "did-fail-load"
  );
  const renderProcessGoneHandlers = createWindowExecution.eventHandlers.filter(
    (handler) => handler.eventName === "render-process-gone"
  );

  if (loadUrlCalls.length === 0) {
    return ["desktop startup contract missing loadURL inside createWindow"];
  }

  const firstLoadUrlCall = loadUrlCalls[0].call;
  if (
    createWindowExecution.callRecords.some(
      (record) =>
        record.revealMethodName !== null &&
        !isAllowedDesktopStartupRevealRecord(
          record,
          launchFailureBinding,
          firstLoadUrlCall,
          sourceFile
        )
    )
  ) {
    failures.push("desktop window show timing must not run directly inside createWindow or its nested handlers");
  }

  const absoluteFallbackCall = createWindowExecution.callRecords.find(
    (record) =>
      fallbackArmBinding &&
      record.resolvedBinding?.id === fallbackArmBinding.id &&
      getCreateWindowCallContext(record.call, parentMap) === "createWindow" &&
      record.call.getStart(sourceFile) < firstLoadUrlCall.getStart(sourceFile)
  );
  if (absoluteFallbackCall) {
    failures.push("desktop startup must not arm the shell-ready failure fallback before loadURL gives the renderer a chance to run");
  }

  const postLoadFallbackCall = createWindowExecution.callRecords.find(
    (record) =>
      fallbackArmBinding &&
      record.resolvedBinding?.id === fallbackArmBinding.id &&
      getCreateWindowCallContext(record.call, parentMap) === "createWindow" &&
      record.call.getStart(sourceFile) > firstLoadUrlCall.getStart(sourceFile)
  );
  if (!postLoadFallbackCall) {
    failures.push("desktop startup must arm a shell-ready failure fallback after loadURL completes");
  }

  if (
    !readyToShowHandlers.some((handler) => {
      const handlerExecution = collectReachableExecutionData(
        handler.callback.body,
        handler.scope,
        "event:ready-to-show",
        [],
        analysis
      );
      return (
        fallbackArmBinding &&
        handlerExecution.callRecords.some(
          (record) => record.resolvedBinding?.id === fallbackArmBinding.id
        )
      );
    })
  ) {
    failures.push("desktop startup must re-arm a bounded shell-ready failure fallback from the ready-to-show path");
  }

  if (
    readyToShowHandlers.some((handler) =>
      collectReachableExecutionData(
        handler.callback.body,
        handler.scope,
        "event:ready-to-show",
        [],
        analysis
      ).callRecords.some((record) => record.revealMethodName !== null)
    )
  ) {
    failures.push("ready-to-show handlers must not reveal the desktop window directly or through a helper");
  }

  if (
    !didFailLoadHandlers.some((handler) => {
      const handlerExecution = collectReachableExecutionData(
        handler.callback.body,
        handler.scope,
        "event:did-fail-load",
        [],
        analysis
      );
      return (
        launchFailureBinding &&
        handlerExecution.callRecords.some(
          (record) => record.resolvedBinding?.id === launchFailureBinding.id
        )
      );
    })
  ) {
    failures.push("desktop startup failure handlers must reveal the window on did-fail-load");
  }

  if (
    !renderProcessGoneHandlers.some((handler) => {
      const handlerExecution = collectReachableExecutionData(
        handler.callback.body,
        handler.scope,
        "event:render-process-gone",
        [],
        analysis
      );
      return (
        launchFailureBinding &&
        handlerExecution.callRecords.some(
          (record) => record.resolvedBinding?.id === launchFailureBinding.id
        )
      );
    })
  ) {
    failures.push("desktop startup failure handlers must reveal the window on render-process-gone");
  }

  const unexpectedRevealCalls = createWindowExecution.callRecords.filter(
    (record) =>
      record.revealMethodName !== null &&
      !isAllowedDesktopStartupRevealRecord(
        record,
        launchFailureBinding,
        firstLoadUrlCall,
        sourceFile
      )
  );
  if (unexpectedRevealCalls.length > 0) {
    failures.push("desktop window reveal helpers must stay confined to shell-ready and explicit launch-failure paths");
  }

  return failures;
}

function isAllowedDesktopStartupRevealRecord(record, launchFailureBinding, firstLoadUrlCall, sourceFile) {
  if (
    launchFailureBinding &&
    record.context === "catch" &&
    record.bindingStack.includes(launchFailureBinding.id)
  ) {
    return true;
  }

  return (
    record.revealMethodName === "maximize" &&
    record.context === "createWindow" &&
    record.call.getStart(sourceFile) < firstLoadUrlCall.getStart(sourceFile)
  );
}

function buildStartupValidatorAnalysis(sourceFile) {
  const rootScope = createRuntimeScope(null);
  hoistFunctionDeclarationsIntoScope(sourceFile, rootScope);
  registerRuntimeVariableBindings(sourceFile, rootScope);

  return {
    rootScope,
    createWindowBinding: lookupScopeBinding(rootScope, "createWindow"),
    fallbackArmBinding: lookupScopeBinding(rootScope, "armShellReadyFailureFallback"),
    launchFailureBinding: lookupScopeBinding(rootScope, "revealMainWindowForLaunchFailure")
  };
}

function createRuntimeScope(parent) {
  return {
    parent,
    bindings: new Map()
  };
}

function createCallableBinding(name, node, body, closureScope) {
  return {
    id: `${name}:${node.pos}:${node.end}`,
    kind: "callable",
    name,
    node,
    body,
    closureScope
  };
}

function createNamespaceBinding(name) {
  return {
    id: `namespace:${name}`,
    kind: "namespace",
    name,
    members: new Map()
  };
}

function bindScopeBinding(scope, name, binding) {
  scope.bindings.set(name, binding);
}

function lookupScopeBinding(scope, name) {
  let current = scope;
  while (current) {
    if (current.bindings.has(name)) {
      return current.bindings.get(name);
    }
    current = current.parent;
  }
  return null;
}

function hoistFunctionDeclarationsIntoScope(container, scope) {
  for (const statement of directContainerStatements(container)) {
    if (ts.isFunctionDeclaration(statement) && statement.name?.text) {
      bindScopeBinding(
        scope,
        statement.name.text,
        createCallableBinding(statement.name.text, statement, statement.body, scope)
      );
    }
  }
}

function registerRuntimeVariableBindings(container, scope) {
  for (const statement of directContainerStatements(container)) {
    if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        registerVariableCallableBinding(declaration, scope);
      }
    }
  }
}

function directContainerStatements(container) {
  if (ts.isSourceFile(container) || ts.isBlock(container)) {
    return container.statements;
  }
  return [];
}

function registerVariableCallableBinding(declaration, scope) {
  if (!ts.isIdentifier(declaration.name) || !declaration.initializer) {
    return;
  }

  const bindingName = declaration.name.text;
  const initializer = declaration.initializer;

  if (ts.isArrowFunction(initializer) || ts.isFunctionExpression(initializer)) {
    bindScopeBinding(
      scope,
      bindingName,
      createCallableBinding(bindingName, initializer, initializer.body, scope)
    );
    return;
  }

  if (ts.isObjectLiteralExpression(initializer)) {
    bindScopeBinding(scope, bindingName, createObjectLiteralNamespaceBinding(bindingName, initializer, scope));
    return;
  }

  const aliasTarget = resolveBindingFromExpression(scope, initializer);
  if (aliasTarget) {
    bindScopeBinding(scope, bindingName, aliasTarget);
  }
}

function createObjectLiteralNamespaceBinding(name, objectLiteral, scope) {
  const namespaceBinding = createNamespaceBinding(name);

  for (const property of objectLiteral.properties) {
    const propertyName = objectLiteralPropertyName(property.name);
    if (!propertyName) {
      continue;
    }

    let memberBinding = null;
    if (
      ts.isPropertyAssignment(property) &&
      (ts.isArrowFunction(property.initializer) || ts.isFunctionExpression(property.initializer))
    ) {
      memberBinding = createCallableBinding(
        `${name}.${propertyName}`,
        property.initializer,
        property.initializer.body,
        scope
      );
    } else if (ts.isMethodDeclaration(property)) {
      memberBinding = createCallableBinding(
        `${name}.${propertyName}`,
        property,
        property.body,
        scope
      );
    } else if (ts.isPropertyAssignment(property) && ts.isObjectLiteralExpression(property.initializer)) {
      memberBinding = createObjectLiteralNamespaceBinding(
        `${name}.${propertyName}`,
        property.initializer,
        scope
      );
    } else if (ts.isPropertyAssignment(property)) {
      memberBinding = resolveBindingFromExpression(scope, property.initializer);
    }

    if (memberBinding) {
      namespaceBinding.members.set(propertyName, memberBinding);
    }
  }

  return namespaceBinding;
}

function resolveBindingFromExpression(scope, expression) {
  let current = expression;
  while (ts.isParenthesizedExpression(current)) {
    current = current.expression;
  }

  if (ts.isIdentifier(current)) {
    return lookupScopeBinding(scope, current.text);
  }

  if (ts.isPropertyAccessExpression(current)) {
    const target = resolveBindingFromExpression(scope, current.expression);
    return target?.kind === "namespace" ? target.members.get(current.name.text) ?? null : null;
  }

  if (ts.isElementAccessExpression(current)) {
    const target = resolveBindingFromExpression(scope, current.expression);
    const staticKey = staticElementAccessKey(current.argumentExpression);
    return target?.kind === "namespace" && staticKey !== null
      ? target.members.get(staticKey) ?? null
      : null;
  }

  return null;
}

function collectReachableExecutionData(container, parentScope, context, activeBindings, analysis) {
  const scope = createRuntimeScope(parentScope);
  hoistFunctionDeclarationsIntoScope(container, scope);

  const execution = {
    callRecords: [],
    eventHandlers: []
  };

  if (ts.isSourceFile(container) || ts.isBlock(container)) {
    for (const statement of container.statements) {
      visitExecutedNode(statement, scope, context, activeBindings, analysis, execution);
    }
  } else {
    visitExecutedNode(container, scope, context, activeBindings, analysis, execution, true);
  }

  return execution;
}

function visitExecutedNode(
  node,
  scope,
  context,
  activeBindings,
  analysis,
  execution,
  allowRootFunctionLike = false
) {
  if (!allowRootFunctionLike && ts.isFunctionLike(node)) {
    return;
  }

  if (ts.isVariableStatement(node)) {
    for (const declaration of node.declarationList.declarations) {
      visitExecutedNode(declaration, scope, context, activeBindings, analysis, execution);
    }
    return;
  }

  if (ts.isVariableDeclaration(node)) {
    if (node.initializer) {
      visitExecutedNode(node.initializer, scope, context, activeBindings, analysis, execution);
    }
    registerVariableCallableBinding(node, scope);
    return;
  }

  if (ts.isTryStatement(node)) {
    visitExecutedNode(node.tryBlock, scope, context, activeBindings, analysis, execution, true);
    if (node.catchClause) {
      const catchScope = createRuntimeScope(scope);
      if (node.catchClause.variableDeclaration?.name && ts.isIdentifier(node.catchClause.variableDeclaration.name)) {
        bindScopeBinding(catchScope, node.catchClause.variableDeclaration.name.text, null);
      }
      visitExecutedNode(node.catchClause.block, catchScope, "catch", activeBindings, analysis, execution, true);
    }
    if (node.finallyBlock) {
      visitExecutedNode(node.finallyBlock, scope, context, activeBindings, analysis, execution, true);
    }
    return;
  }

  if (ts.isBlock(node)) {
    const blockScope = createRuntimeScope(scope);
    hoistFunctionDeclarationsIntoScope(node, blockScope);
    for (const statement of node.statements) {
      visitExecutedNode(statement, blockScope, context, activeBindings, analysis, execution);
    }
    return;
  }

  if (ts.isCallExpression(node)) {
    const resolvedBinding = resolveBindingFromExpression(scope, node.expression);
    const callRecord = {
      call: node,
      resolvedBinding,
      context,
      bindingStack: [...activeBindings],
      revealMethodName: desktopRevealMethodName(node)
    };
    execution.callRecords.push(callRecord);

    const registration = matchEventRegistration(node);
    if (registration) {
      execution.eventHandlers.push({
        eventName: registration.eventName,
        callback: registration.callback,
        scope
      });
    }

    const inlineIife = immediatelyInvokedFunctionExpression(node);
    if (inlineIife) {
      mergeExecutionData(
        execution,
        collectReachableExecutionData(
          inlineIife.body,
          scope,
          context,
          activeBindings,
          analysis
        )
      );
    }

    if (
      resolvedBinding?.kind === "callable" &&
      !activeBindings.includes(resolvedBinding.id)
    ) {
      mergeExecutionData(
        execution,
        collectReachableExecutionData(
          resolvedBinding.body,
          resolvedBinding.closureScope,
          context,
          [...activeBindings, resolvedBinding.id],
          analysis
        )
      );
    }

    ts.forEachChild(node, (child) => {
      if (!ts.isFunctionLike(child)) {
        visitExecutedNode(child, scope, context, activeBindings, analysis, execution);
      }
    });
    return;
  }

  if (ts.isNewExpression(node)) {
    const promiseExecutor = promiseExecutorFunctionExpression(node);
    if (promiseExecutor) {
      mergeExecutionData(
        execution,
        collectReachableExecutionData(
          promiseExecutor.body,
          scope,
          context,
          activeBindings,
          analysis
        )
      );
    }
  }

  ts.forEachChild(node, (child) =>
    visitExecutedNode(child, scope, context, activeBindings, analysis, execution)
  );
}

function mergeExecutionData(target, source) {
  target.callRecords.push(...source.callRecords);
  target.eventHandlers.push(...source.eventHandlers);
}

function collectNamedFunctionData(sourceFile) {
  const functions = new Map();

  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text) {
      functions.set(node.name.text, createNamedFunctionData(node.name.text, node, node.body));
    } else if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.initializer &&
      (ts.isArrowFunction(node.initializer) || ts.isFunctionExpression(node.initializer))
    ) {
      functions.set(
        node.name.text,
        createNamedFunctionData(node.name.text, node.initializer, node.initializer.body)
      );
    } else if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.initializer &&
      !ts.isObjectLiteralExpression(node.initializer)
    ) {
      const aliasTargetName = callableExpressionName(node.initializer);
      if (aliasTargetName) {
        functions.set(
          node.name.text,
          createAliasFunctionData(node.name.text, node, aliasTargetName)
        );
      }
    } else if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.initializer &&
      ts.isObjectLiteralExpression(node.initializer)
    ) {
      for (const property of node.initializer.properties) {
        const propertyName = objectLiteralPropertyName(property.name);
        if (!propertyName) {
          continue;
        }

        if (
          ts.isPropertyAssignment(property) &&
          (ts.isArrowFunction(property.initializer) ||
            ts.isFunctionExpression(property.initializer))
        ) {
          functions.set(
            `${node.name.text}.${propertyName}`,
            createNamedFunctionData(
              `${node.name.text}.${propertyName}`,
              property.initializer,
              property.initializer.body
            )
          );
        } else if (ts.isMethodDeclaration(property)) {
          functions.set(
            `${node.name.text}.${propertyName}`,
            createNamedFunctionData(
              `${node.name.text}.${propertyName}`,
              property,
              property.body
            )
          );
        }
      }
    }

    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
  return functions;
}

function createNamedFunctionData(name, node, body) {
  const execution = collectExecutionData(body);
  return {
    name,
    node,
    directCallees: execution.calledNames,
    callsShow: execution.callsShow
  };
}

function createAliasFunctionData(name, node, targetName) {
  return {
    name,
    node,
    directCallees: new Set([targetName]),
    callsShow: false
  };
}

function collectTransitiveHelperNames(namedFunctions, { seedNames = [], predicate } = {}) {
  const helperNames = new Set(seedNames);

  if (predicate) {
    for (const [name, data] of namedFunctions.entries()) {
      if (predicate(data)) {
        helperNames.add(name);
      }
    }
  }

  let changed = true;
  while (changed) {
    changed = false;
    for (const [name, data] of namedFunctions.entries()) {
      if (helperNames.has(name)) {
        continue;
      }
      if ([...data.directCallees].some((callee) => helperNames.has(callee))) {
        helperNames.add(name);
        changed = true;
      }
    }
  }

  return helperNames;
}

function collectExecutionData(body) {
  const calledNames = new Set();
  const callExpressions = [];
  let callsShow = false;

  if (!body) {
    return { calledNames, callExpressions, callsShow };
  }

  function visit(current) {
    if (ts.isCallExpression(current)) {
      callExpressions.push(current);
      const calleeName = calledFunctionName(current);
      if (calleeName) {
        calledNames.add(calleeName);
      }
      if (isPropertyAccessCall(current, "show")) {
        callsShow = true;
      }
      const inlineIife = immediatelyInvokedFunctionExpression(current);
      if (inlineIife) {
        visit(inlineIife.body);
      }
    } else if (ts.isNewExpression(current)) {
      const promiseExecutor = promiseExecutorFunctionExpression(current);
      if (promiseExecutor) {
        visit(promiseExecutor.body);
      }
    }
    if (ts.isFunctionLike(current)) {
      return;
    }
    ts.forEachChild(current, visit);
  }

  visit(body);
  return { calledNames, callExpressions, callsShow };
}

function collectReachableHelperCalls(body, helperNames) {
  return collectExecutionData(body).callExpressions.filter((call) =>
    callsAnyHelper(call, helperNames)
  );
}

function callsAnyHelper(call, helperNames) {
  const calleeName = calledFunctionName(call);
  return Boolean(calleeName && helperNames.has(calleeName));
}

function collectEventHandlers(node, eventName) {
  return collectCallExpressions(
    node,
    (call) => matchEventRegistration(call)?.eventName === eventName
  ).flatMap((call) => {
    const registration = matchEventRegistration(call);
    return registration ? [{ call, callback: registration.callback }] : [];
  });
}

function collectReachableEventHandlers(body, eventName) {
  return collectExecutionData(body).callExpressions.flatMap((call) => {
    const registration = matchEventRegistration(call);
    return registration?.eventName === eventName ? [{ call, callback: registration.callback }] : [];
  });
}

function callbackContainsHelperCall(callback, helperNames) {
  return (
    collectReachableHelperCalls(callback.body, helperNames).length > 0 ||
    collectExecutionData(callback.body).callsShow
  );
}

function collectCallExpressions(node, predicate) {
  const matches = [];

  function visit(current, isRoot = false) {
    if (!isRoot && ts.isFunctionLike(current)) {
      return;
    }
    if (ts.isCallExpression(current) && predicate(current)) {
      matches.push(current);
    }
    ts.forEachChild(current, (child) => visit(child, false));
  }

  visit(node, true);
  return matches;
}

function buildParentMap(node) {
  const parentMap = new Map();

  function visit(current) {
    ts.forEachChild(current, (child) => {
      parentMap.set(child, current);
      visit(child);
    });
  }

  visit(node);
  return parentMap;
}

function getCreateWindowCallContext(call, parentMap) {
  let current = call;

  while (current) {
    if (ts.isCatchClause(current)) {
      return "catch";
    }

    if (ts.isArrowFunction(current) || ts.isFunctionExpression(current)) {
      const parent = parentMap.get(current);
      if (parent && ts.isCallExpression(parent)) {
        const registration = matchEventRegistration(parent);
        if (registration?.callback === current) {
          return `event:${registration.eventName}`;
        }
      }
    }

    current = parentMap.get(current);
  }

  return "createWindow";
}

function matchEventRegistration(call) {
  if (
    !ts.isPropertyAccessExpression(call.expression) ||
    (call.expression.name.text !== "on" && call.expression.name.text !== "once")
  ) {
    return null;
  }

  const [eventArg, callbackArg] = call.arguments;
  if (!eventArg || !ts.isStringLiteral(eventArg)) {
    return null;
  }
  if (
    !callbackArg ||
    (!ts.isArrowFunction(callbackArg) && !ts.isFunctionExpression(callbackArg))
  ) {
    return null;
  }

  return {
    eventName: eventArg.text,
    callback: callbackArg
  };
}

function calledFunctionName(call) {
  return callableExpressionName(call.expression);
}

function desktopRevealMethodName(call) {
  const memberName = staticMemberAccessName(call.expression);
  return memberName && isDesktopRevealMethodName(memberName) ? memberName : null;
}

function isDesktopRevealMethodName(name) {
  return name === "show" || name === "showInactive" || name === "maximize";
}

function staticMemberAccessName(expression) {
  let current = expression;
  while (ts.isParenthesizedExpression(current)) {
    current = current.expression;
  }

  if (ts.isPropertyAccessExpression(current)) {
    return current.name.text;
  }

  if (ts.isElementAccessExpression(current)) {
    return staticElementAccessKey(current.argumentExpression);
  }

  return null;
}

function isPropertyAccessCall(call, propertyName) {
  return ts.isPropertyAccessExpression(call.expression) && call.expression.name.text === propertyName;
}

function immediatelyInvokedFunctionExpression(call) {
  let expression = call.expression;

  while (ts.isParenthesizedExpression(expression)) {
    expression = expression.expression;
  }

  if (ts.isArrowFunction(expression) || ts.isFunctionExpression(expression)) {
    return expression;
  }

  return null;
}

function objectLiteralPropertyName(name) {
  if (!name) {
    return null;
  }
  if (ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name)) {
    return name.text;
  }
  return null;
}

function promiseExecutorFunctionExpression(expression) {
  if (
    !ts.isIdentifier(expression.expression) ||
    expression.expression.text !== "Promise"
  ) {
    return null;
  }
  const [executor] = expression.arguments;
  if (
    executor &&
    (ts.isArrowFunction(executor) || ts.isFunctionExpression(executor))
  ) {
    return executor;
  }
  return null;
}

function propertyAccessExpressionRoot(expression) {
  if (ts.isIdentifier(expression)) {
    return expression.text;
  }
  if (ts.isPropertyAccessExpression(expression)) {
    const parentRoot = propertyAccessExpressionRoot(expression.expression);
    if (!parentRoot) {
      return null;
    }
    return `${parentRoot}.${expression.name.text}`;
  }
  if (ts.isElementAccessExpression(expression)) {
    const parentRoot = propertyAccessExpressionRoot(expression.expression);
    const staticKey = staticElementAccessKey(expression.argumentExpression);
    if (!parentRoot || staticKey === null) {
      return null;
    }
    return `${parentRoot}.${staticKey}`;
  }
  return null;
}

function staticElementAccessKey(argumentExpression) {
  if (!argumentExpression) {
    return null;
  }
  if (ts.isStringLiteral(argumentExpression) || ts.isNumericLiteral(argumentExpression)) {
    return argumentExpression.text;
  }
  if (ts.isNoSubstitutionTemplateLiteral(argumentExpression)) {
    return argumentExpression.text;
  }
  return null;
}

function callableExpressionName(expression) {
  let current = expression;
  while (ts.isParenthesizedExpression(current)) {
    current = current.expression;
  }

  if (ts.isIdentifier(current)) {
    return current.text;
  }
  if (ts.isPropertyAccessExpression(current)) {
    const expressionRoot = propertyAccessExpressionRoot(current.expression);
    if (expressionRoot) {
      return `${expressionRoot}.${current.name.text}`;
    }
    return current.name.text;
  }
  if (ts.isElementAccessExpression(current)) {
    const expressionRoot = propertyAccessExpressionRoot(current.expression);
    const staticKey = staticElementAccessKey(current.argumentExpression);
    if (expressionRoot && staticKey !== null) {
      return `${expressionRoot}.${staticKey}`;
    }
  }
  return null;
}

function extractFirstRequiredFunctionBody(source, functionNames, contractName) {
  for (const functionName of functionNames) {
    const body = extractFunctionBody(source, functionName);
    if (body) {
      return body;
    }
  }

  fail(`${contractName} function missing; expected one of ${functionNames.join(", ")}`);
  return "";
}

function handlerSupportsKeyboardPreview(body) {
  return body.includes("onOpenPreview(") && hasEnterKeyCheck(body) && hasSpaceKeyCheck(body);
}

function previewStageStyleDerivesFromMediaDimensions(source) {
  const body = extractFunctionBody(source, "previewStageStyle");
  if (!body) {
    return false;
  }

  const returnIndex = body.indexOf("return");
  const returnObjectStart = returnIndex === -1 ? -1 : body.indexOf("{", returnIndex);
  const returnObjectEnd = returnObjectStart === -1 ? -1 : findMatchingBraceIndex(body, returnObjectStart);
  const returnObjectBody =
    returnObjectStart === -1 || returnObjectEnd === -1
      ? ""
      : body.slice(returnObjectStart + 1, returnObjectEnd);
  const aspectRatioMatch = returnObjectBody.match(/\baspectRatio\s*:\s*([^,\n]+)/);
  if (!aspectRatioMatch) {
    return false;
  }

  const aspectRatioExpression = aspectRatioMatch[1];
  return /\bmedia\.width\b/.test(aspectRatioExpression) && /\bmedia\.height\b/.test(aspectRatioExpression);
}

function hasEnterKeyCheck(body) {
  return /["']Enter["']/.test(body);
}

function hasSpaceKeyCheck(body) {
  return /["'] ["']|["']Space["']|["']Spacebar["']/.test(body);
}

function findMatchingBraceIndex(source, openBraceIndex) {
  let depth = 0;

  for (let index = openBraceIndex; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") depth += 1;
    if (char === "}") depth -= 1;
    if (depth === 0) {
      return index;
    }
  }

  return -1;
}

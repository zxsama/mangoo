import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function read(relativePath) {
  return readFileSync(path.join(root, relativePath), "utf8");
}

function readOptional(relativePath) {
  const absolutePath = path.join(root, relativePath);
  return existsSync(absolutePath) ? readFileSync(absolutePath, "utf8") : "";
}

function functionBody(source, signature) {
  const start = source.indexOf(signature);
  if (start === -1) {
    return null;
  }

  const openBrace = source.indexOf("{", start);
  if (openBrace === -1) {
    return null;
  }

  let depth = 0;
  for (let index = openBrace; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return source.slice(openBrace + 1, index);
      }
    }
  }

  return null;
}

const main = read("apps/desktop/src/main.ts");
const session = read("apps/desktop/src/core-session.ts");
const processFile = read("apps/desktop/src/core-process.ts");
const lifecycleQueue = read("apps/desktop/src/core-lifecycle-queue.ts");
const preload = read("apps/desktop/src/preload.cjs");
const notificationIpc = readOptional("apps/desktop/src/notification-ipc.ts");
const notificationRuntime = readOptional("apps/desktop/src/notification-runtime.ts");
const notificationCoreTransition = readOptional("apps/desktop/src/notification-core-transition.ts");
const notificationShutdown = readOptional("apps/desktop/src/notification-shutdown.ts");
const notificationChannels = readOptional("apps/desktop/src/notification-ipc-channels.cjs");
const notificationStore = readOptional("apps/desktop/src/notification-store.ts");
const desktopIpcPermissions = readOptional("apps/desktop/src/desktop-ipc-permissions.ts");
const secondaryWindowOpening = readOptional("apps/desktop/src/secondary-window-opening.ts");
const notificationContract = readOptional("packages/notification-contract/src/index.ts");
const webDesktopBridge = read("apps/web/src/core/desktop.ts");
const webMain = read("apps/web/src/main.tsx");
const secondaryReveal = read("apps/desktop/src/secondary-window-reveal.ts");
const desktopPackage = read("apps/desktop/package.json");
const rootPackage = read("package.json");
const devRunner = read("tools/dev/run-dev.mjs");

for (const value of [
  "startCoreProcess",
  "waitForCoreHealth",
  "createCoreSession",
  "AUTO_CORE_STARTUP_MAX_ATTEMPTS",
  "shouldRetryAutoCoreStartup",
  "handleDesktopStartupFailure",
  "CoreLifecycleQueue",
  "coreLifecycleQueue.enqueue",
  "coreLifecycleQueue.shutdown",
  "additionalArguments",
  "--megle-core-url=",
  "--megle-session-token=",
  "before-quit"
]) {
  if (!main.includes(value)) {
    fail(`desktop main missing ${value}`);
  }
}

for (const value of ["baseUrl", "bindAddress", "dbPath", "sessionToken", "MEGLE_CORE_ADDR"]) {
  if (!session.includes(value)) {
    fail(`core session missing ${value}`);
  }
}
if (session.includes("47321")) {
  fail("desktop core session must not default Electron-owned Core to fixed 127.0.0.1:47321");
}
for (const value of [
  "MEGLE_CORE_EXTERNAL",
  "MEGLE_CORE_URL is required when MEGLE_CORE_EXTERNAL=1",
  "MEGLE_SESSION_TOKEN is required when MEGLE_CORE_EXTERNAL=1",
  "normalizeLoopbackHost",
  "Unsupported Core host",
  "MEGLE_CORE_ADDR and MEGLE_CORE_URL must point to the same host/port",
  "isLoopbackOrIpLiteral"
]) {
  if (!session.includes(value)) {
    fail(`core session missing local/external endpoint guardrail: ${value}`);
  }
}

for (const value of [
  "MEGLE_CORE_EXTERNAL",
  "MEGLE_DB_PATH",
  "MEGLE_CORE_ADDR",
  "MEGLE_SESSION_TOKEN",
  "MEGLE_ALLOWED_ORIGIN",
  "health",
  "x-megle-session",
  "windowsHide",
  "repoRoot"
]) {
  if (!processFile.includes(value)) {
    fail(`core process orchestration missing ${value}`);
  }
}
const cargoRunCoreLaunchPattern =
  /spawn\(\s*[\s\S]*?,\s*\[\s*["']run["']\s*,\s*["']-p["']\s*,\s*["']megle-core["']\s*\]/;
if (cargoRunCoreLaunchPattern.test(processFile)) {
  fail("desktop internal Core launch must not use cargo run because it rewrites the Windows debug exe");
}
for (const value of ["coreBinaryPath", "target", "debug", "megle-core.exe", "fs.existsSync"]) {
  if (!processFile.includes(value)) {
    fail(`core process must spawn the prebuilt internal Core binary directly: ${value}`);
  }
}
for (const value of ["taskkill", "/T", "/F", "stopCoreProcessTree"]) {
  if (!processFile.includes(value)) {
    fail(`core process cleanup missing ${value}`);
  }
}

for (const value of ["contextBridge", "coreUrl", "sessionToken", "notifyShellReady", 'ipcRenderer.invoke("megle:shell-ready")']) {
  if (!preload.includes(value)) {
    fail(`preload bridge missing ${value}`);
  }
}

for (const value of [
  "notifySecondaryWindowReady",
  'ipcRenderer.invoke("megle:secondary-window-ready", role)'
]) {
  if (!preload.includes(value)) {
    fail(`preload bridge missing secondary renderer-ready contract: ${value}`);
  }
}

for (const value of [
  "openPluginsWindow",
  "closePluginsWindow",
  'ipcRenderer.invoke("megle:open-plugins-window")',
  'ipcRenderer.invoke("megle:close-plugins-window")'
]) {
  if (!preload.includes(value)) {
    fail(`preload bridge missing Plugins secondary-window contract: ${value}`);
  }
}

for (const value of [
  'type SecondaryWindowRole = "settings" | "plugins" | "notifications"',
  "SECONDARY_WINDOW_CONFIGS",
  "secondaryWindows",
  "secondaryWindowReadiness",
  "SECONDARY_WINDOW_READY_TIMEOUT_MS",
  'route: "#/settings"',
  'route: "#/plugins"',
  'route: "#/notifications"',
  'ipcMain.handle("megle:open-plugins-window"',
  'ipcMain.handle("megle:close-plugins-window"',
  'ipcMain.handle(NOTIFICATION_IPC_CHANNELS.openWindow',
  'ipcMain.handle(NOTIFICATION_IPC_CHANNELS.closeWindow',
  'ipcMain.handle("megle:secondary-window-ready"',
  "openSecondaryWindow",
  "closeSecondaryWindow",
  "closeAllSecondaryWindows"
]) {
  if (!main.includes(value)) {
    fail(`desktop secondary-window lifecycle missing ${value}`);
  }
}

const openSecondaryWindowBody = functionBody(main, "function openSecondaryWindow(role: SecondaryWindowRole): Promise<void>");
const createSecondaryWindowBody = functionBody(main, "async function createSecondaryWindow(");
const focusSecondaryWindowBody = functionBody(main, "function focusExistingSecondaryWindow(role: SecondaryWindowRole): boolean");
if (!openSecondaryWindowBody || !createSecondaryWindowBody || !focusSecondaryWindowBody) {
  fail("desktop must centralize Settings/Plugins/Notifications creation in openSecondaryWindow");
} else {
  for (const value of ["secondaryWindowOpenings.run", "await ensureCoreReady()", "focusExistingSecondaryWindow(role)", "createSecondaryWindow("]) {
    if (!openSecondaryWindowBody.includes(value)) {
      fail(`shared secondary window opening must preserve single-flight and post-Core recheck: ${value}`);
    }
  }
  for (const value of [
    "existing.isMinimized()",
    "existing.restore()",
    "readiness?.windowId === existing.id",
    "!readiness.revealState.settled",
    "existing.focus()"
  ]) {
    if (!focusSecondaryWindowBody.includes(value)) {
      fail(`shared secondary window focus must preserve existing-window behavior: ${value}`);
    }
  }
  for (const value of [
    "parent",
    "modal: false",
    'backgroundMaterial: "acrylic"',
    "opacity: 0",
    "frame: false",
    "secondaryWindows[role] = window",
    "revealState: createSecondaryWindowRevealState",
    "readiness.revealState.nativeReady = true",
    "SECONDARY_WINDOW_READY_TIMEOUT_MS",
    'window.once("ready-to-show"',
    "revealSecondaryWindowWhenReady(role, window)",
    "--megle-window=${role}",
    "config.route"
  ]) {
    if (!createSecondaryWindowBody.includes(value)) {
      fail(`shared secondary window must preserve the Settings lifecycle/material contract: ${value}`);
    }
  }
  for (const value of ["did-fail-load", "render-process-gone", "destroyFailedSecondaryWindow", "processLogSubscriptions.clearTarget", "readiness.revealState.settle(false)"]) {
    if (!main.includes(value)) {
      fail(`failed secondary windows must release role state and subscriptions: ${value}`);
    }
  }
  for (const value of ["class SecondaryWindowOpeningCoordinator", "this.openings", "token", "finally", "clear(role: Role, token: symbol)"]) {
    if (!secondaryWindowOpening.includes(value)) {
      fail(`secondary window opening coordinator missing ${value}`);
    }
  }
}

for (const value of [
  "notificationDatabasePath(userDataDir())",
  "createNotificationStore",
  "createNotificationService",
  "NotificationRuntime",
  "attachNotificationServiceForSession",
  "quiesceNotificationRuntime",
  "restoreNotificationsAfterFailedCoreStop",
  "runRecoverableNotificationShutdown",
  "closeNotificationRuntime",
  "closeNotifications: closeNotificationRuntime"
]) {
  if (!main.includes(value)) {
    fail(`desktop notification lifecycle missing ${value}`);
  }
}
if (!notificationStore.includes('"notifications.sqlite"')) {
  fail("desktop notification store must use the global notifications.sqlite path");
}

for (const value of [
  "windowIntent",
  "focusMedia",
  "focusMediaRequested",
  "compact",
  "history",
  "cancel",
  "retry",
  "reportError",
  "thumbnailSummary",
  "logsPage",
  "logsSubscribe",
  "logsUnsubscribe",
  "logsClear",
  "rendererLogs",
  "changed",
  "logsChanged"
]) {
  if (!notificationChannels.includes(value)) {
    fail(`central notification IPC channel table missing ${value}`);
  }
}

for (const value of [
  "canAccessNotificationIpc",
  "parseNotificationHistoryQuery",
  "parseNotificationAction",
  "parseRendererErrorReport",
  "parseThumbnailCacheSummary",
  "parseProcessLogFilter",
  "parseNotificationWindowIntent",
  "parseMediaFocusRequest",
  "parseRendererLogEntries",
  "LatestValueBroadcastQueue",
  "ProcessLogIpcSubscriptions"
]) {
  if (!notificationIpc.includes(value)) {
    fail(`notification IPC boundary missing ${value}`);
  }
}

for (const value of ["class NotificationRuntime", "start()", "attachLibrary", "detachLibrary", "close()"] ) {
  if (!notificationRuntime.includes(value)) {
    fail(`notification runtime missing ${value}`);
  }
}
for (const value of [
  "runNotificationSafeCoreTransition",
  "await options.detachNotifications()",
  "await options.stopOldCore()",
  "await options.startHealthyCore()",
  "await options.commitActiveLibrary(session)",
  "await options.attachNotifications(session)"
]) {
  if (!notificationCoreTransition.includes(value)) {
    fail(`notification-safe Core transition missing ${value}`);
  }
}
if (main.includes("scheduleNotificationServiceAttach")) {
  fail("notification attachment must stay inside the serialized Core lifecycle transaction");
}
if (!main.includes("runNotificationSafeCoreTransition")) {
  fail("desktop Core replacement must use the notification-safe transition boundary");
}

for (const value of [
  "getCompactSnapshot",
  "listHistory",
  "cancel",
  "retry",
  "reportError",
  "publishThumbnailSummary",
  "onChanged",
  "getHistoryFacets",
  "getLogs",
  "exportLogs",
  "snapshotAndSubscribeLogs",
  "clearLogs",
  "emitRendererLogs"
]) {
  if (!preload.includes(value)) {
    fail(`preload notification bridge missing ${value}`);
  }
  if (!notificationContract.includes(value)) {
    fail(`typed Web notification bridge missing ${value}`);
  }
}
for (const value of [
  "NOTIFICATION_IPC_CHANNELS.historyFacets",
  "NOTIFICATION_IPC_CHANNELS.logsExport",
  "dialog.showSaveDialog",
  "processLogBuffer.snapshot"
]) {
  if (!main.includes(value)) {
    fail(`desktop notification history/export boundary missing ${value}`);
  }
}
if (!preload.includes("notifications: notificationsBridge") ||
    !webDesktopBridge.includes("notifications?: DesktopNotificationsBridge")) {
  fail("preload and Web bridge must expose one nested notifications surface");
}
if (!webDesktopBridge.includes('from "@megle/notification-contract"') ||
    !notificationContract.includes("interface NotificationIpcBridge") ||
    notificationContract.includes("onLogs")) {
  fail("Desktop and Web must share the tokenized type-only notification IPC contract");
}
for (const value of [
  "subscriptionId",
  "ProcessLogSubscriptionStart",
  "ProcessLogSubscriptionEvent",
  "parseProcessLogSubscriptionId",
  "clearTarget"
]) {
  if (!notificationIpc.includes(value) && !notificationContract.includes(value)) {
    fail(`tokenized process log IPC missing ${value}`);
  }
}
if (preload.includes("onLogs:") ||
    !preload.includes("logsUnsubscribe, id") ||
    !preload.includes('windowRole === "notifications"')) {
  fail("preload must use one tokenized log protocol and a role-scoped notifications bridge");
}
for (const value of [
  "subscribeNotificationWindowIntent",
  "onNotificationWindowIntent",
  "onFocusMedia",
  "focusMedia"
]) {
  if (!preload.includes(value)) {
    fail(`notification drill-down preload bridge missing ${value}`);
  }
}
for (const value of [
  "NOTIFICATION_IPC_CHANNELS.windowIntent",
  "NOTIFICATION_IPC_CHANNELS.focusMedia",
  "NOTIFICATION_IPC_CHANNELS.focusMediaRequested",
  "coreLogHandlerForSession"
]) {
  if (!main.includes(value)) {
    fail(`notification drill-down Main integration missing ${value}`);
  }
}
for (const value of [
  "canAccessDesktopIpc",
  "PRIVILEGED_WINDOW_ROLES",
  '"megle:switch-library"',
  '"megle:shell-open-path"'
]) {
  if (!desktopIpcPermissions.includes(value)) {
    fail(`desktop sender-role allowlist missing ${value}`);
  }
}
for (const channel of [
  "megle:switch-library",
  "megle:create-library",
  "megle:remove-library-from-list",
  "megle:delete-library-file",
  "megle:shell-reveal-path",
  "megle:shell-open-path"
]) {
  if (!main.includes(`requireDesktopIpcAccess(event, "${channel}")`)) {
    fail(`desktop Main must authorize the real sender before ${channel}`);
  }
}
for (const value of [
  "await options.quiesceNotifications()",
  "await options.stopCore()",
  "await options.restoreNotifications()",
  "await options.closeNotifications()"
]) {
  if (!notificationShutdown.includes(value)) {
    fail(`recoverable notification shutdown missing ${value}`);
  }
}
if (preload.includes('require("./notification-ipc-channels.cjs")') ||
    !desktopPackage.includes("channels+'\\\\n'+preload")) {
  fail("sandboxed preload must bundle the centralized notification channels without a local require");
}

if (!webMain.includes('windowRole === "notifications"') ||
    !webMain.includes('import("./features/notifications/NotificationWindow")')) {
  fail("web entry must lazily route the standalone NotificationWindow");
}

if (/from\s+["']node:fs["']|require\(["']node:fs["']\)/.test(webDesktopBridge)) {
  fail("Web notification bridge must not import node:fs");
}

if (main.includes('window.once("ready-to-show", () => window.show())')) {
  fail("secondary windows must not reveal from ready-to-show before renderer/native warm-up readiness");
}
for (const value of [
  "const secondaryWindowPositions",
  "restoreSecondaryWindowPositions(state.secondaryWindows)",
  "const position = resolveSecondaryWindowPosition(role, parent, config)",
  "x: position.x",
  "y: position.y",
  "rememberSecondaryWindowPosition(role, window)",
  "secondaryWindows: readSecondaryWindowPositions(parsed.secondaryWindows)"
]) {
  if (!main.includes(value)) {
    fail(`secondary windows must restore and persist their desktop position: ${value}`);
  }
}
for (const value of [
  'ipcMain.handle("megle:secondary-window-ready", async (event, role: unknown)',
  "isSecondaryWindowRole(role)",
  "windowForIpcEvent(event)",
  "secondaryWindows[role] !== window",
  "readiness.revealState.rendererReady = true",
  "return readiness.revealState.result"
]) {
  if (!main.includes(value)) {
    fail(`secondary renderer-ready IPC must stay role/sender isolated: ${value}`);
  }
}
const revealSecondaryWindowBody = functionBody(
  main,
  "function revealSecondaryWindowWhenReady("
);
if (
  !revealSecondaryWindowBody ||
  !revealSecondaryWindowBody.includes("readiness.revealState.beginWarmup()") ||
  !revealSecondaryWindowBody.includes("warmSecondaryWindowNativeHost(role, window, readiness, warmupToken)")
) {
  fail("secondary reveal gate must start one cancellable native opacity warm-up after native+renderer readiness");
}

const warmSecondaryWindowBody = functionBody(
  main,
  "async function warmSecondaryWindowNativeHost("
);
const invisibleShowIndex = warmSecondaryWindowBody?.indexOf("window.showInactive()") ?? -1;
const warmupWaitIndex =
  warmSecondaryWindowBody?.indexOf("await waitForSecondaryNativeWindowWarmup(") ?? -1;
const nativeRevealTimestampIndex =
  warmSecondaryWindowBody?.indexOf("readiness.nativeWindowRevealedAt = Date.now()") ?? -1;
const nativeRevealIndex =
  warmSecondaryWindowBody?.indexOf("revealSecondaryNativeWindow(") ?? -1;
if (
  !warmSecondaryWindowBody ||
  invisibleShowIndex < 0 ||
  warmupWaitIndex <= invisibleShowIndex ||
  nativeRevealTimestampIndex <= warmupWaitIndex ||
  nativeRevealIndex <= nativeRevealTimestampIndex ||
  !warmSecondaryWindowBody.includes("waitForRendererFrame(window)") ||
  !warmSecondaryWindowBody.includes("NATIVE_WINDOW_WARMUP_MS") ||
  !warmSecondaryWindowBody.includes("fallback: false") ||
  warmSecondaryWindowBody.includes("window.setOpacity(1)")
) {
  fail("secondary reveal must show at opacity zero, finish native warm-up, then reveal through the shared helper");
}

if (
  !/export const NATIVE_WINDOW_WARMUP_MS = 100;/.test(secondaryReveal) ||
  !secondaryReveal.includes("state.cancellation") ||
  !secondaryReveal.includes("clearTimeout(timer)") ||
  !secondaryReveal.includes("state.isWarmupCurrent(token)")
) {
  fail("secondary native window warm-up must be a cancellable 100ms hold with stale-token protection");
}

const revealNativeWindowBody = functionBody(
  secondaryReveal,
  "export function revealSecondaryNativeWindow<"
);
const opacityRevealIndex = revealNativeWindowBody?.indexOf("window.setOpacity(1)") ?? -1;
const settleRevealIndex = revealNativeWindowBody?.indexOf("state.settle(result)") ?? -1;
const focusRevealIndex = revealNativeWindowBody?.indexOf("window.focus()") ?? -1;
if (
  !revealNativeWindowBody ||
  opacityRevealIndex < 0 ||
  settleRevealIndex <= opacityRevealIndex ||
  focusRevealIndex <= settleRevealIndex ||
  (secondaryReveal.match(/window\.setOpacity\(1\)/g) ?? []).length !== 1
) {
  fail("secondary native reveal must set opacity once before settling ready IPC and focusing");
}

const forceSecondaryWindowBody = functionBody(
  main,
  "function forceSecondaryWindowReveal("
);
if (
  !forceSecondaryWindowBody ||
  !forceSecondaryWindowBody.includes("clearSecondaryWindowReadyFallback(role, window.id)") ||
  !forceSecondaryWindowBody.includes("if (!window.isVisible()) window.showInactive()") ||
  !forceSecondaryWindowBody.includes("fallback: true") ||
  !forceSecondaryWindowBody.includes("revealSecondaryNativeWindow(")
) {
  fail("secondary fallback must show, restore opacity, settle ready IPC, and focus through the shared reveal helper");
}

if (
  main.includes("compositionPromise") ||
  main.includes("revealWaiters") ||
  main.includes("resolveSecondaryWindowRevealWaiters")
) {
  fail("secondary ready IPC must not split completion between composition promises and reveal waiters");
}

if (!main.includes("readiness.revealState.settle(false)")) {
  fail("closing a secondary window must resolve ready IPC false and cancel warm-up");
}

if (!main.includes("BrowserWindow.fromWebContents(event.sender)")) {
  fail("window-control IPC must target the invoking BrowserWindow instead of hard-coding the main window");
}
for (const legacyMainWindowControl of [
  "mainWindow?.minimize()",
  "mainWindow?.close()",
  "mainWindow?.isMaximized()",
  "mainWindow?.getBounds()"
]) {
  if (main.includes(legacyMainWindowControl)) {
    fail(`secondary windows must not route controls through mainWindow: ${legacyMainWindowControl}`);
  }
}

for (const script of ['"typecheck"', '"test"']) {
  if (!desktopPackage.includes(script)) fail(`desktop package missing ${script} script`);
}
if (desktopPackage.includes("vite --host")) {
  fail("desktop package dev script must not start Vite directly; use root dev runner");
}
if (!rootPackage.includes('"check:desktop"')) {
  fail("root package missing check:desktop script");
}
if (!rootPackage.includes('"dev"')) {
  fail("root package missing dev script");
}

for (const value of ["@megle/web", "@megle/desktop", "MEGLE_WEB_URL", "MEGLE_DB_PATH", "electron"]) {
  if (!devRunner.includes(value)) {
    fail(`dev runner missing ${value}`);
  }
}
for (const value of ["--port", "5173", "--strictPort"]) {
  if (!devRunner.includes(value)) {
    fail(`dev runner must start Vite with explicit ${value}`);
  }
}
for (const value of ["MEGLE_CORE_EXTERNAL", "findCargoCommand", "cargo", "build", "-p", "megle-core"]) {
  if (!devRunner.includes(value)) {
    fail(`dev runner must build internal Core before Electron startup: ${value}`);
  }
}
if (!/run\(\s*findCargoCommand\(\)\s*,\s*\[\s*"build"\s*,\s*"-p"\s*,\s*"megle-core"\s*\]\s*\)/.test(devRunner)) {
  fail("dev runner must use cargo build -p megle-core instead of leaving Core build to Electron startup");
}

// Core is started from exactly two call sites: the initial cold-start/retry
// loop in startCoreSession, and the library-switch restart routine in
// restartCoreForLibrary (which stops the old process and reuses the same
// bind address + session token per spec Decision 6). Any other count means a
// stray/duplicate startup path snuck in.
const startCoreCalls = [...main.matchAll(/\bstartCoreProcess\(/g)].length;
if (startCoreCalls !== 2) {
  fail(
    `desktop main must start Core from exactly two call sites (startCoreSession + restartCoreForLibrary); found ${startCoreCalls} calls`
  );
}
if (processFile.includes('process.stdout.write("[core stdout] ")') || processFile.includes('process.stderr.write("[core stderr] ")')) {
  fail("Core output must use the line-based desktop logger path without duplicate raw chunk console writes");
}
const lifecycleEnqueueCalls = [...main.matchAll(/coreLifecycleQueue\.enqueue\(/g)].length;
if (lifecycleEnqueueCalls < 4) {
  fail("desktop library/startup lifecycle operations must all enter the shared Core lifecycle queue");
}
const restartCoreForLibraryBody = functionBody(
  main,
  "async function restartCoreForLibrary("
);
if (!restartCoreForLibraryBody) {
  fail("desktop main must expose restartCoreForLibrary to restart Core for a library switch");
} else {
  if (!restartCoreForLibraryBody.includes("startCoreProcess(")) {
    fail("desktop main restartCoreForLibrary must start a new Core process for the target library");
  }
  if (!restartCoreForLibraryBody.includes("waitForPortFree(")) {
    fail("desktop main restartCoreForLibrary must wait for the old Core's port to free before rebinding");
  }
  if (!restartCoreForLibraryBody.includes("sessionForDbPath(")) {
    fail("desktop main restartCoreForLibrary must reuse the existing bind address/session token via sessionForDbPath");
  }
  for (const value of ["pendingCoreSession", "assertCurrentCoreGeneration(generation, signal)", "pendingCoreSession = null"]) {
    if (!restartCoreForLibraryBody.includes(value)) {
      fail(`desktop main restartCoreForLibrary must preserve candidate session lifecycle state: ${value}`);
    }
  }
  if (restartCoreForLibraryBody.includes("coreSession = null")) {
    fail("desktop main must retain the last-known coreSession while a library candidate is unhealthy");
  }
  const processCommitIndex = restartCoreForLibraryBody.indexOf("coreProcess = handle");
  const generationFenceIndex = restartCoreForLibraryBody.lastIndexOf(
    "assertCurrentCoreGeneration(generation, signal)",
    processCommitIndex
  );
  if (processCommitIndex === -1 || generationFenceIndex === -1) {
    fail("desktop library restart must compare lifecycle generation before committing its Core handle");
  }
}
if (!main.includes("ensureCoreReady")) {
  fail("desktop main must centralize Core startup outside createWindow");
}
if (!main.includes("coreReadyPromise")) {
  fail("desktop main must coalesce concurrent Core startup with coreReadyPromise");
}
if (!main.includes("pendingCoreProcess")) {
  fail("desktop main must track pending Core process for app quit cleanup");
}
if (!main.includes("waitForCoreHealth(session.baseUrl, session.sessionToken,")) {
  fail("desktop main must pass session token to health wait");
}
if (!lifecycleQueue.includes("this.active?.controller.abort()") || !lifecycleQueue.includes("this.pending.splice(0)")) {
  fail("Core lifecycle queue must cancel active work and discard queued work on shutdown");
}
if (!lifecycleQueue.includes("this.shutdownRequested || this.closed")) {
  fail("Core lifecycle queue must reject work after shutdown begins");
}
for (const value of ["beginGeneration()", "isCurrentGeneration(", "this.generation += 1"]) {
  if (!lifecycleQueue.includes(value)) {
    fail(`Core lifecycle queue must expose generation fencing: ${value}`);
  }
}
const ensureCoreReadyBody = functionBody(main, "async function ensureCoreReady(): Promise<CoreSession>");
const currentHealthyCoreSessionBody = functionBody(
  main,
  "async function currentHealthyCoreSession(signal: AbortSignal): Promise<CoreSession | null>"
);
const startCoreSessionBody = functionBody(
  main,
  "async function startCoreSession(signal: AbortSignal, generation: number): Promise<CoreSession>"
);
if (!ensureCoreReadyBody) {
  fail("desktop main must keep ensureCoreReady as an async CoreSession function");
} else {
  if (!ensureCoreReadyBody.includes("return coreReadyPromise")) {
    fail("desktop main must return shared Core startup promise to concurrent callers");
  }
  for (const value of ["currentHealthyCoreSession(signal)", "coreLifecycleQueue.beginGeneration()", "clearReadyPromise"]) {
    if (!ensureCoreReadyBody.includes(value)) {
      fail(`desktop ensureCoreReady must recheck queued Core state before starting: ${value}`);
    }
  }
}
if (!currentHealthyCoreSessionBody) {
  fail("desktop main must recheck queued Core handles and health before starting");
} else {
  for (const value of [
    "const handle = coreProcess",
    "const session = coreSession",
    "coreLifecycleQueue.currentGeneration",
    "await waitForCoreHealth(",
    "coreProcess !== handle",
    "coreSession !== session"
  ]) {
    if (!currentHealthyCoreSessionBody.includes(value)) {
      fail(`desktop queued Core reuse must recheck state at execution time: ${value}`);
    }
  }
}
if (!startCoreSessionBody) {
  fail("desktop main must isolate Core startup in startCoreSession");
} else {
  if (!startCoreSessionBody.includes("shouldRetryAutoCoreStartup()")) {
    fail("desktop main must retry only auto-selected internal Core startup failures");
  }
  if (!startCoreSessionBody.includes("AUTO_CORE_STARTUP_MAX_ATTEMPTS")) {
    fail("desktop main must bound auto Core startup retries");
  }
  if (!startCoreSessionBody.includes("[megle] Core startup attempt")) {
    fail("desktop main must log Core startup retry/failure attempts clearly");
  }
  if (startCoreSessionBody.includes("coreSession = await createCoreSession")) {
    fail("desktop main must not cache coreSession before Core health succeeds");
  }
  const healthIndex = startCoreSessionBody.indexOf("await waitForCoreHealth");
  const sessionPublishIndex = startCoreSessionBody.indexOf("coreSession = session");
  if (healthIndex === -1 || sessionPublishIndex === -1 || sessionPublishIndex < healthIndex) {
    fail("desktop main must publish coreSession only after Core health succeeds");
  }
  const processPublishIndex = startCoreSessionBody.indexOf("coreProcess = pendingProcess");
  if (healthIndex === -1 || processPublishIndex === -1 || processPublishIndex < healthIndex) {
    fail("desktop main must publish coreProcess only after Core health succeeds");
  }
  const generationFenceIndex = startCoreSessionBody.lastIndexOf(
    "assertCurrentCoreGeneration(generation, signal)",
    processPublishIndex
  );
  if (generationFenceIndex === -1 || generationFenceIndex > processPublishIndex) {
    fail("desktop main must compare lifecycle generation immediately before publishing a started Core");
  }
  if (!startCoreSessionBody.includes("catch") || !startCoreSessionBody.includes("coreReadyPromise = null")) {
    fail("desktop main must clear pending Core startup state on failure");
  }
  if (!startCoreSessionBody.includes("await stopPendingCoreProcess()")) {
    fail("desktop main must stop spawned Core process on startup failure");
  }
  if (!startCoreSessionBody.includes("pendingCoreProcess = pendingProcess")) {
    fail("desktop main must publish pending Core process for quit cleanup during startup");
  }
  if (!startCoreSessionBody.includes("pendingCoreProcess = null")) {
    fail("desktop main must clear pending Core process after startup settles");
  }
  if (!startCoreSessionBody.includes("assertCurrentCoreGeneration(generation, signal)")) {
    fail("desktop main must fence Core startup commits by lifecycle generation");
  }
  if (!startCoreSessionBody.includes("cleanupFailed") || !startCoreSessionBody.includes("retaining process for retry")) {
    fail("desktop main must retain a pending Core process when cleanup fails before retrying startup");
  }
}
const shouldRetryBody = functionBody(main, "function shouldRetryAutoCoreStartup(): boolean");
if (!shouldRetryBody) {
  fail("desktop main must expose a helper for auto Core startup retry eligibility");
} else {
  for (const value of ["MEGLE_CORE_EXTERNAL", "MEGLE_CORE_ADDR", "MEGLE_CORE_URL"]) {
    if (!shouldRetryBody.includes(value)) {
      fail(`auto Core startup retry eligibility must preserve explicit/external setting: ${value}`);
    }
  }
}

const createWindowBody = functionBody(main, "async function createWindow(): Promise<void>");
if (!createWindowBody) {
  fail("desktop main must keep createWindow as an async function");
} else {
  const loadUrlIndex = createWindowBody.indexOf("await window.loadURL(devServer)");
  if (loadUrlIndex === -1) {
    fail("desktop createWindow must load the configured renderer URL");
  } else if (/const\s+devServer[\s\S]*?;\s*armShellReadyFailureFallback\(window\);\s*try\s*\{\s*await\s+window\.loadURL\(devServer\)/.test(createWindowBody)) {
    fail("desktop shell-ready failure fallback must not start before renderer loadURL has had a chance to run");
  }
  if (!/await\s+window\.loadURL\(devServer\);[\s\S]*?armShellReadyFailureFallback\(window\)/.test(createWindowBody)) {
    fail("desktop shell-ready failure fallback must be armed after renderer loadURL completes");
  }
}

const shellReadyHandlerBody = functionBody(main, 'ipcMain.handle("megle:shell-ready", async (event) =>');
if (!shellReadyHandlerBody) {
  fail("desktop main must handle the shell-ready IPC handshake");
} else {
  const clearIndex = shellReadyHandlerBody.indexOf("clearShellReadyFailureFallback()");
  const revealIndex = shellReadyHandlerBody.indexOf("revealMainWindowForShellReady(window)");
  if (clearIndex === -1 || revealIndex === -1 || clearIndex > revealIndex) {
    fail("desktop shell-ready IPC must clear the failure fallback before waiting for ready-to-show reveal");
  }
}

const shellReadyRevealBody = functionBody(
  main,
  "async function revealMainWindowForShellReady(window: BrowserWindow): Promise<boolean>"
);
if (!shellReadyRevealBody) {
  fail("desktop main must keep revealMainWindowForShellReady as an inspectable helper");
} else {
  if (
    shellReadyRevealBody.includes("mainWindowReadyToShow") ||
    shellReadyRevealBody.includes("waitForRendererPaint: true") ||
    shellReadyRevealBody.includes("waitForRendererFrame") ||
    shellReadyRevealBody.includes("executeJavaScript")
  ) {
    fail(
      "desktop shell-ready reveal must not wait on renderer readiness or renderer paint because the renderer is awaiting the shell-ready IPC response"
    );
  }
}

const armShellReadyFailureFallbackBody = functionBody(main, "function armShellReadyFailureFallback(window: BrowserWindow)");
if (!armShellReadyFailureFallbackBody) {
  fail("desktop main must keep armShellReadyFailureFallback as an inspectable helper");
} else if (!armShellReadyFailureFallbackBody.includes("shellReadyRevealPromise")) {
  fail("desktop shell-ready failure fallback must not arm while a shell-ready reveal is already pending");
}

if (!main.includes(".catch(handleDesktopStartupFailure)")) {
  fail("desktop main must catch app startup failures instead of leaving unhandled rejections");
}
const startupFailureBody = functionBody(main, "function handleDesktopStartupFailure(error: unknown): void");
if (!startupFailureBody) {
  fail("desktop main must centralize startup failure handling");
} else {
  if (!startupFailureBody.includes("console.error") && !startupFailureBody.includes("desktopLogger.error")) {
    fail("desktop startup failure handler must log the failure clearly");
  }
  if (!startupFailureBody.includes("requestDesktopShutdown(1)")) {
    fail("desktop startup failure handler must exit predictably with a non-zero status");
  }
}
const shutdownBody = functionBody(main, "function requestDesktopShutdown(exitCode: number): void");
if (!shutdownBody) {
  fail("desktop main must centralize asynchronous Core shutdown and logger close");
} else {
  if (!shutdownBody.includes("stopCore: stopDesktopCoreProcesses")) {
    fail("desktop shutdown must await pending and active Core process termination");
  }
  if (!shutdownBody.includes("await desktopLogger.close()")) {
    fail("desktop shutdown must close the desktop logger after Core termination");
  }
  if (!shutdownBody.includes("app.exit(exitCode)")) {
    fail("desktop shutdown must exit with the requested status after cleanup");
  }
}
for (const [signature, required] of [
  ["async function stopPendingCoreProcess(): Promise<void>", "stopCoreProcessHandleAndClear"],
  ["async function stopDesktopCoreProcesses(): Promise<void>", "stopCoreProcessesBeforeReplacement"]
]) {
  const body = functionBody(main, signature);
  if (!body || !body.includes(required)) {
    fail(`desktop main must use ${required} for retry-safe Core cleanup`);
  }
}
const beforeQuitBody = functionBody(main, 'app.on("before-quit", (event) =>');
if (!beforeQuitBody) {
  fail("desktop main must handle before-quit cleanup");
} else if (!beforeQuitBody.includes("requestDesktopShutdown(0)")) {
  fail("desktop main must stop and await Core processes on before-quit");
}

if (!process.exitCode) {
  console.log("PASS: desktop core orchestration boundaries");
}

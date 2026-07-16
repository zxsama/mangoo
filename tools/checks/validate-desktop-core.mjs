import { readFileSync } from "node:fs";
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
const preload = read("apps/desktop/src/preload.cjs");
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
  'type SecondaryWindowRole = "settings" | "plugins"',
  "SECONDARY_WINDOW_CONFIGS",
  "secondaryWindows",
  'route: "#/settings"',
  'route: "#/plugins"',
  'ipcMain.handle("megle:open-plugins-window"',
  'ipcMain.handle("megle:close-plugins-window"',
  "openSecondaryWindow",
  "closeSecondaryWindow",
  "closeAllSecondaryWindows"
]) {
  if (!main.includes(value)) {
    fail(`desktop secondary-window lifecycle missing ${value}`);
  }
}

const openSecondaryWindowBody = functionBody(
  main,
  "async function openSecondaryWindow(role: SecondaryWindowRole): Promise<void>"
);
if (!openSecondaryWindowBody) {
  fail("desktop must centralize Settings/Plugins creation in openSecondaryWindow");
} else {
  for (const value of [
    "existing.isMinimized()",
    "existing.restore()",
    "existing.focus()",
    "parent",
    "modal: false",
    'backgroundMaterial: "acrylic"',
    "frame: false",
    "secondaryWindows[role] = window",
    "--megle-window=${role}",
    "config.route"
  ]) {
    if (!openSecondaryWindowBody.includes(value)) {
      fail(`shared secondary window must preserve the Settings lifecycle/material contract: ${value}`);
    }
  }
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

if (!desktopPackage.includes('"typecheck"')) {
  fail("desktop package missing typecheck script");
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
const restartCoreForLibraryBody = functionBody(
  main,
  "async function restartCoreForLibrary(dbPath: string): Promise<void>"
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
if (!main.includes("waitForCoreHealth(session.baseUrl, session.sessionToken")) {
  fail("desktop main must pass session token to health wait");
}
const ensureCoreReadyBody = functionBody(main, "async function ensureCoreReady(): Promise<CoreSession>");
const startCoreSessionBody = functionBody(main, "async function startCoreSession(): Promise<CoreSession>");
if (!ensureCoreReadyBody) {
  fail("desktop main must keep ensureCoreReady as an async CoreSession function");
} else {
  if (!ensureCoreReadyBody.includes("return coreReadyPromise")) {
    fail("desktop main must return shared Core startup promise to concurrent callers");
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
  if (!startCoreSessionBody.includes("catch") || !startCoreSessionBody.includes("coreReadyPromise = null")) {
    fail("desktop main must clear pending Core startup state on failure");
  }
  if (!startCoreSessionBody.includes("pendingProcess?.stop()")) {
    fail("desktop main must stop spawned Core process on startup failure");
  }
  if (!startCoreSessionBody.includes("pendingCoreProcess = pendingProcess")) {
    fail("desktop main must publish pending Core process for quit cleanup during startup");
  }
  if (!startCoreSessionBody.includes("pendingCoreProcess = null")) {
    fail("desktop main must clear pending Core process after startup settles");
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

const shellReadyHandlerBody = functionBody(main, 'ipcMain.handle("megle:shell-ready", async () =>');
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
  if (!startupFailureBody.includes("console.error")) {
    fail("desktop startup failure handler must log the failure clearly");
  }
  if (!startupFailureBody.includes("app.exit(1)")) {
    fail("desktop startup failure handler must exit predictably with a non-zero status");
  }
}
const beforeQuitBody = functionBody(main, 'app.on("before-quit", (event) =>');
if (!beforeQuitBody) {
  fail("desktop main must handle before-quit cleanup");
} else if (!beforeQuitBody.includes("pendingCoreProcess?.stop()")) {
  fail("desktop main must stop pending Core process on before-quit");
}

if (!process.exitCode) {
  console.log("PASS: desktop core orchestration boundaries");
}

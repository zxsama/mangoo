import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { createRequire } from "node:module";
import { isMangooWebServerReady, resolveWebDevUrl, waitForMangooWebServer } from "./web-dev-url.mjs";

const root = process.cwd();
const webUrl = await resolveWebDevUrl({ defaultUrl: "http://127.0.0.1:5173" });
console.log(`[mangoo] Starting Desktop with Web at ${webUrl}`);
const parsedWebUrl = new URL(webUrl);
const webHost = parsedWebUrl.hostname;
const webPort = parsedWebUrl.port || (parsedWebUrl.protocol === "https:" ? "443" : "80");
const dataDir = path.join(root, ".data");
const hostDir = path.join(dataDir, "mangoo", "host");

fs.mkdirSync(dataDir, { recursive: true });

const children = new Set();

function spawnChild(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: root,
    env: {
      ...process.env,
      MANGOO_WEB_URL: webUrl,
      MANGOO_HOST_DB_PATH: process.env.MANGOO_HOST_DB_PATH ?? path.join(hostDir, "host.sqlite"),
      MANGOO_LIBRARIES_DIR: process.env.MANGOO_LIBRARIES_DIR ?? path.join(hostDir, "libraries")
    },
    stdio: "inherit",
    shell: false,
    windowsHide: true,
    ...options
  });
  children.add(child);
  child.on("exit", () => {
    children.delete(child);
  });
  return child;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: shouldUseShell(command),
    env: process.env
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function shouldUseShell(command) {
  return process.platform === "win32" && !path.isAbsolute(command);
}

function findCargoCommand() {
  if (process.env.CARGO?.trim()) {
    return process.env.CARGO;
  }

  const cargoHome = process.env.CARGO_HOME ?? path.join(process.env.USERPROFILE ?? "", ".cargo");
  const cargoExe = process.platform === "win32" ? "cargo.exe" : "cargo";
  const cargoPath = path.join(cargoHome, "bin", cargoExe);
  return fs.existsSync(cargoPath) ? cargoPath : "cargo";
}

function electronCommand() {
  return createRequire(import.meta.url)("electron");
}

function shutdown() {
  for (const child of children) {
    if (!child.killed) child.kill();
  }
}

process.on("SIGINT", () => {
  shutdown();
  process.exit(130);
});
process.on("SIGTERM", () => {
  shutdown();
  process.exit(143);
});
process.on("exit", shutdown);

if (process.env.MANGOO_CORE_EXTERNAL !== "1") {
  run("node", ["tools/build/build-bundled-plugins.mjs"]);
  run(findCargoCommand(), ["build", "-p", "mangoo-core"]);
}

if (await isMangooWebServerReady(webUrl)) {
  console.log(`[mangoo] Reusing existing Mangoo web dev server at ${webUrl}`);
} else {
  const web = spawnChild(process.execPath, [
    path.join(root, "node_modules", "vite", "bin", "vite.js"),
    "--host",
    webHost,
    "--port",
    webPort,
    "--strictPort"
  ], { cwd: path.join(root, "apps", "web") });
  await waitForMangooWebServer(webUrl, web);
}

run("npm", ["--workspace", "@mangoo/desktop", "run", "build"]);

const electronArgs = ["apps/desktop/dist/main.js"];
if (process.env.MANGOO_ELECTRON_USER_DATA_DIR) {
  electronArgs.unshift(`--user-data-dir=${path.resolve(process.env.MANGOO_ELECTRON_USER_DATA_DIR)}`);
}
if (process.env.MANGOO_REMOTE_DEBUG === "1") {
  // Default 9500, NOT 9222: on Windows 9222 falls inside a reserved excluded TCP
  // port range (netsh excludedportrange lists 9181-9280), so Electron fails to
  // bind it ("Cannot start http server for devtools" / WSAEACCES) and CDP is
  // unavailable. 9500 sits in a free gap. Override with MANGOO_REMOTE_DEBUG_PORT.
  electronArgs.unshift(`--remote-debugging-port=${process.env.MANGOO_REMOTE_DEBUG_PORT ?? "9500"}`);
}
if (process.env.MANGOO_VISUAL_HARNESS === "1") {
  electronArgs.push("--mangoo-visual-harness=1");
}
if (process.env.MANGOO_VISUAL_RUN_ID) {
  electronArgs.push(`--mangoo-visual-run-id=${process.env.MANGOO_VISUAL_RUN_ID}`);
}
const electron = spawnChild(electronCommand(), electronArgs);
electron.on("exit", (code) => {
  shutdown();
  process.exit(code ?? 0);
});

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const webUrl = process.env.MEGLE_WEB_URL ?? "http://127.0.0.1:5173";
const parsedWebUrl = new URL(webUrl);
const webHost = parsedWebUrl.hostname;
const webPort = parsedWebUrl.port || (parsedWebUrl.protocol === "https:" ? "443" : "80");
const dataDir = path.join(root, ".data");
const dbPath = process.env.MEGLE_DB_PATH ?? path.join(dataDir, "megle-dev.sqlite");

fs.mkdirSync(dataDir, { recursive: true });

const children = new Set();

function spawnChild(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: root,
    env: {
      ...process.env,
      MEGLE_WEB_URL: webUrl,
      MEGLE_DB_PATH: dbPath
    },
    stdio: "inherit",
    shell: process.platform === "win32",
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
  const command = process.platform === "win32" ? "electron.cmd" : "electron";
  return path.join(root, "node_modules", ".bin", command);
}

async function waitForUrl(url, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
      lastError = new Error(`${url} returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${url}: ${String(lastError)}`);
}

async function isMegleWebServerReady(url) {
  try {
    const response = await fetch(url, {
      cache: "no-store"
    });
    if (!response.ok) return false;

    const contentType = response.headers.get("content-type") ?? "";
    if (!contentType.includes("text/html")) return true;

    const html = await response.text();
    return html.includes("<title>Megle</title>") && html.includes("/src/main.tsx");
  } catch {
    return false;
  }
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

if (process.env.MEGLE_CORE_EXTERNAL !== "1") {
  run(findCargoCommand(), ["build", "-p", "megle-core"]);
}

if (await isMegleWebServerReady(webUrl)) {
  console.log(`[megle] Reusing existing Megle web dev server at ${webUrl}`);
} else {
  spawnChild("npm", [
    "--workspace",
    "@megle/web",
    "run",
    "dev",
    "--",
    "--host",
    webHost,
    "--port",
    webPort,
    "--strictPort"
  ]);
  await waitForUrl(webUrl);
}

run("npm", ["--workspace", "@megle/desktop", "run", "build"]);

const electronArgs = ["apps/desktop/dist/main.js"];
if (process.env.MEGLE_ELECTRON_USER_DATA_DIR) {
  electronArgs.unshift(`--user-data-dir=${path.resolve(process.env.MEGLE_ELECTRON_USER_DATA_DIR)}`);
}
if (process.env.MEGLE_REMOTE_DEBUG === "1") {
  // Default 9500, NOT 9222: on Windows 9222 falls inside a reserved excluded TCP
  // port range (netsh excludedportrange lists 9181-9280), so Electron fails to
  // bind it ("Cannot start http server for devtools" / WSAEACCES) and CDP is
  // unavailable. 9500 sits in a free gap. Override with MEGLE_REMOTE_DEBUG_PORT.
  electronArgs.unshift(`--remote-debugging-port=${process.env.MEGLE_REMOTE_DEBUG_PORT ?? "9500"}`);
}
if (process.env.MEGLE_VISUAL_HARNESS === "1") {
  electronArgs.push("--megle-visual-harness=1");
}
if (process.env.MEGLE_VISUAL_RUN_ID) {
  electronArgs.push(`--megle-visual-run-id=${process.env.MEGLE_VISUAL_RUN_ID}`);
}
const electron = spawnChild(electronCommand(), electronArgs);
electron.on("exit", (code) => {
  shutdown();
  process.exit(code ?? 0);
});

// Production one-click launcher for the Megle DESKTOP app.
//
// Behavior (what start-desktop-prod.bat runs):
//   - Hash all build inputs (web/desktop/core source + manifests). If the hash
//     matches the last successful build AND every build output exists, SKIP all
//     compilation and launch immediately (near-zero delay).
//   - If anything changed (or an output is missing), build the production
//     artifacts — web `vite build`, desktop `tsc`, Core `cargo build --release`
//     — then record the new hash.
//   - Serve the built web bundle with `vite preview` (static, no HMR) and launch
//     Electron against it with the RELEASE Core sidecar.
//
// This is the desktop product path only (no web/docker server). `npm run dev`
// remains the hot-reload dev path.

import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { ensureMeglePreview } from "./prod-preview.mjs";

const root = process.cwd();
const dataDir = path.join(root, ".data");
const hostDir = path.join(dataDir, "host");
const previewUrl = "http://127.0.0.1:4173";
const hashFile = path.join(root, ".data", ".prod-build-hash");

fs.mkdirSync(dataDir, { recursive: true });

const children = new Set();
function shutdown() {
  for (const child of children) {
    if (!child.killed) child.kill();
  }
}
process.on("SIGINT", () => { shutdown(); process.exit(130); });
process.on("SIGTERM", () => { shutdown(); process.exit(143); });
process.on("exit", shutdown);

function shouldUseShell(command) {
  return process.platform === "win32" && !path.isAbsolute(command);
}

function run(command, args, label) {
  console.log(`[prod] ${label}: ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: shouldUseShell(command),
    env: process.env
  });
  if (result.status !== 0) {
    console.error(`[prod] ${label} failed (exit ${result.status ?? "?"})`);
    process.exit(result.status ?? 1);
  }
}

function spawnChild(command, args, env = {}) {
  // On Windows, `.cmd`/`.bat` (e.g. electron.cmd, npm) cannot be spawned
  // directly even by absolute path — they need a shell. Use shell on Windows.
  const child = spawn(command, args, {
    cwd: root,
    env: { ...process.env, ...env },
    stdio: "inherit",
    shell: process.platform === "win32",
    windowsHide: true
  });
  children.add(child);
  child.on("exit", () => children.delete(child));
  return child;
}

function findCargoCommand() {
  if (process.env.CARGO?.trim()) return process.env.CARGO;
  const cargoHome = process.env.CARGO_HOME ?? path.join(process.env.USERPROFILE ?? "", ".cargo");
  const cargoExe = process.platform === "win32" ? "cargo.exe" : "cargo";
  const cargoPath = path.join(cargoHome, "bin", cargoExe);
  return fs.existsSync(cargoPath) ? cargoPath : "cargo";
}

function electronCommand() {
  const command = process.platform === "win32" ? "electron.cmd" : "electron";
  return path.join(root, "node_modules", ".bin", command);
}

// ── Hash gate ──────────────────────────────────────────────────────────────
// Recursively hash the content of every source file under the build-input
// directories plus the manifests, so any code/config change busts the cache.
// Build OUTPUTS are excluded (they're derived); only inputs count.
const HASH_INPUTS = [
  "apps/web/src",
  "apps/desktop/src",
  "crates/core/src",
  "crates/core/migrations",
  "crates/core/host_migrations",
  "crates/fake-provider/src",
  "crates/onedrive-provider/src",
  "bundled-plugins",
  "packages/core-client/src"
];
const HASH_FILES = [
  "Cargo.toml",
  "Cargo.lock",
  "crates/core/Cargo.toml",
  "crates/fake-provider/Cargo.toml",
  "crates/onedrive-provider/Cargo.toml",
  "apps/web/vite.config.ts",
  "apps/web/package.json",
  "apps/desktop/package.json",
  "package.json"
];
const HASH_EXCLUDED_FILES = new Set([
  path.normalize("crates/core/src/plugins/runtime/generated_trust.rs")
]);

function hashInputs() {
  const hash = crypto.createHash("sha256");
  const walk = (dir) => {
    if (!fs.existsSync(dir)) return;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else {
        const relative = path.normalize(path.relative(root, full));
        if (HASH_EXCLUDED_FILES.has(relative)) continue;
        hash.update(full);
        hash.update(fs.readFileSync(full));
      }
    }
  };
  for (const dir of HASH_INPUTS) walk(path.join(root, dir));
  for (const file of HASH_FILES) {
    const full = path.join(root, file);
    if (fs.existsSync(full)) {
      hash.update(full);
      hash.update(fs.readFileSync(full));
    }
  }
  return hash.digest("hex");
}

function outputsExist() {
  const coreExe = process.platform === "win32" ? "megle-core.exe" : "megle-core";
  const providerExe = (packageName) => process.platform === "win32" ? `${packageName}.exe` : packageName;
  return (
    fs.existsSync(path.join(root, "apps", "web", "dist", "index.html")) &&
    fs.existsSync(path.join(root, "apps", "desktop", "dist", "main.js")) &&
    fs.existsSync(path.join(root, "target", "release", coreExe)) &&
    fs.existsSync(path.join(root, "target", "release", "bundled-plugins", "com.megle.fake-sync", "plugin.json")) &&
    fs.existsSync(path.join(root, "target", "release", "bundled-plugins", "com.megle.fake-sync", "bin", providerExe("megle-fake-provider"))) &&
    fs.existsSync(path.join(root, "target", "release", "bundled-plugins", "com.megle.onedrive", "plugin.json")) &&
    fs.existsSync(path.join(root, "target", "release", "bundled-plugins", "com.megle.onedrive", "bin", providerExe("megle-onedrive-provider")))
  );
}

// ── Main ─────────────────────────────────────────────────────────────────────
const currentHash = hashInputs();
const storedHash = fs.existsSync(hashFile) ? fs.readFileSync(hashFile, "utf8").trim() : null;
const upToDate = storedHash === currentHash && outputsExist();

if (upToDate) {
  console.log("[prod] no source changes — skipping build, launching directly.");
} else {
  console.log("[prod] changes detected (or missing outputs) — building production artifacts…");
  // Build order: Core (slowest, release) can run while we build the JS, but keep
  // it simple and sequential for clear output. Each tool skips its own unchanged
  // work internally, so a partial change is still fast.
  // A local product-mode launch needs optimized artifacts, but it is not an
  // official release packaging operation. The latter intentionally requires a
  // publisher-owned OneDrive client ID and remains available through --release.
  run("node", ["tools/build/build-bundled-plugins.mjs", "--release-profile"], "build bundled Providers");
  run(findCargoCommand(), ["build", "--release", "-p", "megle-core"], "build core (release)");
  run("npm", ["--workspace", "@megle/web", "run", "build"], "build web");
  run("npm", ["--workspace", "@megle/desktop", "run", "build"], "build desktop");
  fs.writeFileSync(hashFile, currentHash);
  console.log("[prod] build complete.");
}

// Serve the built bundle (static, no HMR). Reuse an existing Megle preview, but
// fail closed if a newly spawned preview exits before our page is confirmed.
await ensureMeglePreview({
  url: previewUrl,
  spawnPreview: () => spawnChild("npm", ["--workspace", "@megle/web", "run", "preview"])
});

// Launch Electron against the preview server with the RELEASE Core sidecar.
console.log("[prod] launching desktop…");
const electronArgs = ["apps/desktop/dist/main.js"];
const electron = spawnChild(electronCommand(), electronArgs, {
  MEGLE_WEB_URL: previewUrl,
  MEGLE_HOST_DB_PATH: process.env.MEGLE_HOST_DB_PATH ?? path.join(hostDir, "host.sqlite"),
  MEGLE_LIBRARIES_DIR: process.env.MEGLE_LIBRARIES_DIR ?? path.join(hostDir, "libraries"),
  MEGLE_CORE_PROFILE: "release"
});
electron.on("exit", (code) => {
  shutdown();
  process.exit(code ?? 0);
});

// Production one-click launcher for the Megle DESKTOP app.
//
// Behavior (what start-desktop-prod.bat runs):
//   - Hash all build and packaging inputs. If the hash matches the last
//     successful package AND every runtime artifact exists, launch immediately.
//   - If anything changed (or an output is missing), build the production
//     artifacts — web `vite build`, desktop `tsc`, Core `cargo build --release`
//     — package a real Windows `Megle.exe`, then record the new hash.
//   - Launch the packaged executable and exit. The runtime contains no Node,
//     npm, CMD, or Vite preview parent process.
//
// This is the desktop product path only (no web/docker server). `npm run dev`
// remains the hot-reload dev path.

import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = path.resolve(import.meta.dirname, "..", "..");
const dataDir = path.join(root, ".data");
const hostDir = path.join(dataDir, "host");
const hashFile = path.join(root, ".data", ".prod-build-hash");
const packagedExecutable = path.join(root, ".data", "desktop-release", "win-unpacked", "Megle.exe");

fs.mkdirSync(dataDir, { recursive: true });

if (process.platform !== "win32") {
  throw new Error("start-desktop-prod.bat packages and launches the Windows desktop build only");
}

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

function findCargoCommand() {
  if (process.env.CARGO?.trim()) return process.env.CARGO;
  const cargoHome = process.env.CARGO_HOME ?? path.join(process.env.USERPROFILE ?? "", ".cargo");
  const cargoExe = process.platform === "win32" ? "cargo.exe" : "cargo";
  const cargoPath = path.join(cargoHome, "bin", cargoExe);
  return fs.existsSync(cargoPath) ? cargoPath : "cargo";
}

// ── Hash gate ──────────────────────────────────────────────────────────────
// Recursively hash the content of every source file under the build-input
// directories plus the manifests, so any code/config change busts the cache.
// Build OUTPUTS are excluded (they're derived); only inputs count.
const HASH_INPUTS = [
  "apps/web/src",
  "apps/desktop/src",
  "apps/web/public",
  "crates/core/src",
  "crates/core/tests",
  "crates/core/migrations",
  "crates/core/host_migrations",
  "crates/fake-provider/src",
  "crates/fake-provider/tests",
  "crates/onedrive-provider/src",
  "crates/onedrive-provider/tests",
  "bundled-plugins",
  "packages/core-client/src",
  "packages/notification-contract/src"
];
const HASH_FILES = [
  "Cargo.toml",
  "Cargo.lock",
  "crates/core/Cargo.toml",
  "crates/fake-provider/Cargo.toml",
  "crates/onedrive-provider/Cargo.toml",
  "tsconfig.base.json",
  "apps/web/index.html",
  "apps/web/tsconfig.json",
  "apps/web/vite.config.ts",
  "apps/web/package.json",
  "apps/desktop/tsconfig.json",
  "apps/desktop/package.json",
  "packages/core-client/package.json",
  "packages/core-client/tsconfig.json",
  "packages/notification-contract/package.json",
  "packages/notification-contract/tsconfig.json",
  "package.json",
  "package-lock.json",
  "tools/build/build-bundled-plugins.mjs",
  "tools/build/package-desktop.mjs",
  "tools/build/generate-desktop-icon.mjs"
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

const requiredOutputs = [
  path.join(root, "apps", "web", "dist", "index.html"),
  path.join(root, "apps", "desktop", "dist", "main.js"),
  path.join(root, "target", "release", "megle-core.exe"),
  path.join(root, "target", "release", "bundled-plugins", "com.megle.fake-sync", "plugin.json"),
  path.join(root, "target", "release", "bundled-plugins", "com.megle.fake-sync", "bin", "megle-fake-provider.exe"),
  path.join(root, "target", "release", "bundled-plugins", "com.megle.onedrive", "plugin.json"),
  path.join(root, "target", "release", "bundled-plugins", "com.megle.onedrive", "bin", "megle-onedrive-provider.exe"),
  packagedExecutable,
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "app.asar"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "icon.png"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "web", "index.html"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "core", "megle-core.exe"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "core", "bundled-plugins", "com.megle.fake-sync", "plugin.json"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "core", "bundled-plugins", "com.megle.fake-sync", "bin", "megle-fake-provider.exe"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "core", "bundled-plugins", "com.megle.onedrive", "plugin.json"),
  path.join(root, ".data", "desktop-release", "win-unpacked", "resources", "core", "bundled-plugins", "com.megle.onedrive", "bin", "megle-onedrive-provider.exe")
];

function missingOutputs() {
  return requiredOutputs.filter((output) => !fs.existsSync(output) || !fs.statSync(output).isFile());
}

function outputsExist() {
  return missingOutputs().length === 0;
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
  // A local product-mode launch needs optimized artifacts. OneDrive application
  // identity is self-managed at runtime, so release artifacts contain no Client ID.
  run("node", ["tools/build/build-bundled-plugins.mjs", "--release-profile"], "build bundled Providers");
  run(findCargoCommand(), ["build", "--release", "-p", "megle-core"], "build core (release)");
  run("npm", ["--workspace", "@megle/web", "run", "build"], "build web");
  run("npm", ["--workspace", "@megle/desktop", "run", "build"], "build desktop");
  run("node", ["tools/build/generate-desktop-icon.mjs"], "generate desktop icon");
  run("node", ["tools/build/package-desktop.mjs"], "package desktop");
  const missing = missingOutputs();
  if (missing.length > 0) {
    throw new Error(`desktop package is incomplete; missing:\n${missing.join("\n")}`);
  }
  fs.writeFileSync(hashFile, currentHash);
  console.log("[prod] build complete.");
}

console.log(`[prod] launching ${packagedExecutable}`);
const desktop = spawn(packagedExecutable, [], {
  cwd: path.dirname(packagedExecutable),
  env: {
    ...process.env,
    MEGLE_HOST_DB_PATH: process.env.MEGLE_HOST_DB_PATH ?? path.join(hostDir, "host.sqlite"),
    MEGLE_LIBRARIES_DIR: process.env.MEGLE_LIBRARIES_DIR ?? path.join(hostDir, "libraries")
  },
  detached: true,
  stdio: "ignore",
  shell: false,
  windowsHide: false
});
await new Promise((resolve, reject) => {
  desktop.once("spawn", resolve);
  desktop.once("error", reject);
});
desktop.unref();
console.log("[prod] Megle started; build launcher exiting.");

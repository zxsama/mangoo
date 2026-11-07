import { build as buildElectron, Platform } from "electron-builder";
import { build as buildJavaScript } from "esbuild";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { assertNoLinks, INVENTORY_FILE, readInventory, renderTrust } from "./plugin-artifacts.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const desktopDir = path.join(root, "apps", "desktop");
const stageDir = path.join(root, ".data", "desktop-package-stage");
const outputDir = path.join(root, ".data", "desktop-release");
const iconIco = path.join(desktopDir, "build", "icon.ico");
const iconPng = path.join(desktopDir, "build", "icon.png");
const electronDist = path.join(root, "node_modules", "electron", "dist");

if (process.platform !== "win32") {
  throw new Error("desktop packaging currently supports Windows only");
}

function assertFile(filePath, label) {
  if (!fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
    throw new Error(`${label} is missing at ${filePath}`);
  }
}

assertFile(path.join(root, "apps", "web", "dist", "index.html"), "web build");
assertFile(path.join(root, "target", "release", "mangoo-core.exe"), "release Core");
const bundledRoot = path.join(root, "target", "release", "bundled-plugins");
const inventory = readInventory(bundledRoot);
const generatedTrust = fs.readFileSync(path.join(root, "crates", "core", "src", "plugins", "runtime", "generated_trust.rs"), "utf8");
if (generatedTrust !== renderTrust(inventory.plugins)) throw new Error("release inventory does not match Core compile-time trust input; rebuild release artifacts");
assertFile(iconIco, "Windows icon");
assertFile(iconPng, "window icon");
assertFile(path.join(electronDist, "electron.exe"), "local Electron distribution");

assertNoLinks(stageDir, { allowMissing: true });
fs.rmSync(stageDir, { recursive: true, force: true });
fs.mkdirSync(stageDir, { recursive: true });
const stagedDist = path.join(stageDir, "dist");
fs.mkdirSync(stagedDist, { recursive: true });
await buildJavaScript({
  entryPoints: [
    path.join(desktopDir, "src", "main.ts"),
    path.join(desktopDir, "src", "notification-store-worker.ts")
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node22",
  outdir: stagedDist,
  external: ["electron"],
  sourcemap: false,
  legalComments: "none"
});
fs.copyFileSync(path.join(desktopDir, "dist", "preload.cjs"), path.join(stagedDist, "preload.cjs"));
fs.copyFileSync(
  path.join(desktopDir, "dist", "notification-ipc-channels.cjs"),
  path.join(stagedDist, "notification-ipc-channels.cjs")
);
const rootPackage = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
fs.writeFileSync(
  path.join(stageDir, "package.json"),
  `${JSON.stringify(
    {
      name: "mangoo-desktop",
      productName: "Mangoo",
      version: rootPackage.version,
      description: "Windows-first local media browser.",
      author: "Mangoo",
      type: "module",
      main: "dist/main.js"
    },
    null,
    2
  )}\n`
);

await buildElectron({
  projectDir: root,
  targets: Platform.WINDOWS.createTarget(["dir"]),
  config: {
    appId: "com.mangoo.desktop",
    productName: "Mangoo",
    executableName: "Mangoo",
    electronVersion: "43.1.0",
    electronDist,
    directories: {
      app: stageDir,
      output: outputDir,
      buildResources: path.join(desktopDir, "build")
    },
    files: ["package.json", "dist/**/*"],
    asar: true,
    extraResources: [
      {
        from: path.join(root, "apps", "web", "dist"),
        to: "web"
      },
      {
        from: path.join(root, "target", "release", "mangoo-core.exe"),
        to: "core/mangoo-core.exe"
      },
      {
        from: path.join(bundledRoot, INVENTORY_FILE),
        to: `core/bundled-plugins/${INVENTORY_FILE}`
      },
      ...inventory.plugins.flatMap((plugin) => [
        { from: path.join(bundledRoot, plugin.id, "plugin.json"), to: `core/bundled-plugins/${plugin.id}/plugin.json` },
        ...(plugin.entry ? [{ from: path.join(bundledRoot, plugin.id, plugin.entry), to: `core/bundled-plugins/${plugin.id}/${plugin.entry}` }] : []),
        ...(plugin.ui ? [{ from: path.join(bundledRoot, plugin.id, plugin.ui.entry), to: `core/bundled-plugins/${plugin.id}/${plugin.ui.entry}` }] : [])
      ]),
      {
        from: iconPng,
        to: "icon.png"
      }
    ],
    win: {
      icon: iconIco,
      requestedExecutionLevel: "asInvoker",
      signAndEditExecutable: true
    },
    npmRebuild: false
  }
});

const executable = path.join(outputDir, "win-unpacked", "Mangoo.exe");
assertFile(executable, "packaged Mangoo executable");
for (const [relativePath, label] of [
  ["resources/app.asar", "packaged application archive"],
  ["resources/icon.png", "packaged window icon"],
  ["resources/web/index.html", "packaged web entry"],
  ["resources/core/mangoo-core.exe", "packaged Core"]
]) {
  assertFile(path.join(outputDir, "win-unpacked", ...relativePath.split("/")), label);
}
const packagedInventory = readInventory(path.join(outputDir, "win-unpacked", "resources", "core", "bundled-plugins"));
if (JSON.stringify(packagedInventory) !== JSON.stringify(inventory)) throw new Error("packaged plugin inventory differs from release build");
console.log(`[desktop-package] ready: ${executable}`);

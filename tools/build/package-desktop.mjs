import { build as buildElectron, Platform } from "electron-builder";
import { build as buildJavaScript } from "esbuild";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

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
assertFile(path.join(root, "target", "release", "megle-core.exe"), "release Core");
for (const [pluginId, executableName] of [
  ["com.megle.fake-sync", "megle-fake-provider.exe"],
  ["com.megle.onedrive", "megle-onedrive-provider.exe"]
]) {
  const pluginRoot = path.join(root, "target", "release", "bundled-plugins", pluginId);
  assertFile(path.join(pluginRoot, "plugin.json"), `${pluginId} manifest`);
  assertFile(path.join(pluginRoot, "bin", executableName), `${pluginId} executable`);
}
assertFile(iconIco, "Windows icon");
assertFile(iconPng, "window icon");
assertFile(path.join(electronDist, "electron.exe"), "local Electron distribution");

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
      name: "megle-desktop",
      productName: "Megle",
      version: rootPackage.version,
      description: "Windows-first local media browser.",
      author: "Megle",
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
    appId: "com.megle.desktop",
    productName: "Megle",
    executableName: "Megle",
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
        from: path.join(root, "target", "release", "megle-core.exe"),
        to: "core/megle-core.exe"
      },
      {
        from: path.join(root, "target", "release", "bundled-plugins"),
        to: "core/bundled-plugins"
      },
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

const executable = path.join(outputDir, "win-unpacked", "Megle.exe");
assertFile(executable, "packaged Megle executable");
for (const [relativePath, label] of [
  ["resources/app.asar", "packaged application archive"],
  ["resources/icon.png", "packaged window icon"],
  ["resources/web/index.html", "packaged web entry"],
  ["resources/core/megle-core.exe", "packaged Core"],
  ["resources/core/bundled-plugins/com.megle.fake-sync/plugin.json", "packaged fake Provider manifest"],
  ["resources/core/bundled-plugins/com.megle.fake-sync/bin/megle-fake-provider.exe", "packaged fake Provider executable"],
  ["resources/core/bundled-plugins/com.megle.onedrive/plugin.json", "packaged OneDrive manifest"],
  ["resources/core/bundled-plugins/com.megle.onedrive/bin/megle-onedrive-provider.exe", "packaged OneDrive executable"]
]) {
  assertFile(path.join(outputDir, "win-unpacked", ...relativePath.split("/")), label);
}
console.log(`[desktop-package] ready: ${executable}`);

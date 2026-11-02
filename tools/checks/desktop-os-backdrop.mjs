import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { launchDesktopHarness, stopDesktopHarness } from "./desktop-harness-lib.mjs";
import { createRunDirectory, root, writeJson } from "./sync-release-lib.mjs";

function parseArgs(argv) {
  const options = { output: null, port: 9520 };
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--output" && argv[index + 1]) options.output = path.resolve(argv[++index]);
    else if (argv[index] === "--port" && argv[index + 1]) options.port = Number(argv[++index]);
    else throw new Error(`unknown or incomplete argument: ${argv[index]}`);
  }
  return options;
}

const options = parseArgs(process.argv.slice(2));
if (process.platform !== "win32") throw new Error("the acrylic OS-backdrop check requires Windows");
const exactTreeRoot = path.resolve(process.cwd());
const expectedRoot = path.join(exactTreeRoot, "tools", "checks", "desktop-os-backdrop.mjs");
if (path.resolve(import.meta.filename) !== expectedRoot) {
  throw new Error(`OS-backdrop harness must execute from its own exact tree: ${exactTreeRoot}`);
}
const exactTree = process.env.MANGOO_EVIDENCE_EXACT_TREE === "1";
const commit = process.env.MANGOO_EVIDENCE_COMMIT
  ?? spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8", windowsHide: true }).stdout.trim();
if (exactTree && !process.env.MANGOO_EVIDENCE_COMMIT) {
  throw new Error("exact-tree OS-backdrop collection requires MANGOO_EVIDENCE_COMMIT");
}
const runDirectory = await createRunDirectory("os-backdrop", options.output);
const runId = path.basename(runDirectory);
const mainSource = await readFile(path.join(exactTreeRoot, "apps", "desktop", "src", "main.ts"), "utf8");
const staticContract = {
  constructorAcrylic: /backgroundMaterial:\s*["']acrylic["']/.test(mainSource),
  nativeRefresh: /setBackgroundMaterial\(["']acrylic["']\)/.test(mainSource),
  transparentBacking: /backgroundColor:\s*["']#00000000["']/.test(mainSource),
};
let desktop;
try {
  desktop = await launchDesktopHarness({ directory: path.join(runDirectory, "desktop-fixture"), port: options.port, runId, fixtureCount: 64, repoRoot: exactTreeRoot });
  await delay(5_000);
  const runtime = await desktop.cdp.evalJson(`(() => ({
    title: document.title,
    role: window.mangooDesktop.windowRole,
    visualHarness: typeof window.mangooDesktop.visual?.capturePage === "function",
    htmlBackground: getComputedStyle(document.documentElement).backgroundColor,
    bodyBackground: getComputedStyle(document.body).backgroundColor,
    shellPresent: Boolean(document.querySelector(".app-shell, .app-frame, main")),
    error: document.querySelector(".error-strip")?.textContent ?? null
  }))()`);
  const captureEvaluation = await desktop.cdp.send("Runtime.evaluate", {
    expression: "window.mangooDesktop.visual.capturePage()",
    awaitPromise: true,
    returnByValue: true,
  });
  if (captureEvaluation.exceptionDetails) {
    throw new Error(`native visual capture failed: ${captureEvaluation.exceptionDetails.text}`);
  }
  const nativeCapture = captureEvaluation.result.value;
  const captureBytes = typeof nativeCapture === "string" ? Buffer.from(nativeCapture, "base64").length : 0;
  const screenshot = path.join(runDirectory, "os-backdrop.png");
  await desktop.cdp.screenshot(screenshot);
  const passed = Object.values(staticContract).every(Boolean)
    && runtime.role === "main"
    && runtime.visualHarness
    && runtime.shellPresent
    && !runtime.error
    && captureBytes > 1_000;
  const result = { schemaVersion: 1, kind: "windows-os-backdrop", commit, exactTree, runId, staticContract, runtime, nativeCaptureBytes: captureBytes, screenshot, passed };
  await writeJson(path.join(runDirectory, "os-backdrop-result.json"), result);
  console.log(`WINDOWS_OS_BACKDROP_JSON=${JSON.stringify(result)}`);
  if (!passed) process.exitCode = 1;
} finally {
  desktop?.cdp.close();
  await stopDesktopHarness(desktop?.child);
}

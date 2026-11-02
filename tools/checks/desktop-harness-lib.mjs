import { spawn, spawnSync } from "node:child_process";
import { mkdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { connectCdp, resolvePageWsUrl } from "../dev/cdp-client.mjs";
import { managedLibraryHostEnv } from "../dev/managed-library-harness.mjs";
import { root } from "./sync-release-lib.mjs";

const ONE_PIXEL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
);

export async function prepareDesktopFixture(directory, count = 256) {
  const mediaRoot = path.join(directory, "media");
  const managedRoot = path.join(directory, "managed-library");
  await rm(directory, { recursive: true, force: true });
  await mkdir(mediaRoot, { recursive: true });
  await mkdir(managedRoot, { recursive: true });
  await Promise.all(
    Array.from({ length: count }, (_, index) =>
      writeFile(path.join(mediaRoot, `fixture-${String(index).padStart(4, "0")}.png`), ONE_PIXEL_PNG),
    ),
  );
  return { mediaRoot, managedRoot };
}

export async function launchDesktopHarness({ directory, port, runId, fixtureCount = 256, repoRoot = root }) {
  if (process.platform !== "win32") {
    throw new Error("the desktop compositor harness requires Windows");
  }
  const { mediaRoot, managedRoot } = await prepareDesktopFixture(directory, fixtureCount);
  const child = spawn("npm", ["run", "dev"], {
    cwd: repoRoot,
    env: {
      ...process.env,
      MANGOO_WEB_URL: `http://127.0.0.1:${port + 1}`,
      MANGOO_REMOTE_DEBUG: "1",
      MANGOO_REMOTE_DEBUG_PORT: String(port),
      MANGOO_VISUAL_HARNESS: "1",
      MANGOO_VISUAL_OS_BACKDROP: "1",
      MANGOO_VISUAL_RUN_ID: runId,
      MANGOO_ELECTRON_USER_DATA_DIR: path.join(directory, "electron-user-data"),
      MANGOO_AUTO_ADD_ROOT: mediaRoot,
      MANGOO_AUTO_LIBRARY_BASE: managedRoot,
      ...managedLibraryHostEnv(directory),
    },
    stdio: ["ignore", "pipe", "pipe"],
    shell: true,
    windowsHide: true,
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const deadline = Date.now() + 180_000;
  let ws = null;
  while (Date.now() < deadline && child.exitCode === null) {
    try {
      ws = await resolvePageWsUrl(port);
      if (ws) break;
    } catch {}
    await delay(500);
  }
  if (!ws) {
    await stopDesktopHarness(child);
    throw new Error(`desktop CDP target did not start\n${stdout}\n${stderr}`);
  }
  const cdp = connectCdp(ws);
  await cdp.ready;
  await cdp.send("Runtime.enable");
  await cdp.send("Log.enable");
  await cdp.send("Page.enable");
  const readyDeadline = Date.now() + 90_000;
  let snapshot = null;
  while (Date.now() < readyDeadline) {
    snapshot = await cdp.evalJson(`(() => ({
      title: document.title,
      ready: document.readyState,
      role: window.mangooDesktop?.windowRole ?? null,
      hasVisualCapture: typeof window.mangooDesktop?.visual?.capturePage === "function",
      mediaTiles: document.querySelectorAll(".media-tile").length,
      grid: Boolean(document.querySelector(".virtual-grid")),
      error: document.querySelector(".error-strip")?.textContent ?? null
    }))()`);
    if (snapshot?.ready === "complete" && snapshot?.role === "main" && snapshot?.hasVisualCapture) break;
    await delay(500);
  }
  if (snapshot?.ready !== "complete" || snapshot?.role !== "main" || !snapshot?.hasVisualCapture) {
    cdp.close();
    await stopDesktopHarness(child);
    throw new Error(`desktop renderer did not become ready: ${JSON.stringify(snapshot)}\n${stdout}\n${stderr}`);
  }
  return { child, cdp, mediaRoot, managedRoot, output: () => ({ stdout, stderr }) };
}

export async function stopDesktopHarness(child) {
  if (!child || child.exitCode !== null) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true,
    });
  } else {
    child.kill("SIGTERM");
  }
  await Promise.race([
    new Promise((resolve) => child.once("close", resolve)),
    delay(10_000),
  ]);
}

import { execFileSync, spawn } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import {
  addExistingMediaRoot,
  createAndActivateManagedLibrary,
  managedLibraryHostEnv
} from "../dev/managed-library-harness.mjs";

const root = process.cwd();
const token = "managed-library-lifecycle-bench";
const addr = process.env.MEGLE_LIBRARY_BENCH_ADDR ?? "127.0.0.1:47421";
const baseUrl = `http://${addr}/api`;
const output = process.env.MEGLE_LIBRARY_BENCH_OUTPUT ?? path.join(root, ".tmp", "bench", "managed-library-lifecycle.json");
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lP7zWQAAAABJRU5ErkJggg==", "base64");

async function api(apiPath, init = {}) {
  const headers = new Headers(init.headers);
  headers.set("x-megle-session", token);
  if (init.body) headers.set("content-type", "application/json");
  const response = await fetch(`${baseUrl}${apiPath}`, { ...init, headers });
  const text = await response.text();
  if (!response.ok) throw new Error(`${apiPath} -> ${response.status}: ${text}`);
  return text ? JSON.parse(text) : null;
}

async function waitForHealth() {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    try { await api("/health"); return; } catch { await delay(250); }
  }
  throw new Error("Core did not become healthy");
}

async function waitForScan(taskId) {
  for (let attempt = 0; attempt < 240; attempt += 1) {
    const tasks = await api("/tasks");
    const task = tasks.items.find((item) => item.id === taskId);
    if (task?.status === "succeeded") return task;
    if (task && ["failed", "cancelled"].includes(task.status)) throw new Error(`scan ${taskId} ${task.status}: ${task.error}`);
    await delay(250);
  }
  throw new Error(`scan ${taskId} timed out`);
}

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
}

function processSample(pid) {
  if (process.platform !== "win32") return null;
  const script = `$p=Get-Process -Id ${pid}; [pscustomobject]@{cpu=$p.CPU;privateBytes=$p.PrivateMemorySize64}|ConvertTo-Json -Compress`;
  return JSON.parse(execFileSync("powershell.exe", ["-NoProfile", "-Command", script], { encoding: "utf8" }));
}

async function main() {
  const dataDir = await mkdtemp(path.join(tmpdir(), "megle-library-bench-"));
  const mediaRoot = path.join(dataDir, "media");
  await mkdir(mediaRoot, { recursive: true });
  await Promise.all(Array.from({ length: 100 }, (_, index) => writeFile(path.join(mediaRoot, `sample-${index}.png`), png)));
  execFileSync("cargo", ["build", "-q", "-p", "megle-core"], { cwd: root, stdio: "inherit" });
  const corePath = path.join(root, "target", "debug", process.platform === "win32" ? "megle-core.exe" : "megle-core");
  const core = spawn(corePath, [], {
    cwd: root,
    env: { ...process.env, ...managedLibraryHostEnv(dataDir), MEGLE_SESSION_TOKEN: token, MEGLE_CORE_ADDR: addr, RUST_LOG: "warn" },
    stdio: ["ignore", "ignore", "inherit"],
    windowsHide: true
  });
  try {
    await waitForHealth();
    const createStarted = performance.now();
    const first = await createAndActivateManagedLibrary({ baseUrl, token, name: "Baseline A", managedRoot: path.join(dataDir, "library-a") });
    const firstCreationMs = performance.now() - createStarted;
    const accepted = await addExistingMediaRoot({ baseUrl, token, contextRevision: first.contextRevision, mediaRoot });
    await waitForScan(accepted.taskId);
    const second = await createAndActivateManagedLibrary({ baseUrl, token, name: "Baseline B", managedRoot: path.join(dataDir, "library-b") });

    let active = await api("/libraries/active");
    const switchMs = [];
    for (let index = 0; index < 20; index += 1) {
      const target = index % 2 === 0 ? first.library.libraryUuid : second.library.libraryUuid;
      const started = performance.now();
      active = await api(`/libraries/${target}/activate`, { method: "POST", headers: { "x-megle-context-revision": String(active.contextRevision) } });
      switchMs.push(performance.now() - started);
    }
    if (active.library.libraryUuid !== first.library.libraryUuid) {
      active = await api(`/libraries/${first.library.libraryUuid}/activate`, { method: "POST", headers: { "x-megle-context-revision": String(active.contextRevision) } });
    }

    const browseMs = [];
    for (let index = 0; index < 100; index += 1) {
      const started = performance.now();
      await api(`/media?limit=96&offset=${index % 5}&sort=mtime_desc`);
      browseMs.push(performance.now() - started);
    }
    const beforeIdle = processSample(core.pid);
    const idleWindowMs = 5_000;
    await delay(idleWindowMs);
    const afterIdle = processSample(core.pid);
    const report = {
      capturedAtUtc: new Date().toISOString(),
      environment: { platform: process.platform, arch: process.arch, node: process.version, sampleMedia: 100, idleWindowMs },
      firstCreationMs,
      librarySwitch: { samples: switchMs.length, p50Ms: percentile(switchMs, 0.5), p95Ms: percentile(switchMs, 0.95), maxMs: Math.max(...switchMs) },
      browseHotPath: { samples: browseMs.length, p50Ms: percentile(browseMs, 0.5), p95Ms: percentile(browseMs, 0.95), maxMs: Math.max(...browseMs) },
      idle: afterIdle && beforeIdle ? { cpuPercent: ((afterIdle.cpu - beforeIdle.cpu) * 1000 / idleWindowMs) * 100, privateBytes: afterIdle.privateBytes } : null
    };
    await mkdir(path.dirname(output), { recursive: true });
    await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);
    console.log(JSON.stringify(report, null, 2));
  } finally {
    if (core.exitCode === null && core.signalCode === null) core.kill();
    await Promise.race([
      new Promise((resolve) => core.once("close", resolve)),
      delay(2_000)
    ]);
    if (process.platform === "win32" && core.exitCode === null && core.signalCode === null) {
      const killed = spawn("taskkill", ["/PID", String(core.pid), "/T", "/F"], {
        stdio: "ignore",
        windowsHide: true
      });
      await new Promise((resolve) => {
        killed.once("error", resolve);
        killed.once("close", resolve);
      });
    }
    if (process.env.MEGLE_LIBRARY_BENCH_KEEP !== "1") await rm(dataDir, { recursive: true, force: true });
  }
}

await main();

import { spawnSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { launchDesktopHarness, stopDesktopHarness } from "../checks/desktop-harness-lib.mjs";
import { createRunDirectory, hashJson, root, runId as createRunId, writeJson } from "../checks/sync-release-lib.mjs";

function parseArgs(argv) {
  const options = { output: null, quick: false, port: 9510 };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "--output" && argv[index + 1]) options.output = path.resolve(argv[++index]);
    else if (value === "--quick") options.quick = true;
    else if (value === "--port" && argv[index + 1]) options.port = Number(argv[++index]);
    else throw new Error(`unknown or incomplete argument: ${value}`);
  }
  return options;
}

function git(args) {
  const result = spawnSync("git", args, { cwd: root, encoding: "utf8", windowsHide: true });
  if (result.status !== 0) throw new Error(result.stderr || `git ${args.join(" ")} failed`);
  return result.stdout.trim();
}

function percentile(values, fraction) {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
}

function summarize(sample) {
  const frames = sample.frameDeltas.filter((value) => value > 0 && value < 1000);
  const longFrames = frames.filter((value) => value > 50).length;
  const dropped = frames.reduce((sum, value) => sum + Math.max(0, Math.floor(value / (1000 / 60)) - 1), 0);
  const expectedFrames = frames.length + dropped;
  return {
    durationMs: sample.durationMs,
    frames: frames.length,
    frameP50Ms: percentile(frames, 0.50),
    frameP95Ms: percentile(frames, 0.95),
    frameP99Ms: percentile(frames, 0.99),
    longFrameCount: longFrames,
    longFramePercent: frames.length ? (longFrames / frames.length) * 100 : 100,
    droppedFrames: dropped,
    droppedFramePercent: expectedFrames ? (dropped / expectedFrames) * 100 : 100,
    longTasks: sample.longTasks,
    syncRequests: sample.syncRequests ?? 0,
    syncRequestFailures: sample.syncRequestFailures ?? 0,
    inputEvents: sample.inputEvents ?? 0,
  };
}

async function installCollector(cdp) {
  await cdp.run(`(() => {
    window.__megleRendererPerf = { frameDeltas: [], longTasks: [], syncRequests: 0, syncRequestFailures: 0, inputEvents: 0, startedAt: performance.now(), lastFrame: null };
    const state = window.__megleRendererPerf;
    const tick = (now) => {
      if (state.lastFrame !== null) state.frameDeltas.push(now - state.lastFrame);
      state.lastFrame = now;
      state.raf = requestAnimationFrame(tick);
    };
    state.raf = requestAnimationFrame(tick);
    state.observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) state.longTasks.push(entry.duration);
    });
    try { state.observer.observe({ type: "longtask", buffered: true }); } catch {}
    addEventListener("pointermove", () => { state.inputEvents += 1; }, { passive: true });
    addEventListener("keydown", () => { state.inputEvents += 1; }, { passive: true });
  })()`);
}

async function takeSample(cdp) {
  const sample = await cdp.evalJson(`(() => {
    const state = window.__megleRendererPerf;
    const value = {
      frameDeltas: state.frameDeltas,
      longTasks: state.longTasks,
      syncRequests: state.syncRequests,
      syncRequestFailures: state.syncRequestFailures,
      inputEvents: state.inputEvents,
      durationMs: performance.now() - state.startedAt
    };
    cancelAnimationFrame(state.raf);
    state.observer?.disconnect();
    return value;
  })()`);
  return summarize(sample);
}

const options = parseArgs(process.argv.slice(2));
if (process.platform !== "win32") throw new Error("renderer sync performance collection requires Windows");
const exactTree = process.env.MEGLE_EVIDENCE_EXACT_TREE === "1";
const commit = process.env.MEGLE_EVIDENCE_COMMIT ?? git(["rev-parse", "HEAD"]);
const dirty = exactTree ? false : git(["status", "--porcelain=v1"]).length > 0;
if (exactTree && !process.env.MEGLE_EVIDENCE_COMMIT) {
  throw new Error("exact-tree renderer collection requires MEGLE_EVIDENCE_COMMIT");
}
const exactTreeRoot = path.resolve(process.cwd());
const expectedRoot = path.join(exactTreeRoot, "tools", "bench", "renderer-sync-performance.mjs");
if (path.resolve(import.meta.filename) !== expectedRoot) {
  throw new Error(`renderer collector must execute from its own exact tree: ${exactTreeRoot}`);
}
const runDirectory = await createRunDirectory("renderer", options.output ? path.dirname(options.output) : null);
const output = options.output ?? path.join(runDirectory, "renderer-evidence.json");
const runId = createRunId("renderer");
const fixtureDirectory = path.join(runDirectory, "desktop-fixture");
const durationMs = options.quick ? 3_000 : 10_000;
const preloadSource = await readFile(path.join(exactTreeRoot, "apps", "desktop", "src", "preload.cjs"), "utf8");
const syncIpcMatches = preloadSource.match(/ipcRenderer\.sendSync\s*\(/g) ?? [];
let desktop;
try {
  desktop = await launchDesktopHarness({ directory: fixtureDirectory, port: options.port, runId, fixtureCount: options.quick ? 128 : 512, repoRoot: exactTreeRoot });
  await delay(options.quick ? 4_000 : 12_000);
  await installCollector(desktop.cdp);
  await delay(durationMs);
  const baseline = await takeSample(desktop.cdp);

  await installCollector(desktop.cdp);
  await desktop.cdp.run(`(() => {
    const state = window.__megleRendererPerf;
    const coreUrl = window.megleDesktop.coreUrl;
    const token = window.megleDesktop.sessionToken;
    let direction = 1;
    state.stressTimer = setInterval(() => {
      const grid = document.querySelector(".virtual-grid");
      if (grid) {
        const maximum = Math.max(0, grid.scrollHeight - grid.clientHeight);
        if (grid.scrollTop >= maximum) direction = -1;
        if (grid.scrollTop <= 0) direction = 1;
        grid.scrollTop = Math.max(0, Math.min(maximum, grid.scrollTop + direction * 360));
      }
      dispatchEvent(new PointerEvent("pointermove", { clientX: 420, clientY: 360, bubbles: true }));
      dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
      for (const endpoint of ["/sync/status", "/sync/wake"]) {
        state.syncRequests += 1;
        fetch(coreUrl + endpoint, {
          method: endpoint.endsWith("wake") ? "POST" : "GET",
          headers: { "x-megle-session": token }
        }).then((response) => { if (!response.ok) state.syncRequestFailures += 1; })
          .catch(() => { state.syncRequestFailures += 1; });
      }
    }, 250);
  })()`);
  await delay(durationMs);
  await desktop.cdp.run(`clearInterval(window.__megleRendererPerf.stressTimer)`);
  const stress = await takeSample(desktop.cdp);
  const longFrameIncrease = stress.longFramePercent - baseline.longFramePercent;
  const droppedFrameIncrease = stress.droppedFramePercent - baseline.droppedFramePercent;
  const result = {
    schemaVersion: 1,
    kind: "renderer-sync-performance",
    runId,
    commit,
    dirty,
    exactTree,
    capturedAt: new Date().toISOString(),
    machine: {
      hostname: os.hostname(),
      platform: process.platform,
      release: os.release(),
      cpu: os.cpus()[0]?.model ?? null,
      cpuCount: os.cpus().length,
      totalMemoryBytes: os.totalmem(),
    },
    fixture: {
      mediaCount: options.quick ? 128 : 512,
      digest: hashJson({ generator: "one-pixel-png", mediaCount: options.quick ? 128 : 512 }),
    },
    policy: "same renderer process, fixture, machine, power state and warmed cache; baseline idle then sustained production /sync/status + /sync/wake traffic with grid and input activity",
    baseline,
    stress,
    baselineLongFramePercent: baseline.longFramePercent,
    stressLongFramePercent: stress.longFramePercent,
    baselineDroppedFramePercent: baseline.droppedFramePercent,
    stressDroppedFramePercent: stress.droppedFramePercent,
    longFrameIncrease,
    droppedFrameIncrease,
    inputSyncIpcCalls: syncIpcMatches.length,
    inputSyncIpcEvidence: "exact-tree preload source contains no ipcRenderer.sendSync call; runtime stress uses asynchronous fetch against production Core sync endpoints",
    passed: baseline.frames >= 30
      && stress.frames >= 30
      && stress.syncRequests > 0
      && stress.syncRequestFailures === 0
      && longFrameIncrease <= 2
      && droppedFrameIncrease <= 2
      && syncIpcMatches.length === 0,
  };
  await writeJson(output, result);
  console.log(`SYNC_RENDERER_PERF_JSON=${JSON.stringify(result)}`);
  if (!result.passed) process.exitCode = 1;
} finally {
  desktop?.cdp.close();
  await stopDesktopHarness(desktop?.child);
}

import path from "node:path";
import process from "node:process";
import os from "node:os";
import { spawnSync } from "node:child_process";
import { readFile, rm } from "node:fs/promises";
import { createRunDirectory, environmentFacts, requiredFailures, root, runCommand, writeJson } from "../checks/sync-release-lib.mjs";

function parseArgs(argv) {
  const options = {
    output: null,
    quick: false,
    largeBytes: null,
    largeTransferRoot: process.env.MEGLE_SYNC_LARGE_TRANSFER_ROOT ?? null,
    cleanupLargeTransfer: false,
    rendererEvidence: process.env.MEGLE_SYNC_RENDERER_EVIDENCE ?? null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "--output" && argv[index + 1]) options.output = argv[++index];
    else if (value === "--quick") options.quick = true;
    else if (value === "--large-bytes" && argv[index + 1]) options.largeBytes = argv[++index];
    else if (value === "--large-transfer-root" && argv[index + 1]) options.largeTransferRoot = path.resolve(argv[++index]);
    else if (value === "--cleanup-large-transfer") options.cleanupLargeTransfer = true;
    else if (value === "--renderer-evidence" && argv[index + 1]) options.rendererEvidence = argv[++index];
    else throw new Error(`unknown or incomplete argument: ${value}`);
  }
  return options;
}

const options = parseArgs(process.argv.slice(2));
const runDirectory = await createRunDirectory("benchmark", options.output);
const assets = options.quick ? "1000" : "1000000";
const largeBytes = options.largeBytes ?? (options.quick ? String(128 * 1024 * 1024) : String(10 * 1024 * 1024 * 1024));
const largeTransferRoot = options.largeTransferRoot ?? path.join(runDirectory, "large-transfer");
if (options.cleanupLargeTransfer) {
  const temporaryRoot = `${path.resolve(os.tmpdir())}${path.sep}`.toLowerCase();
  const cleanupRoot = path.resolve(largeTransferRoot);
  if (
    !cleanupRoot.toLowerCase().startsWith(temporaryRoot)
    || !path.basename(cleanupRoot).startsWith("megle-sync-large-")
  ) {
    throw new Error("--cleanup-large-transfer requires a megle-sync-large-* directory under the system temporary root");
  }
}
const rows = [];
const expectedCommit = process.env.MEGLE_EVIDENCE_COMMIT
  ?? spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8", windowsHide: true }).stdout.trim();

async function rendererEvidenceRow(file) {
  if (!file) {
    return {
      name: "renderer-performance-gate",
      command: "validate renderer evidence",
      cwd: process.cwd(),
      startedAt: new Date().toISOString(),
      durationMs: 0,
      exitCode: 1,
      signal: null,
      status: "blocked",
      required: true,
      reason: "renderer evidence is absent; pass --renderer-evidence or MEGLE_SYNC_RENDERER_EVIDENCE",
    };
  }
  try {
    const evidencePath = path.resolve(file);
    const evidence = JSON.parse(await readFile(evidencePath, "utf8"));
    const required = ["baselineLongFramePercent", "stressLongFramePercent", "baselineDroppedFramePercent", "stressDroppedFramePercent", "inputSyncIpcCalls"];
    const missing = required.filter((name) => !Number.isFinite(evidence[name]));
    const longFrameIncrease = evidence.stressLongFramePercent - evidence.baselineLongFramePercent;
    const droppedFrameIncrease = evidence.stressDroppedFramePercent - evidence.baselineDroppedFramePercent;
    const violations = [];
    if (evidence.kind !== "renderer-sync-performance") violations.push("unexpected evidence kind");
    if (typeof evidence.runId !== "string" || evidence.runId.length < 16) violations.push("missing stable collector run identity");
    if (!expectedCommit || evidence.commit !== expectedCommit) violations.push(`commit ${evidence.commit ?? "missing"} != ${expectedCommit || "unknown"}`);
    if (evidence.dirty !== false) violations.push("collector did not run from a clean tree");
    if (process.env.MEGLE_EVIDENCE_EXACT_TREE === "1" && evidence.exactTree !== true) violations.push("collector evidence is not exact-tree bound");
    if (!evidence.machine?.hostname || !evidence.machine?.platform || !evidence.machine?.cpu) violations.push("missing machine identity");
    if (!evidence.fixture?.digest) violations.push("missing fixture identity");
    if (!Number.isFinite(Date.parse(evidence.capturedAt))) violations.push("missing capture timestamp");
    if (missing.length) violations.push(`missing numeric fields: ${missing.join(", ")}`);
    if (!missing.length && longFrameIncrease > 2) violations.push(`long-frame increase ${longFrameIncrease.toFixed(3)}pp > 2pp`);
    if (!missing.length && droppedFrameIncrease > 2) violations.push(`dropped-frame increase ${droppedFrameIncrease.toFixed(3)}pp > 2pp`);
    if (!missing.length && evidence.inputSyncIpcCalls !== 0) violations.push(`input-path sync IPC calls ${evidence.inputSyncIpcCalls} != 0`);
    return {
      name: "renderer-performance-gate",
      command: `validate ${evidencePath}`,
      cwd: process.cwd(),
      startedAt: new Date().toISOString(),
      durationMs: 0,
      exitCode: violations.length ? 1 : 0,
      signal: null,
      status: violations.length ? "failed" : "passed",
      required: true,
      reason: violations.length ? violations.join("; ") : null,
      evidencePath,
      metrics: { ...evidence, longFrameIncrease, droppedFrameIncrease },
    };
  } catch (error) {
    return {
      name: "renderer-performance-gate",
      command: `validate ${path.resolve(file)}`,
      cwd: process.cwd(),
      startedAt: new Date().toISOString(),
      durationMs: 0,
      exitCode: 1,
      signal: null,
      status: "failed",
      required: true,
      reason: `renderer evidence could not be read: ${error.message}`,
    };
  }
}

rows.push(await runCommand({
  name: "paired-sync-performance",
  command: "node",
  args: ["tools/bench/sync-orchestration-bench.mjs"],
  env: { MEGLE_SYNC_BENCH_ASSETS: assets },
  runDirectory,
  timeoutMs: 75 * 60 * 1000,
}));
rows.push(await runCommand({
  name: "media-inventory-scale",
  command: "cargo",
  args: ["test", "-p", "megle-core", "--release", "sync::media::worker::tests::media_inventory_scale_probe", "--", "--ignored", "--nocapture"],
  runDirectory,
  timeoutMs: 30 * 60 * 1000,
}));

function parseMetric(log, marker) {
  const match = log.match(new RegExp(`${marker}=(\\{[^\\n]+\\})`));
  if (!match) throw new Error(`missing ${marker} in benchmark output`);
  return JSON.parse(match[1]);
}

const performanceRow = rows.find((candidate) => candidate.name === "paired-sync-performance");
const performanceSample = performanceRow?.status === "passed"
  ? parseMetric(await readFile(performanceRow.stdoutPath, "utf8"), "SYNC_BENCH_JSON")
  : null;
let pairedPerformance = null;
if (performanceSample) {
  pairedPerformance = {
    policy: "same commit, generated fixture, machine, process and warm cache; baseline and stress use identical warmed production query paths",
    metrics: performanceSample,
    regressionPercent: {
      browseP95: performanceSample.browseP95RegressionPercent,
      browseP99: performanceSample.browseP99RegressionPercent,
      searchP95: performanceSample.searchP95RegressionPercent,
      searchP99: performanceSample.searchP99RegressionPercent,
      thumbP95: performanceSample.thumbP95RegressionPercent,
    },
  };
  const violations = [];
  if (performanceSample.browseP95Ms > 50 || pairedPerformance.regressionPercent.browseP95 > 10) violations.push("browse p95");
  if (performanceSample.browseP99Ms > 100 || pairedPerformance.regressionPercent.browseP99 > 15) violations.push("browse p99");
  if (performanceSample.searchP95Ms > 150 || pairedPerformance.regressionPercent.searchP95 > 10) violations.push("search p95");
  if (performanceSample.searchP99Ms > 300 || pairedPerformance.regressionPercent.searchP99 > 15) violations.push("search p99");
  if ((performanceSample.thumbP95Ms > 1 && pairedPerformance.regressionPercent.thumbP95 > 10) || performanceSample.providerCalls !== 0) violations.push("thumbnail/provider");
  if (performanceSample.localCaptureP95Ms > 5 || performanceSample.localCaptureP99Ms > 15) violations.push("capture latency");
  if (performanceSample.applyP95Ms > 10 || performanceSample.applyP99Ms > 25) violations.push("apply latency");
  if (performanceSample.idleCpuPercent >= 1 || performanceSample.dormantPrivateMiB >= 100) violations.push("idle resource");
  if (performanceSample.stressCpuPercent >= 15 || performanceSample.stressPrivateMiB >= 256) violations.push("stress resource");
  rows.push({
    name: "paired-performance-gate",
    command: "internal metric comparison",
    cwd: process.cwd(),
    startedAt: new Date().toISOString(),
    durationMs: 0,
    exitCode: violations.length ? 1 : 0,
    signal: null,
    status: violations.length ? "failed" : "passed",
    required: true,
    reason: violations.length ? `threshold failures: ${violations.join(", ")}` : null,
  });
}
rows.push(await runCommand({
  name: "build-bundled-providers-release",
  command: "node",
  // This benchmark needs optimized Provider binaries, not a publishable Core
  // artifact. Publisher Client ID validation remains an independent release row.
  args: ["tools/build/build-bundled-plugins.mjs", "--release-profile"],
  runDirectory,
  timeoutMs: 30 * 60 * 1000,
}));
const targetRoot = process.env.CARGO_TARGET_DIR ? path.resolve(root, process.env.CARGO_TARGET_DIR) : path.join(root, "target");
rows.push(await runCommand({
  name: "physical-large-transfer-resume",
  command: "cargo",
  args: ["test", "-p", "megle-core", "--release", "sync::media::worker::tests::physical_large_transfer_resume_probe", "--", "--ignored", "--nocapture"],
  env: {
    MEGLE_SYNC_LARGE_TRANSFER_BYTES: largeBytes,
    MEGLE_SYNC_LARGE_TRANSFER_ROOT: largeTransferRoot,
    MEGLE_SYNC_BUNDLED_PLUGINS_DIR: path.join(targetRoot, "release", "bundled-plugins"),
  },
  runDirectory,
  timeoutMs: 3 * 60 * 60 * 1000,
}));
let physicalLargeTransfer = null;
const physicalRow = rows.find((candidate) => candidate.name === "physical-large-transfer-resume");
try {
  if (physicalRow?.status === "passed") {
    physicalLargeTransfer = parseMetric(
      await readFile(physicalRow.stdoutPath, "utf8"),
      "SYNC_LARGE_TRANSFER_JSON",
    );
  }
} finally {
  if (options.cleanupLargeTransfer) {
    try {
    await rm(largeTransferRoot, { recursive: true, force: true });
    } catch (error) {
      rows.push({
        name: "physical-large-transfer-cleanup",
        command: `remove ${largeTransferRoot}`,
        cwd: process.cwd(),
        startedAt: new Date().toISOString(),
        durationMs: 0,
        exitCode: 1,
        signal: null,
        status: "failed",
        required: true,
        reason: error.message,
      });
    }
  }
}
rows.push(await rendererEvidenceRow(options.rendererEvidence));

const result = {
  schemaVersion: 1,
  kind: "sync-integration-benchmark",
  runDirectory,
  quick: options.quick,
  configuredAssets: Number(assets),
  configuredLargeTransferBytes: Number(largeBytes),
  largeTransferRoot,
  physicalLargeTransfer,
  rendererEvidence: options.rendererEvidence ? path.resolve(options.rendererEvidence) : null,
  pairedPerformance,
  environment: await environmentFacts(),
  rows,
  status: requiredFailures(rows).length === 0 ? "passed" : "failed",
};
await writeJson(path.join(runDirectory, "benchmark-result.json"), result);
console.log(`SYNC_INTEGRATION_BENCH_JSON=${JSON.stringify(result)}`);
if (result.status !== "passed") process.exitCode = 1;

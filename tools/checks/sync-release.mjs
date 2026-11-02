import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { commandExists, createRunDirectory, environmentFacts, fileEvidence, requiredFailures, root, runCommand, writeJson } from "./sync-release-lib.mjs";

function parseArgs(argv) {
  const options = { output: null, quick: false, allowDirty: false, rendererEvidence: process.env.MANGOO_SYNC_RENDERER_EVIDENCE ?? null };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "--output" && argv[index + 1]) options.output = argv[++index];
    else if (value === "--quick") options.quick = true;
    else if (value === "--allow-dirty") options.allowDirty = true;
    else if (value === "--renderer-evidence" && argv[index + 1]) options.rendererEvidence = argv[++index];
    else throw new Error(`unknown or incomplete argument: ${value}`);
  }
  return options;
}

async function capture(command, args, cwd = root) {
  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    const child = spawn(command, args, { cwd, shell: process.platform === "win32", windowsHide: true });
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("error", (error) => resolve({ status: 1, stdout, stderr: `${stderr}${error.message}` }));
    child.on("close", (status) => resolve({ status: status ?? 1, stdout, stderr }));
  });
}

const options = parseArgs(process.argv.slice(2));
const runDirectory = await createRunDirectory("release", options.output);
const rows = [];
const commit = (await capture("git", ["rev-parse", "HEAD"])).stdout.trim();
let extracted = null;
let rendererEvidence = options.rendererEvidence ? path.resolve(options.rendererEvidence) : null;
const status = await capture("git", ["status", "--porcelain=v1"]);
const dirty = status.status !== 0 || status.stdout.trim().length > 0;
if (rendererEvidence && !existsSync(rendererEvidence)) {
  rows.push({ name: "renderer-performance-evidence", status: "failed", required: true, reason: `renderer evidence does not exist: ${rendererEvidence}` });
} else if (rendererEvidence) {
  try {
    const parsed = JSON.parse(await readFile(rendererEvidence, "utf8"));
    const mismatches = [];
    if (parsed.kind !== "renderer-sync-performance") mismatches.push("unexpected evidence kind");
    if (typeof parsed.runId !== "string" || parsed.runId.length < 16) mismatches.push("missing stable collector run identity");
    if (parsed.commit !== commit) mismatches.push(`commit ${parsed.commit ?? "missing"} != ${commit}`);
    if (parsed.dirty !== false) mismatches.push("collector did not run from a clean tree");
    if (parsed.exactTree !== true) mismatches.push("collector evidence is not exact-tree bound");
    if (!parsed.machine?.hostname || !parsed.machine?.platform || !parsed.machine?.cpu) mismatches.push("missing machine identity");
    if (!parsed.fixture?.digest) mismatches.push("missing fixture identity");
    if (!Number.isFinite(Date.parse(parsed.capturedAt))) mismatches.push("missing capture timestamp");
    if (parsed.passed !== true) mismatches.push("collector did not pass");
    rows.push({
      name: "renderer-performance-evidence",
      status: mismatches.length ? "failed" : "passed",
      required: true,
      reason: mismatches.length ? mismatches.join("; ") : null,
      evidence: await fileEvidence(rendererEvidence),
    });
  } catch (error) {
    rows.push({ name: "renderer-performance-evidence", status: "failed", required: true, reason: `renderer evidence is invalid JSON: ${error.message}` });
  }
}
if (dirty) {
  rows.push({ name: "immutable-tree", status: "blocked", required: true, reason: "worktree is dirty; commit before release evidence", details: status.stdout.trim() });
}
if (dirty && options.allowDirty) {
  rows.push({ name: "dirty-tree-diagnostic", status: "passed", required: false, reason: "current working-tree diagnostics were already run separately; immutable release rows remain blocked until commit" });
}
if (!dirty) {
  const archive = path.join(runDirectory, "source.tar");
  const archiveRow = await runCommand({ name: "git-archive", command: "git", args: ["archive", "--format=tar", `--output=${archive}`, commit], runDirectory });
  rows.push(archiveRow);
  if (archiveRow.status === "passed") {
    extracted = path.join(runDirectory, "exact-tree");
    await mkdir(extracted, { recursive: true });
    rows.push(await runCommand({ name: "extract-archive", command: "tar", args: ["-xf", archive, "-C", extracted], runDirectory }));
    if (rows.at(-1).status === "passed") {
      rows.push(await runCommand({ name: "exact-tree-npm-ci", command: "npm", args: ["ci", "--no-audit", "--no-fund"], cwd: extracted, runDirectory, timeoutMs: 30 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-legacy-selector-adoption-absence", command: "node", args: ["tools/checks/sync-legacy-absence.mjs"], cwd: extracted, runDirectory }));
      rows.push(await runCommand({ name: "exact-tree-full-gate", command: "npm", args: ["test"], cwd: extracted, runDirectory, timeoutMs: 2 * 60 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-bundled-artifacts", command: "node", args: ["tools/build/build-bundled-plugins.mjs", "--release"], cwd: extracted, runDirectory, timeoutMs: 2 * 60 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-desktop-plugin-link-boundary", command: "node", args: ["--no-warnings", "--import", "./tests/filter-sqlite-experimental-warning.mjs", "--import", "./tests/register-source-imports.mjs", "--experimental-strip-types", "--test", "tests/plugin-external-link.test.mts"], cwd: path.join(extracted, "apps", "desktop"), runDirectory, timeoutMs: 15 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-sync-integration", command: "node", args: ["tools/checks/sync-integration.mjs", "--output", path.join(runDirectory, "exact-integration"), ...(options.quick ? ["--quick"] : [])], cwd: extracted, runDirectory, timeoutMs: 2 * 60 * 60 * 1000 }));
      if (!rendererEvidence && process.platform === "win32") {
        rendererEvidence = path.join(runDirectory, "exact-renderer-evidence.json");
        rows.push(await runCommand({
          name: "exact-tree-renderer-performance",
          command: "node",
          args: ["tools/bench/renderer-sync-performance.mjs", "--output", rendererEvidence, ...(options.quick ? ["--quick"] : [])],
          cwd: extracted,
          env: { MANGOO_EVIDENCE_COMMIT: commit, MANGOO_EVIDENCE_EXACT_TREE: "1" },
          runDirectory,
          timeoutMs: 45 * 60 * 1000,
        }));
      }
      const largeTransferRoot = await mkdtemp(path.join(os.tmpdir(), "mangoo-sync-large-"));
      rows.push(await runCommand({ name: "exact-tree-sync-benchmark", command: "node", args: ["tools/bench/sync-integration-bench.mjs", "--output", path.join(runDirectory, "exact-benchmark"), "--large-transfer-root", largeTransferRoot, "--cleanup-large-transfer", ...(options.quick ? ["--quick"] : []), ...(rendererEvidence ? ["--renderer-evidence", rendererEvidence] : [])], cwd: extracted, env: { MANGOO_EVIDENCE_COMMIT: commit, MANGOO_EVIDENCE_EXACT_TREE: "1" }, runDirectory, timeoutMs: 5 * 60 * 60 * 1000 }));
    }
    await writeJson(path.join(runDirectory, "archive.json"), { commit, archive: await fileEvidence(archive) });
  }
}
if (!rendererEvidence) {
  rows.push({
    name: "renderer-performance-evidence",
    status: "blocked",
    required: true,
    reason: process.platform === "win32"
      ? "exact committed tree is unavailable, so the renderer collector could not run"
      : "renderer collection requires the Windows desktop compositor",
  });
}

if (process.platform === "win32") {
  const backdrop = extracted ? path.join(extracted, "tools", "checks", "desktop-os-backdrop.mjs") : null;
  if (backdrop && existsSync(backdrop)) {
    rows.push(await runCommand({ name: "windows-os-backdrop", command: "node", args: ["tools/checks/desktop-os-backdrop.mjs", "--output", path.join(runDirectory, "windows-os-backdrop")], cwd: extracted, env: { MANGOO_VISUAL_OS_BACKDROP: "1", MANGOO_EVIDENCE_COMMIT: commit, MANGOO_EVIDENCE_EXACT_TREE: "1" }, runDirectory, timeoutMs: 45 * 60 * 1000 }));
  } else {
    rows.push({ name: "windows-os-backdrop", status: "blocked", required: true, reason: "the committed archive does not contain a release-owned OS-backdrop harness" });
  }
} else {
  rows.push({ name: "windows-os-backdrop", status: "not-applicable", required: false, reason: "release runner is not executing on Windows" });
}

if (await commandExists("docker", ["info"]) && extracted) {
  const image = `mangoo:sync-release-${commit.slice(0, 12)}`;
  const verifierImage = `${image}-verifier`;
  rows.push(await runCommand({ name: "docker-linux-build", command: "docker", args: ["build", "--progress=plain", "-t", image, "."], cwd: extracted, runDirectory, timeoutMs: 3 * 60 * 60 * 1000 }));
  if (rows.at(-1).status === "passed") {
    rows.push(await runCommand({ name: "docker-linux-verifier-build", command: "docker", args: ["build", "--progress=plain", "--target", "sync-verifier", "-t", verifierImage, "."], cwd: extracted, runDirectory, timeoutMs: 3 * 60 * 60 * 1000 }));
    const dockerData = await mkdtemp(path.join(os.tmpdir(), "mangoo-release-docker-"));
    const key = path.join(dockerData, "secret.key");
    await writeFile(key, randomBytes(32));
    const volume = `mangoo-sync-release-${Date.now()}`;
    rows.push(await runCommand({ name: "docker-linux-runtime", command: "docker", args: ["run", "--rm", "--user", "10001:10001", "--mount", `type=bind,source=${key},target=/run/secrets/mangoo_secret_key,readonly`, "--mount", `type=volume,source=${volume},target=/data`, "--entrypoint", "/bin/sh", image, "-lc", "test -w /data && test ! -w /opt/mangoo/plugins && test $(id -u) -eq 10001 && test $(wc -c </run/secrets/mangoo_secret_key) -eq 32"], runDirectory, timeoutMs: 10 * 60 * 1000 }));
    const container = `${volume}-core`;
    rows.push(await runCommand({ name: "docker-linux-start", command: "docker", args: ["run", "-d", "--name", container, "--user", "10001:10001", "-e", "MANGOO_BASIC_AUTH=release:release-canary", "--mount", `type=bind,source=${key},target=/run/secrets/mangoo_secret_key,readonly`, "--mount", `type=volume,source=${volume},target=/data`, image], runDirectory, timeoutMs: 10 * 60 * 1000 }));
    if (rows.at(-1).status === "passed") {
      rows.push(await runCommand({ name: "docker-linux-health", command: "docker", args: ["exec", container, "/bin/sh", "-lc", "for i in $(seq 1 60); do wget -qO- http://127.0.0.1:47321/api/health && exit 0; sleep 1; done; exit 1"], runDirectory, timeoutMs: 2 * 60 * 1000 }));
      rows.push(await runCommand({ name: "docker-linux-stop", command: "docker", args: ["rm", "-f", container], runDirectory, timeoutMs: 2 * 60 * 1000 }));
    }
    rows.push(await runCommand({ name: "docker-linux-volume-restart", command: "docker", args: ["run", "--rm", "--user", "10001:10001", "--mount", `type=bind,source=${key},target=/run/secrets/mangoo_secret_key,readonly`, "--mount", `type=volume,source=${volume},target=/data`, "--entrypoint", "/bin/sh", image, "-lc", "test -d /data && test -w /data && test ! -w /opt/mangoo/plugins"], runDirectory, timeoutMs: 10 * 60 * 1000 }));
    if (rows.find((row) => row.name === "docker-linux-verifier-build")?.status === "passed") {
      const linuxLargeBytes = options.quick ? String(128 * 1024 * 1024) : String(10 * 1024 * 1024 * 1024);
      const persistenceCommand = "cargo test --release -p mangoo-core library_manager::tests::docker_volume_restart_persists_library_catalog_and_sync_state -- --ignored --nocapture";
      rows.push(await runCommand({ name: "docker-linux-persistence-create", command: "docker", args: ["run", "--rm", "--mount", `type=volume,source=${volume},target=/evidence`, "-e", "MANGOO_DOCKER_PERSISTENCE_PHASE=create", "-e", "MANGOO_DOCKER_PERSISTENCE_ROOT=/evidence/persistence", "-e", "MANGOO_SYNC_BUNDLED_PLUGINS_DIR=/src/target/release/bundled-plugins", "--entrypoint", "/bin/sh", verifierImage, "-lc", persistenceCommand], runDirectory, timeoutMs: 30 * 60 * 1000 }));
      if (rows.at(-1).status === "passed") {
        rows.push(await runCommand({ name: "docker-linux-persistence-verify-after-restart", command: "docker", args: ["run", "--rm", "--mount", `type=volume,source=${volume},target=/evidence`, "-e", "MANGOO_DOCKER_PERSISTENCE_PHASE=verify", "-e", "MANGOO_DOCKER_PERSISTENCE_ROOT=/evidence/persistence", "-e", "MANGOO_SYNC_BUNDLED_PLUGINS_DIR=/src/target/release/bundled-plugins", "--entrypoint", "/bin/sh", verifierImage, "-lc", persistenceCommand], runDirectory, timeoutMs: 30 * 60 * 1000 }));
      }
      rows.push(await runCommand({ name: "docker-linux-sync-integration", command: "docker", args: ["run", "--rm", "--mount", `type=volume,source=${volume},target=/evidence`, "-e", "MANGOO_SYNC_BUNDLED_PLUGINS_DIR=/src/target/release/bundled-plugins", "-e", `MANGOO_SYNC_LARGE_TRANSFER_BYTES=${linuxLargeBytes}`, "-e", "MANGOO_SYNC_LARGE_TRANSFER_ROOT=/evidence/large-transfer", "--entrypoint", "/bin/sh", verifierImage, "-lc", "cargo test --release -p mangoo-core library_manager::tests::clean_managed_bootstrap_reopens_current_schema -- --nocapture && cargo test --release -p mangoo-core library_manager::tests::real_provider_two_device_and_third_device_restore -- --ignored --nocapture && cargo test --release -p mangoo-core sync::test_model::hundred_thousand_shuffled_and_duplicated_events_converge -- --nocapture && cargo test --release -p mangoo-core sync::persistence::tests::checkpoint_uses_external_bounded_media_indexes_and_restores_chunk_locations -- --nocapture && cargo test --release -p mangoo-core sync::coordinator::tests::process_kill_fault_matrix_preserves_atomic_boundaries_and_recovery -- --nocapture && cargo test --release -p mangoo-core sync::media::worker::tests::physical_large_transfer_resume_probe -- --ignored --nocapture"], runDirectory, timeoutMs: 5 * 60 * 60 * 1000 }));
    }
  }
} else {
  rows.push({ name: "docker-linux", status: "blocked", required: true, reason: extracted ? "Docker CLI/daemon is unavailable" : "exact committed tree is unavailable" });
}
const result = {
  schemaVersion: 1,
  kind: "sync-release",
  scope: "host-generic",
  runDirectory,
  quick: options.quick,
  commit,
  dirtyDiagnostic: dirty,
  environment: await environmentFacts(),
  rows,
  status: requiredFailures(rows).length === 0 ? "passed" : "blocked",
};
await writeJson(path.join(runDirectory, "release-result.json"), result);
console.log(`SYNC_RELEASE_JSON=${JSON.stringify(result)}`);
if (result.status !== "passed") process.exitCode = 1;

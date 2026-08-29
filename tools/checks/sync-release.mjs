import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { commandExists, createRunDirectory, environmentFacts, fileEvidence, requiredFailures, root, runCommand, writeJson } from "./sync-release-lib.mjs";

function parseArgs(argv) {
  const options = { output: null, quick: false, allowDirty: false, rendererEvidence: process.env.MEGLE_SYNC_RENDERER_EVIDENCE ?? null };
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

const PRODUCT_OAUTH_FIELDS = new Set(["code", "state", "error", "error_description", "error_uri", "session_state"]);

async function productOAuthEvidenceRow({ name, file, commit, expected }) {
  if (!file) {
    return { name, status: "blocked", required: true, reason: `${expected.label} evidence path is absent` };
  }
  const resolved = path.resolve(file);
  if (!existsSync(resolved)) {
    return { name, status: "blocked", required: true, reason: `${expected.label} evidence file is absent` };
  }
  try {
    const evidence = JSON.parse(await readFile(resolved, "utf8"));
    const mismatches = [];
    if (evidence.schemaVersion !== 1) mismatches.push("unsupported schemaVersion");
    if (evidence.kind !== "onedrive-product-oauth") mismatches.push("unexpected evidence kind");
    if (evidence.commit !== commit) mismatches.push("commit does not match release tree");
    if (!Number.isFinite(Date.parse(evidence.capturedAt))) mismatches.push("invalid capturedAt");
    if (evidence.passed !== true) mismatches.push("canary did not pass");
    if (evidence.sensitiveMaterialPresent !== false) mismatches.push("evidence is not marked secret-free");
    if (evidence.clientSecretUsed !== false) mismatches.push("public-client flow did not prove client-secret absence");
    if (evidence.pkceS256 !== true) mismatches.push("PKCE S256 was not proven");
    if (evidence.secretStorePersisted !== true) mismatches.push("SecretStore persistence was not proven");
    if (evidence.refreshedAfterExpiry !== true) mismatches.push("refresh after access-token expiry was not proven");
    if (expected.accountKind && evidence.accountKind !== expected.accountKind) mismatches.push(`accountKind is not ${expected.accountKind}`);
    if (expected.platform && evidence.platform !== expected.platform) mismatches.push(`platform is not ${expected.platform}`);
    if (expected.completionMode && evidence.completionMode !== expected.completionMode) mismatches.push(`completionMode is not ${expected.completionMode}`);
    if (expected.redirectUri) {
      if (evidence.redirectUri !== expected.redirectUri) mismatches.push("redirectUri does not match the native-client redirect");
    } else if (expected.completionMode === "loopback" && !/^http:\/\/localhost:\d+\/plugin-auth\/callback$/.test(evidence.redirectUri ?? "")) {
      mismatches.push("redirectUri is not the ephemeral localhost callback");
    }
    if (expected.browserOpened && evidence.browserOpened !== true) mismatches.push("system-browser launch was not proven");
    if (expected.containerRestart && evidence.containerRestartPersisted !== true) mismatches.push("container restart persistence was not proven");
    const fields = evidence.observedResponseFields;
    if (!Array.isArray(fields) || fields.length === 0 || fields.some((field) => typeof field !== "string" || !PRODUCT_OAUTH_FIELDS.has(field))) {
      mismatches.push("observedResponseFields is absent or outside the allowlist");
    }
    if (Array.isArray(fields) && fields.includes("code") === fields.includes("error")) {
      mismatches.push("observed response must contain exactly one of code or error");
    }
    return {
      name,
      status: mismatches.length ? "failed" : "passed",
      required: true,
      reason: mismatches.length ? mismatches.join("; ") : null,
      evidence: await fileEvidence(resolved),
    };
  } catch (error) {
    return { name, status: "failed", required: true, reason: `evidence is invalid JSON: ${error.message}` };
  }
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
      rows.push(await runCommand({ name: "exact-tree-self-managed-onedrive-artifact", command: "node", args: ["tools/build/build-bundled-plugins.mjs", "--release", "--verify-onedrive"], cwd: extracted, runDirectory, timeoutMs: 2 * 60 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-onedrive-desktop-loopback-emulator", command: "cargo", args: ["test", "-p", "megle-onedrive-provider", "--features", "graph-emulator", "--test", "process_conformance", "oauth_poll_is_non_destructive_and_cancel_is_idempotent", "--", "--exact"], cwd: extracted, runDirectory, timeoutMs: 15 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-onedrive-docker-manual-emulator", command: "cargo", args: ["test", "-p", "megle-onedrive-provider", "--features", "graph-emulator", "--test", "process_conformance", "manual_auth_uses_native_client_redirect", "--", "--exact"], cwd: extracted, runDirectory, timeoutMs: 15 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-desktop-oauth-url-boundary", command: "node", args: ["--no-warnings", "--import", "./tests/filter-sqlite-experimental-warning.mjs", "--import", "./tests/register-source-imports.mjs", "--experimental-strip-types", "--test", "tests/oauth-authorization-url.test.mts"], cwd: path.join(extracted, "apps", "desktop"), runDirectory, timeoutMs: 15 * 60 * 1000 }));
      rows.push(await runCommand({ name: "exact-tree-sync-integration", command: "node", args: ["tools/checks/sync-integration.mjs", "--output", path.join(runDirectory, "exact-integration"), ...(options.quick ? ["--quick"] : [])], cwd: extracted, runDirectory, timeoutMs: 2 * 60 * 60 * 1000 }));
      if (!rendererEvidence && process.platform === "win32") {
        rendererEvidence = path.join(runDirectory, "exact-renderer-evidence.json");
        rows.push(await runCommand({
          name: "exact-tree-renderer-performance",
          command: "node",
          args: ["tools/bench/renderer-sync-performance.mjs", "--output", rendererEvidence, ...(options.quick ? ["--quick"] : [])],
          cwd: extracted,
          env: { MEGLE_EVIDENCE_COMMIT: commit, MEGLE_EVIDENCE_EXACT_TREE: "1" },
          runDirectory,
          timeoutMs: 45 * 60 * 1000,
        }));
      }
      const largeTransferRoot = await mkdtemp(path.join(os.tmpdir(), "megle-sync-large-"));
      rows.push(await runCommand({ name: "exact-tree-sync-benchmark", command: "node", args: ["tools/bench/sync-integration-bench.mjs", "--output", path.join(runDirectory, "exact-benchmark"), "--large-transfer-root", largeTransferRoot, "--cleanup-large-transfer", ...(options.quick ? ["--quick"] : []), ...(rendererEvidence ? ["--renderer-evidence", rendererEvidence] : [])], cwd: extracted, env: { MEGLE_EVIDENCE_COMMIT: commit, MEGLE_EVIDENCE_EXACT_TREE: "1" }, runDirectory, timeoutMs: 5 * 60 * 60 * 1000 }));
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
    rows.push(await runCommand({ name: "windows-os-backdrop", command: "node", args: ["tools/checks/desktop-os-backdrop.mjs", "--output", path.join(runDirectory, "windows-os-backdrop")], cwd: extracted, env: { MEGLE_VISUAL_OS_BACKDROP: "1", MEGLE_EVIDENCE_COMMIT: commit, MEGLE_EVIDENCE_EXACT_TREE: "1" }, runDirectory, timeoutMs: 45 * 60 * 1000 }));
  } else {
    rows.push({ name: "windows-os-backdrop", status: "blocked", required: true, reason: "the committed archive does not contain a release-owned OS-backdrop harness" });
  }
} else {
  rows.push({ name: "windows-os-backdrop", status: "not-applicable", required: false, reason: "release runner is not executing on Windows" });
}

if (await commandExists("docker", ["info"]) && extracted) {
  const image = `megle:sync-release-${commit.slice(0, 12)}`;
  const verifierImage = `${image}-verifier`;
  rows.push(await runCommand({ name: "docker-linux-build", command: "docker", args: ["build", "--progress=plain", "-t", image, "."], cwd: extracted, runDirectory, timeoutMs: 3 * 60 * 60 * 1000 }));
  if (rows.at(-1).status === "passed") {
    rows.push(await runCommand({ name: "docker-linux-verifier-build", command: "docker", args: ["build", "--progress=plain", "--target", "sync-verifier", "-t", verifierImage, "."], cwd: extracted, runDirectory, timeoutMs: 3 * 60 * 60 * 1000 }));
    const dockerData = await mkdtemp(path.join(os.tmpdir(), "megle-release-docker-"));
    const key = path.join(dockerData, "secret.key");
    await writeFile(key, randomBytes(32));
    const volume = `megle-sync-release-${Date.now()}`;
    rows.push(await runCommand({ name: "docker-linux-runtime", command: "docker", args: ["run", "--rm", "--user", "10001:10001", "--mount", `type=bind,source=${key},target=/run/secrets/megle_secret_key,readonly`, "--mount", `type=volume,source=${volume},target=/data`, "--entrypoint", "/bin/sh", image, "-lc", "test -w /data && test ! -w /opt/megle/plugins && test $(id -u) -eq 10001 && test $(wc -c </run/secrets/megle_secret_key) -eq 32"], runDirectory, timeoutMs: 10 * 60 * 1000 }));
    const container = `${volume}-core`;
    rows.push(await runCommand({ name: "docker-linux-start", command: "docker", args: ["run", "-d", "--name", container, "--user", "10001:10001", "-e", "MEGLE_BASIC_AUTH=release:release-canary", "--mount", `type=bind,source=${key},target=/run/secrets/megle_secret_key,readonly`, "--mount", `type=volume,source=${volume},target=/data`, image], runDirectory, timeoutMs: 10 * 60 * 1000 }));
    if (rows.at(-1).status === "passed") {
      rows.push(await runCommand({ name: "docker-linux-health", command: "docker", args: ["exec", container, "/bin/sh", "-lc", "for i in $(seq 1 60); do wget -qO- http://127.0.0.1:47321/api/health && exit 0; sleep 1; done; exit 1"], runDirectory, timeoutMs: 2 * 60 * 1000 }));
      rows.push(await runCommand({ name: "docker-linux-stop", command: "docker", args: ["rm", "-f", container], runDirectory, timeoutMs: 2 * 60 * 1000 }));
    }
    rows.push(await runCommand({ name: "docker-linux-volume-restart", command: "docker", args: ["run", "--rm", "--user", "10001:10001", "--mount", `type=bind,source=${key},target=/run/secrets/megle_secret_key,readonly`, "--mount", `type=volume,source=${volume},target=/data`, "--entrypoint", "/bin/sh", image, "-lc", "test -d /data && test -w /data && test ! -w /opt/megle/plugins"], runDirectory, timeoutMs: 10 * 60 * 1000 }));
    if (rows.find((row) => row.name === "docker-linux-verifier-build")?.status === "passed") {
      const linuxLargeBytes = options.quick ? String(128 * 1024 * 1024) : String(10 * 1024 * 1024 * 1024);
      const persistenceCommand = "cargo test --release -p megle-core library_manager::tests::docker_volume_restart_persists_library_catalog_and_sync_state -- --ignored --nocapture";
      rows.push(await runCommand({ name: "docker-linux-persistence-create", command: "docker", args: ["run", "--rm", "--mount", `type=volume,source=${volume},target=/evidence`, "-e", "MEGLE_DOCKER_PERSISTENCE_PHASE=create", "-e", "MEGLE_DOCKER_PERSISTENCE_ROOT=/evidence/persistence", "-e", "MEGLE_SYNC_BUNDLED_PLUGINS_DIR=/src/target/release/bundled-plugins", "--entrypoint", "/bin/sh", verifierImage, "-lc", persistenceCommand], runDirectory, timeoutMs: 30 * 60 * 1000 }));
      if (rows.at(-1).status === "passed") {
        rows.push(await runCommand({ name: "docker-linux-persistence-verify-after-restart", command: "docker", args: ["run", "--rm", "--mount", `type=volume,source=${volume},target=/evidence`, "-e", "MEGLE_DOCKER_PERSISTENCE_PHASE=verify", "-e", "MEGLE_DOCKER_PERSISTENCE_ROOT=/evidence/persistence", "-e", "MEGLE_SYNC_BUNDLED_PLUGINS_DIR=/src/target/release/bundled-plugins", "--entrypoint", "/bin/sh", verifierImage, "-lc", persistenceCommand], runDirectory, timeoutMs: 30 * 60 * 1000 }));
      }
      rows.push(await runCommand({ name: "docker-onedrive-manual-oauth-emulator", command: "docker", args: ["run", "--rm", "--entrypoint", "/bin/sh", verifierImage, "-lc", "cargo test --release -p megle-onedrive-provider --features graph-emulator --test process_conformance manual_auth_uses_native_client_redirect -- --exact"], runDirectory, timeoutMs: 30 * 60 * 1000 }));
      rows.push(await runCommand({ name: "docker-linux-sync-integration", command: "docker", args: ["run", "--rm", "--mount", `type=volume,source=${volume},target=/evidence`, "-e", "MEGLE_SYNC_BUNDLED_PLUGINS_DIR=/src/target/release/bundled-plugins", "-e", `MEGLE_SYNC_LARGE_TRANSFER_BYTES=${linuxLargeBytes}`, "-e", "MEGLE_SYNC_LARGE_TRANSFER_ROOT=/evidence/large-transfer", "--entrypoint", "/bin/sh", verifierImage, "-lc", "cargo test --release -p megle-core library_manager::tests::clean_managed_bootstrap_reopens_current_schema -- --nocapture && cargo test --release -p megle-core library_manager::tests::real_provider_two_device_and_third_device_restore -- --ignored --nocapture && cargo test --release -p megle-core sync::test_model::hundred_thousand_shuffled_and_duplicated_events_converge -- --nocapture && cargo test --release -p megle-core sync::persistence::tests::checkpoint_uses_external_bounded_media_indexes_and_restores_chunk_locations -- --nocapture && cargo test --release -p megle-core sync::coordinator::tests::process_kill_fault_matrix_preserves_atomic_boundaries_and_recovery -- --nocapture && cargo test --release -p megle-core sync::media::worker::tests::physical_large_transfer_resume_probe -- --ignored --nocapture"], runDirectory, timeoutMs: 5 * 60 * 60 * 1000 }));
    }
  }
} else {
  rows.push({ name: "docker-linux", status: "blocked", required: true, reason: extracted ? "Docker CLI/daemon is unavailable" : "exact committed tree is unavailable" });
}
if (!rows.some((row) => row.name === "docker-onedrive-manual-oauth-emulator")) {
  rows.push({ name: "docker-onedrive-manual-oauth-emulator", status: "blocked", required: true, reason: extracted ? "Docker build/runtime did not reach the manual OAuth verifier" : "exact committed tree is unavailable" });
}

const personal = process.env.MEGLE_ONEDRIVE_PERSONAL_ACCESS_TOKEN;
const business = process.env.MEGLE_ONEDRIVE_BUSINESS_ACCESS_TOKEN;
for (const [name, kind, token] of [["onedrive-personal-canary", "personal", personal], ["onedrive-business-canary", "business", business]]) {
  if (!token) rows.push({ name, status: "blocked", required: true, reason: "credential-gated canary token is absent" });
  else rows.push(await runCommand({
    name,
    command: "cargo",
    args: ["test", "-p", "megle-onedrive-provider", "--test", "live_graph", "delta_immutable_range_and_upload_session_round_trip", "--", "--ignored", "--nocapture"],
    cwd: extracted ?? root,
    env: { MEGLE_ONEDRIVE_LIVE_ACCESS_TOKEN: token, MEGLE_ONEDRIVE_EXPECT_ACCOUNT_KIND: kind },
    runDirectory,
    timeoutMs: 30 * 60 * 1000,
  }));
}

rows.push(await productOAuthEvidenceRow({
  name: "onedrive-personal-product-oauth",
  file: process.env.MEGLE_ONEDRIVE_PERSONAL_OAUTH_EVIDENCE,
  commit,
  expected: { label: "Personal Product OAuth", accountKind: "personal" },
}));
rows.push(await productOAuthEvidenceRow({
  name: "onedrive-business-product-oauth",
  file: process.env.MEGLE_ONEDRIVE_BUSINESS_OAUTH_EVIDENCE,
  commit,
  expected: { label: "Business Product OAuth", accountKind: "business" },
}));
rows.push(await productOAuthEvidenceRow({
  name: "onedrive-desktop-loopback-product-oauth",
  file: process.env.MEGLE_ONEDRIVE_DESKTOP_LOOPBACK_EVIDENCE,
  commit,
  expected: { label: "Desktop loopback Product OAuth", platform: "windows", completionMode: "loopback", browserOpened: true },
}));
rows.push(await productOAuthEvidenceRow({
  name: "onedrive-docker-manual-product-oauth",
  file: process.env.MEGLE_ONEDRIVE_DOCKER_MANUAL_EVIDENCE,
  commit,
  expected: { label: "Docker manual response-URI Product OAuth", platform: "docker-linux", completionMode: "manual-response-uri", redirectUri: "https://login.microsoftonline.com/common/oauth2/nativeclient", containerRestart: true },
}));

const result = {
  schemaVersion: 1,
  kind: "sync-release",
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

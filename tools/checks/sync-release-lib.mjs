import { createHash, randomUUID } from "node:crypto";
import { createReadStream, createWriteStream, existsSync } from "node:fs";
import { mkdir, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";

export const root = path.resolve(import.meta.dirname, "../..");

export function runId(prefix) {
  return `${new Date().toISOString().replace(/[:.]/g, "-")}-${prefix}-${randomUUID().slice(0, 8)}`;
}

export async function createRunDirectory(prefix, requested) {
  const directory = path.resolve(requested ?? path.join(root, ".tmp", "sync-release", runId(prefix)));
  await mkdir(path.join(directory, "logs"), { recursive: true });
  await mkdir(path.join(directory, "results"), { recursive: true });
  return directory;
}

export async function sha256File(file) {
  const hash = createHash("sha256");
  const stream = createReadStream(file);
  for await (const chunk of stream) hash.update(chunk);
  return hash.digest("hex");
}

export function hashJson(value) {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

export async function writeJson(file, value) {
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

export async function runCommand({
  name,
  command,
  args = [],
  cwd = root,
  env = {},
  runDirectory,
  timeoutMs = 60 * 60 * 1000,
  required = true,
}) {
  const startedAt = new Date().toISOString();
  const started = performance.now();
  const stdoutPath = path.join(runDirectory, "logs", `${name}.stdout.log`);
  const stderrPath = path.join(runDirectory, "logs", `${name}.stderr.log`);
  const stdout = createWriteStream(stdoutPath, { flags: "wx" });
  const stderr = createWriteStream(stderrPath, { flags: "wx" });
  let status = "failed";
  let reason = null;
  let exitCode = null;
  let signal = null;
  try {
    const result = await new Promise((resolve) => {
      const child = spawn(command, args, {
        cwd,
        env: { ...process.env, ...env },
        shell: process.platform === "win32",
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      });
      const timer = setTimeout(() => child.kill(), timeoutMs);
      child.stdout.pipe(stdout);
      child.stderr.pipe(stderr);
      child.on("error", (error) => resolve({ error }));
      child.on("close", (code, childSignal) => {
        clearTimeout(timer);
        resolve({ code, signal: childSignal });
      });
    });
    if (result.error) {
      reason = result.error.message;
    } else {
      exitCode = result.code;
      signal = result.signal;
      status = result.code === 0 ? "passed" : "failed";
      if (result.code !== 0) reason = `${command} exited ${result.code}${result.signal ? ` (${result.signal})` : ""}`;
    }
  } finally {
    await Promise.all([
      new Promise((resolve) => stdout.end(resolve)),
      new Promise((resolve) => stderr.end(resolve)),
    ]);
  }
  const row = {
    name,
    command: [command, ...args].join(" "),
    cwd,
    startedAt,
    durationMs: Math.round(performance.now() - started),
    exitCode,
    signal,
    status,
    required,
    reason,
    stdoutPath,
    stderrPath,
  };
  await writeJson(path.join(runDirectory, "results", `${name}.json`), row);
  return row;
}

export async function commandExists(command, args = ["--version"]) {
  return new Promise((resolve) => {
    const child = spawn(command, args, { shell: process.platform === "win32", windowsHide: true, stdio: "ignore" });
    child.on("error", () => resolve(false));
    child.on("close", (code) => resolve(code === 0));
  });
}

export async function environmentFacts() {
  return {
    capturedAt: new Date().toISOString(),
    platform: process.platform,
    arch: process.arch,
    node: process.version,
    osRelease: os.release(),
    osVersion: os.version(),
    cpuModel: os.cpus()[0]?.model ?? null,
    cpuCount: os.cpus().length,
    totalMemoryBytes: os.totalmem(),
  };
}

export async function fileEvidence(file) {
  if (!existsSync(file)) return null;
  const info = await stat(file);
  return { path: file, bytes: info.size, sha256: await sha256File(file) };
}

export function requiredFailures(rows) {
  return rows.filter((row) => row.required && row.status !== "passed");
}

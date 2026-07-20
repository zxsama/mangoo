import type { ChildProcess } from "node:child_process";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { StringDecoder } from "node:string_decoder";
import { fileURLToPath } from "node:url";
import type { CoreSession } from "./core-session.js";
import type { ProcessLogLevel } from "./process-log-buffer.js";

export interface CoreProcessHandle {
  process: ChildProcess | null;
  waitForExit: Promise<void>;
  stop: () => Promise<void>;
}

export type CoreProcessLogHandler = (
  source: "core",
  level: Extract<ProcessLogLevel, "info" | "warn" | "error">,
  message: string
) => void;

export interface CoreProcessLifecycle {
  readonly waitForExit: Promise<void>;
  readonly stop: () => Promise<void>;
}

export function createCoreOutputLineSplitter(onLine: (line: string) => void): {
  push: (chunk: string | Uint8Array) => void;
  flush: () => void;
} {
  let pending = "";
  const decoder = new StringDecoder("utf8");
  return {
    push(chunk) {
      pending += decoder.write(typeof chunk === "string" ? Buffer.from(chunk, "utf8") : chunk);
      const lines = pending.split(/\r?\n/);
      pending = lines.pop() ?? "";
      for (const line of lines) onLine(line);
    },
    flush() {
      pending += decoder.end();
      const lines = pending.split(/\r?\n/);
      pending = lines.pop() ?? "";
      for (const line of lines) onLine(line);
      if (pending.length === 0) return;
      onLine(pending);
      pending = "";
    }
  };
}

export function startCoreProcess(
  session: CoreSession,
  onLog: CoreProcessLogHandler = defaultCoreProcessLogHandler
): CoreProcessHandle {
  if (process.env.MEGLE_CORE_EXTERNAL === "1") {
    const waitForExit = Promise.resolve();
    return {
      process: null,
      waitForExit,
      stop: async () => undefined
    };
  }

  const binaryPath = coreBinaryPath();
  if (!fs.existsSync(binaryPath)) {
    throw new Error(
      `Core debug binary is missing at ${binaryPath}. Run "cargo build -p megle-core" before starting internal desktop Core.`
    );
  }

  const child = spawn(binaryPath, [], {
    cwd: repoRoot(),
    env: {
      ...process.env,
      MEGLE_CORE_ADDR: session.bindAddress,
      MEGLE_DB_PATH: session.dbPath,
      MEGLE_SESSION_TOKEN: session.sessionToken,
      MEGLE_ALLOWED_ORIGIN: session.allowedOrigin
    },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true
  });

  const lifecycle = attachCoreProcessLifecycle(child, onLog);
  child.stdout?.on("data", (chunk: Buffer) => {
    process.stdout.write("[core stdout] ");
    process.stdout.write(chunk);
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    process.stderr.write("[core stderr] ");
    process.stderr.write(chunk);
  });

  return {
    process: child,
    waitForExit: lifecycle.waitForExit,
    stop: lifecycle.stop
  };
}

export function attachCoreProcessLifecycle(
  child: ChildProcess,
  onLog: CoreProcessLogHandler = defaultCoreProcessLogHandler
): CoreProcessLifecycle {
  const stdoutSplitter = createCoreOutputLineSplitter((line) => emitCoreLog(onLog, "info", line));
  const stderrSplitter = createCoreOutputLineSplitter((line) => emitCoreLog(onLog, "warn", line));
  let settled = false;
  let exitObserved = false;
  let stopState: "running" | "stopping" | "stopped" = "running";
  let stopPromise: Promise<void> | null = null;
  let resolveExit!: () => void;
  let exitCode: number | null = null;
  let exitSignal: NodeJS.Signals | null = null;
  let processError: unknown;
  const waitForExit = new Promise<void>((resolve) => {
    resolveExit = resolve;
  });

  const settle = (code: number | null, signal: NodeJS.Signals | null, error?: unknown): void => {
    if (settled) return;
    settled = true;
    const intentionalStop = stopState === "stopping";
    stopState = "stopped";
    stdoutSplitter.flush();
    stderrSplitter.flush();
    if (intentionalStop) {
      emitCoreLog(
        onLog,
        "info",
        `Core stopped intentionally code=${code} signal=${signal ?? "none"}`
      );
    } else if (error !== undefined) {
      emitCoreLog(onLog, "error", `Core process error: ${formatCoreError(error)}`);
    } else if (code !== 0 || signal !== null) {
      emitCoreLog(onLog, "error", `Core exited with code=${code} signal=${signal ?? "none"}`);
    }
    resolveExit();
  };

  child.stdout?.on("data", (chunk: Buffer) => stdoutSplitter.push(chunk));
  child.stderr?.on("data", (chunk: Buffer) => stderrSplitter.push(chunk));
  child.once("exit", (code, signal) => {
    exitObserved = true;
    exitCode = code;
    exitSignal = signal;
  });
  child.once("error", (error) => {
    processError = error;
  });
  child.once("close", (code, signal) => {
    settle(code ?? exitCode, signal ?? exitSignal, processError);
  });

  const stop = async (): Promise<void> => {
    if (settled) {
      await waitForExit;
      return;
    }
    if (exitObserved || processError !== undefined) {
      await waitForExit;
      return;
    }
    if (!stopPromise) {
      stopState = "stopping";
      stopPromise = (async () => {
        try {
          stopCoreProcessTree(child);
        } catch (error) {
          emitCoreLog(onLog, "error", `Core stop request failed: ${formatCoreError(error)}`);
        }
        const didExit = await waitForCoreExit(waitForExit, CORE_STOP_TIMEOUT_MS);
        if (!didExit) {
          stdoutSplitter.flush();
          stderrSplitter.flush();
          emitCoreLog(onLog, "error", `Core stop timed out after ${CORE_STOP_TIMEOUT_MS}ms`);
        }
      })();
    }
    await stopPromise;
  };

  return { waitForExit, stop };
}

export async function stopCoreProcesses(
  handles: readonly (CoreProcessHandle | null)[]
): Promise<void> {
  const stopped = new Set<CoreProcessHandle>();
  for (const handle of handles) {
    if (!handle || stopped.has(handle)) continue;
    stopped.add(handle);
    await handle.stop();
  }
}

const CORE_STOP_TIMEOUT_MS = 5_000;

function formatCoreError(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  try {
    return String(error);
  } catch {
    return "[unreadable error]";
  }
}

async function waitForCoreExit(waitForExit: Promise<void>, timeoutMs: number): Promise<boolean> {
  let timer: NodeJS.Timeout | undefined;
  const timedOut = new Promise<boolean>((resolve) => {
    timer = setTimeout(() => resolve(false), timeoutMs);
  });
  const didExit = await Promise.race([waitForExit.then(() => true), timedOut]);
  if (timer) clearTimeout(timer);
  return didExit;
}

function defaultCoreProcessLogHandler(
  _source: "core",
  level: Extract<ProcessLogLevel, "info" | "warn" | "error">,
  message: string
): void {
  const method = level === "info" ? "log" : level;
  const sink = console[method] ?? console.log;
  sink(`[core ${level}] ${message}`);
}

function emitCoreLog(
  onLog: CoreProcessLogHandler,
  level: Extract<ProcessLogLevel, "info" | "warn" | "error">,
  message: string
): void {
  try {
    onLog("core", level, message);
  } catch {
    // A log sink must not affect Core process lifecycle.
  }
}

export async function waitForCoreHealth(
  baseUrl: string,
  sessionToken: string,
  timeoutMs = 12_000
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(new URL("health", ensureTrailingSlash(baseUrl)), {
        headers: {
          "x-megle-session": sessionToken
        }
      });
      if (response.ok) {
        return;
      }
      lastError = new Error(`health returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }

  throw new Error(`Core did not become healthy: ${String(lastError)}`);
}

function repoRoot(): string {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
}

function coreBinaryPath(): string {
  const binaryName = process.platform === "win32" ? "megle-core.exe" : "megle-core";
  // MEGLE_CORE_PROFILE selects the cargo build profile directory. Defaults to
  // "debug" (the `npm run dev` flow). The production launcher sets "release".
  const profile = process.env.MEGLE_CORE_PROFILE === "release" ? "release" : "debug";
  return path.join(repoRoot(), "target", profile, binaryName);
}

function ensureTrailingSlash(value: string): string {
  return value.endsWith("/") ? value : `${value}/`;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function stopCoreProcessTree(child: ChildProcess): void {
  if (child.pid === undefined) {
    child.kill();
    return;
  }

  if (process.platform === "win32") {
    const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true
    });
    killer.on("error", () => {
      child.kill();
    });
    killer.on("exit", (code) => {
      if (code !== 0 && !child.killed) {
        child.kill();
      }
    });
    return;
  }

  if (!child.killed) {
    child.kill("SIGTERM");
    setTimeout(() => {
      if (!child.killed) {
        child.kill("SIGKILL");
      }
    }, 2_000).unref();
  }
}

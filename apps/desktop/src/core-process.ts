import type { ChildProcess } from "node:child_process";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { CoreSession } from "./core-session.js";
import type { ProcessLogLevel, ProcessLogMetadata } from "./process-log-buffer.js";
import { LifecycleCancelledError, throwIfLifecycleAborted } from "./core-lifecycle-queue.js";

export interface CoreProcessHandle {
  process: ChildProcess | null;
  waitForExit: Promise<void>;
  stop: () => Promise<void>;
}

export type CoreProcessLogHandler = (
  source: "core",
  level: ProcessLogLevel,
  message: string,
  metadata?: ProcessLogMetadata
) => void;

export interface ParsedCoreProcessLogLine {
  level: ProcessLogLevel;
  message: string;
  metadata?: ProcessLogMetadata;
}

const CORE_LOG_LEVELS = new Set<ProcessLogLevel>(["trace", "debug", "info", "warn", "error"]);
const CORE_METADATA_KEYS: Readonly<Record<string, string>> = Object.freeze({
  event_kind: "eventKind",
  file_id: "fileId",
  folder_id: "folderId",
  root_id: "rootId",
  relative_path: "relativePath",
  failure_kind: "failureKind"
});

export function parseCoreProcessLogLine(
  line: string,
  fallbackLevel: Extract<ProcessLogLevel, "info" | "warn" | "error">
): ParsedCoreProcessLogLine {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return { level: fallbackLevel, message: line };
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { level: fallbackLevel, message: line };
  }
  const record = parsed as Record<string, unknown>;
  const rawLevel = typeof record.level === "string" ? record.level.toLowerCase() : "";
  const fields = record.fields;
  if (
    !CORE_LOG_LEVELS.has(rawLevel as ProcessLogLevel) ||
    !fields ||
    typeof fields !== "object" ||
    Array.isArray(fields) ||
    typeof (fields as Record<string, unknown>).message !== "string"
  ) {
    return { level: fallbackLevel, message: line };
  }
  const fieldRecord = fields as Record<string, unknown>;
  const metadata: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(fieldRecord)) {
    if (key === "message") continue;
    metadata[CORE_METADATA_KEYS[key] ?? key] = value;
  }
  if (typeof record.target === "string") metadata.target = record.target;
  return {
    level: rawLevel as ParsedCoreProcessLogLine["level"],
    message: fieldRecord.message as string,
    ...(Object.keys(metadata).length > 0 ? { metadata } : {})
  };
}

export interface CoreProcessLifecycle {
  readonly waitForExit: Promise<void>;
  readonly stop: () => Promise<void>;
}

export interface CoreProcessLifecycleOptions {
  readonly platform?: NodeJS.Platform;
  readonly gracefulTimeoutMs?: number;
  readonly forceTimeoutMs?: number;
  readonly requestWindowsStop?: (child: ChildProcess) => void;
}

const CORE_OUTPUT_PARTIAL_MAX_BYTES = 12 * 1024;

export function createCoreOutputLineSplitter(onLine: (line: string) => void): {
  push: (chunk: string | Uint8Array) => void;
  flush: () => void;
} {
  const pending = Buffer.allocUnsafe(CORE_OUTPUT_PARTIAL_MAX_BYTES);
  let pendingLength = 0;
  let omittedBytes = 0;

  const append = (bytes: Buffer): void => {
    const kept = Math.min(bytes.length, CORE_OUTPUT_PARTIAL_MAX_BYTES - pendingLength);
    if (kept > 0) {
      bytes.copy(pending, pendingLength, 0, kept);
      pendingLength += kept;
    }
    omittedBytes += bytes.length - kept;
  };

  const emitPending = (stripCarriageReturn: boolean): void => {
    let visibleLength = pendingLength;
    if (stripCarriageReturn && visibleLength > 0 && pending[visibleLength - 1] === 0x0d) {
      visibleLength -= 1;
    }
    if (omittedBytes > 0) {
      const completeLength = completeUtf8PrefixLength(pending, visibleLength);
      omittedBytes += visibleLength - completeLength;
      const prefix = pending.subarray(0, completeLength).toString("utf8");
      onLine(`${prefix}${prefix.length > 0 ? " " : ""}[truncated ${omittedBytes} bytes]`);
    } else {
      onLine(pending.subarray(0, visibleLength).toString("utf8"));
    }
    pendingLength = 0;
    omittedBytes = 0;
  };

  return {
    push(chunk) {
      const bytes = typeof chunk === "string"
        ? Buffer.from(chunk, "utf8")
        : Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength);
      let offset = 0;
      while (offset < bytes.length) {
        const newline = bytes.indexOf(0x0a, offset);
        if (newline < 0) {
          append(bytes.subarray(offset));
          return;
        }
        append(bytes.subarray(offset, newline));
        emitPending(true);
        offset = newline + 1;
      }
    },
    flush() {
      if (pendingLength === 0 && omittedBytes === 0) return;
      emitPending(false);
    }
  };
}

function completeUtf8PrefixLength(bytes: Buffer, length: number): number {
  if (length === 0) return 0;
  let leadIndex = length - 1;
  while (leadIndex >= 0 && (bytes[leadIndex]! & 0xc0) === 0x80) leadIndex -= 1;
  if (leadIndex < 0) return 0;
  const lead = bytes[leadIndex]!;
  const expectedLength =
    (lead & 0x80) === 0 ? 1 :
    (lead & 0xe0) === 0xc0 ? 2 :
    (lead & 0xf0) === 0xe0 ? 3 :
    (lead & 0xf8) === 0xf0 ? 4 : 1;
  return leadIndex + expectedLength > length ? leadIndex : length;
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
      ...(session.hostDbPath
        ? {
            MEGLE_HOST_DB_PATH: session.hostDbPath,
            MEGLE_LIBRARIES_DIR: session.librariesDir
          }
        : {}),
      MEGLE_SESSION_TOKEN: session.sessionToken,
      MEGLE_ALLOWED_ORIGIN: session.allowedOrigin,
      MEGLE_PLUGINS_DIR: process.env.MEGLE_PLUGINS_DIR ?? bundledPluginsDir()
    },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true
  });

  const lifecycle = attachCoreProcessLifecycle(child, onLog);

  return {
    process: child,
    waitForExit: lifecycle.waitForExit,
    stop: lifecycle.stop
  };
}

export function attachCoreProcessLifecycle(
  child: ChildProcess,
  onLog: CoreProcessLogHandler = defaultCoreProcessLogHandler,
  options: CoreProcessLifecycleOptions = {}
): CoreProcessLifecycle {
  const stdoutSplitter = createCoreOutputLineSplitter((line) => {
    const parsed = parseCoreProcessLogLine(line, "info");
    emitCoreLog(onLog, parsed.level, parsed.message, parsed.metadata);
  });
  const stderrSplitter = createCoreOutputLineSplitter((line) => {
    const parsed = parseCoreProcessLogLine(line, "warn");
    emitCoreLog(onLog, parsed.level, parsed.message, parsed.metadata);
  });
  const platform = options.platform ?? process.platform;
  const gracefulTimeoutMs = options.gracefulTimeoutMs ?? CORE_GRACEFUL_STOP_TIMEOUT_MS;
  const forceTimeoutMs = options.forceTimeoutMs ?? CORE_FORCE_STOP_TIMEOUT_MS;
  const requestWindowsStop = options.requestWindowsStop ?? stopCoreProcessTree;
  let settled = false;
  let exitObserved = false;
  let stopRequested = false;
  let stopPromise: Promise<void> | null = null;
  let resolveExit!: () => void;
  let resolveTermination!: () => void;
  let exitCode: number | null = null;
  let exitSignal: NodeJS.Signals | null = null;
  let processError: unknown;
  const waitForExit = new Promise<void>((resolve) => {
    resolveExit = resolve;
  });
  const waitForTermination = new Promise<void>((resolve) => {
    resolveTermination = resolve;
  });

  const settle = (code: number | null, signal: NodeJS.Signals | null, error?: unknown): void => {
    if (settled) return;
    settled = true;
    stdoutSplitter.flush();
    stderrSplitter.flush();
    if (stopRequested) {
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
    resolveTermination();
    resolveExit();
  };

  child.stdout?.on("data", (chunk: Buffer) => stdoutSplitter.push(chunk));
  child.stderr?.on("data", (chunk: Buffer) => stderrSplitter.push(chunk));
  child.once("exit", (code, signal) => {
    exitObserved = true;
    exitCode = code;
    exitSignal = signal;
    resolveTermination();
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
    if (stopPromise) return stopPromise;

    const attempt = (async () => {
      if (!exitObserved && processError === undefined) {
        stopRequested = true;
        if (platform === "win32") {
          try {
            requestWindowsStop(child);
          } catch (error) {
            emitCoreLog(onLog, "error", `Core taskkill request failed: ${formatCoreError(error)}`);
            requestCoreSignal(child, "SIGKILL", onLog);
          }
        } else {
          requestCoreSignal(child, "SIGTERM", onLog);
          const terminated = await waitForCoreEvent(waitForTermination, gracefulTimeoutMs);
          if (!terminated) requestCoreSignal(child, "SIGKILL", onLog);
        }
      }

      const didClose = await waitForCoreEvent(waitForExit, forceTimeoutMs);
      if (didClose) return;
      stdoutSplitter.flush();
      stderrSplitter.flush();
      const totalTimeoutMs = platform === "win32"
        ? forceTimeoutMs
        : gracefulTimeoutMs + forceTimeoutMs;
      const error = new Error(`Core stop timed out after ${totalTimeoutMs}ms without close`);
      emitCoreLog(onLog, "error", error.message);
      throw error;
    })();
    stopPromise = attempt;
    try {
      await attempt;
    } finally {
      if (!settled && stopPromise === attempt) stopPromise = null;
    }
  };

  return { waitForExit, stop };
}

export async function stopCoreProcesses(
  handles: readonly (CoreProcessHandle | null)[]
): Promise<void> {
  const stopped = new Set<CoreProcessHandle>();
  let firstError: unknown;
  for (const handle of handles) {
    if (!handle || stopped.has(handle)) continue;
    stopped.add(handle);
    try {
      await handle.stop();
    } catch (error) {
      firstError ??= error;
    }
  }
  if (firstError !== undefined) throw firstError;
}

export async function stopCoreProcessHandleAndClear(
  handle: CoreProcessHandle | null,
  clear: () => void
): Promise<void> {
  if (!handle) return;
  await handle.stop();
  clear();
}

export async function stopCoreProcessesBeforeReplacement(
  pending: CoreProcessHandle | null,
  current: CoreProcessHandle | null,
  clearPending: () => void,
  clearCurrent: () => void
): Promise<void> {
  await stopCoreProcessHandleAndClear(pending, clearPending);
  await stopCoreProcessHandleAndClear(current, clearCurrent);
}

export async function stopCoreProcessesAndCloseLogger(
  handles: readonly (CoreProcessHandle | null)[],
  closeLogger: () => Promise<void>
): Promise<void> {
  let stopError: unknown;
  try {
    await stopCoreProcesses(handles);
  } catch (error) {
    stopError = error;
  }
  await closeLogger();
  if (stopError !== undefined) throw stopError;
}

const CORE_GRACEFUL_STOP_TIMEOUT_MS = 2_000;
const CORE_FORCE_STOP_TIMEOUT_MS = 5_000;

function formatCoreError(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  try {
    return String(error);
  } catch {
    return "[unreadable error]";
  }
}

async function waitForCoreEvent(event: Promise<void>, timeoutMs: number): Promise<boolean> {
  let timer: NodeJS.Timeout | undefined;
  const timedOut = new Promise<boolean>((resolve) => {
    timer = setTimeout(() => resolve(false), timeoutMs);
  });
  const didExit = await Promise.race([event.then(() => true), timedOut]);
  if (timer) clearTimeout(timer);
  return didExit;
}

function defaultCoreProcessLogHandler(
  _source: "core",
  level: ProcessLogLevel,
  message: string,
  _metadata?: ProcessLogMetadata
): void {
  const method = level === "info" ? "log" : level === "trace" ? "debug" : level;
  const sink = console[method] ?? console.log;
  sink(`[core ${level}] ${message}`);
}

function emitCoreLog(
  onLog: CoreProcessLogHandler,
  level: ProcessLogLevel,
  message: string,
  metadata?: ProcessLogMetadata
): void {
  try {
    onLog("core", level, message, metadata);
  } catch {
    // A log sink must not affect Core process lifecycle.
  }
}

export async function waitForCoreHealth(
  baseUrl: string,
  sessionToken: string,
  timeoutMs = 12_000,
  signal?: AbortSignal
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;

  while (Date.now() < deadline) {
    if (signal) throwIfLifecycleAborted(signal);
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) break;
    const requestAbort = createHealthRequestAbort(remainingMs, signal);
    try {
      const response = await fetch(new URL("health", ensureTrailingSlash(baseUrl)), {
        headers: {
          "x-megle-session": sessionToken
        },
        signal: requestAbort.signal
      });
      if (response.ok) {
        return;
      }
      lastError = new Error(`health returned ${response.status}`);
    } catch (error) {
      if (signal?.aborted) throw new LifecycleCancelledError();
      if (requestAbort.timedOut()) {
        throw coreHealthTimeoutError(timeoutMs, error);
      }
      lastError = error;
    } finally {
      requestAbort.dispose();
    }
    const delayMs = Math.min(250, deadline - Date.now());
    if (delayMs > 0) await delay(delayMs, signal);
  }

  throw coreHealthTimeoutError(timeoutMs, lastError);
}

interface HealthRequestAbort {
  readonly signal: AbortSignal;
  readonly timedOut: () => boolean;
  readonly dispose: () => void;
}

function createHealthRequestAbort(
  remainingMs: number,
  lifecycleSignal?: AbortSignal
): HealthRequestAbort {
  const controller = new AbortController();
  let timeoutReached = false;
  let lifecycleListenerAttached = false;
  const onLifecycleAbort = () => {
    controller.abort(lifecycleSignal?.reason);
  };

  if (lifecycleSignal) {
    if (lifecycleSignal.aborted) {
      onLifecycleAbort();
    } else {
      lifecycleSignal.addEventListener("abort", onLifecycleAbort, { once: true });
      lifecycleListenerAttached = true;
    }
  }

  const timeoutId = setTimeout(() => {
    timeoutReached = true;
    controller.abort(new DOMException("Core health request timed out", "TimeoutError"));
  }, remainingMs);

  return {
    signal: controller.signal,
    timedOut: () => timeoutReached,
    dispose: () => {
      clearTimeout(timeoutId);
      if (lifecycleSignal && lifecycleListenerAttached) {
        lifecycleSignal.removeEventListener("abort", onLifecycleAbort);
        lifecycleListenerAttached = false;
      }
    }
  };
}

function coreHealthTimeoutError(timeoutMs: number, lastError: unknown): Error {
  return new Error(
    `Core did not become healthy within ${timeoutMs}ms: ${String(lastError ?? "health request timed out")}`
  );
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

function bundledPluginsDir(): string {
  const profile = process.env.MEGLE_CORE_PROFILE === "release" ? "release" : "debug";
  return path.join(repoRoot(), "target", profile, "bundled-plugins");
}

function ensureTrailingSlash(value: string): string {
  return value.endsWith("/") ? value : `${value}/`;
}

function delay(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (!signal) {
      setTimeout(resolve, ms);
      return;
    }
    let settled = false;
    const cleanup = () => signal.removeEventListener("abort", onAbort);
    const timer = setTimeout(() => {
      settled = true;
      cleanup();
      resolve();
    }, ms);
    const onAbort = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      cleanup();
      reject(new LifecycleCancelledError());
    };
    if (signal.aborted) {
      onAbort();
      return;
    }
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

function stopCoreProcessTree(child: ChildProcess): void {
  if (child.pid === undefined) {
    child.kill("SIGKILL");
    return;
  }

  const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
    stdio: "ignore",
    windowsHide: true
  });
  killer.once("error", () => {
    try {
      child.kill("SIGKILL");
    } catch {
      // The lifecycle timeout reports failure if the child still does not close.
    }
  });
  killer.once("exit", (code) => {
    if (code === 0) return;
    try {
      child.kill("SIGKILL");
    } catch {
      // The lifecycle timeout reports failure if the child still does not close.
    }
  });
}

function requestCoreSignal(
  child: ChildProcess,
  signal: NodeJS.Signals,
  onLog: CoreProcessLogHandler
): void {
  try {
    child.kill(signal);
  } catch (error) {
    emitCoreLog(onLog, "error", `Core ${signal} request failed: ${formatCoreError(error)}`);
  }
}

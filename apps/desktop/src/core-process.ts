import type { ChildProcess } from "node:child_process";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { CoreSession } from "./core-session.js";
import type { ProcessLogLevel } from "./process-log-buffer.js";

export interface CoreProcessHandle {
  process: ChildProcess | null;
  stop: () => void;
}

export type CoreProcessLogHandler = (
  source: "core",
  level: Extract<ProcessLogLevel, "info" | "warn" | "error">,
  message: string
) => void;

export function createCoreOutputLineSplitter(onLine: (line: string) => void): {
  push: (chunk: string) => void;
  flush: () => void;
} {
  let pending = "";
  return {
    push(chunk) {
      pending += chunk;
      const lines = pending.split(/\r?\n/);
      pending = lines.pop() ?? "";
      for (const line of lines) onLine(line);
    },
    flush() {
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
    return {
      process: null,
      stop: () => undefined
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

  const stdoutSplitter = createCoreOutputLineSplitter((line) => emitCoreLog(onLog, "info", line));
  const stderrSplitter = createCoreOutputLineSplitter((line) => emitCoreLog(onLog, "warn", line));
  child.stdout?.on("data", (chunk: Buffer) => {
    const text = chunk.toString();
    process.stdout.write(`[core stdout] ${text}`);
    stdoutSplitter.push(text);
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    const text = chunk.toString();
    process.stderr.write(`[core stderr] ${text}`);
    stderrSplitter.push(text);
  });
  child.on("exit", (code, signal) => {
    stdoutSplitter.flush();
    stderrSplitter.flush();
    if (code !== 0 && signal !== "SIGTERM") {
      emitCoreLog(
        onLog,
        "error",
        `Core exited with code=${code} signal=${signal ?? "none"}`
      );
    }
  });

  return {
    process: child,
    stop: () => {
      stopCoreProcessTree(child);
    }
  };
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

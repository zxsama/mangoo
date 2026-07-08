import type { ChildProcess } from "node:child_process";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { CoreSession } from "./core-session.js";

export interface CoreProcessHandle {
  process: ChildProcess | null;
  stop: () => void;
}

export function startCoreProcess(session: CoreSession): CoreProcessHandle {
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

  child.stdout?.on("data", (chunk: Buffer) => {
    process.stdout.write(`[core] ${chunk.toString()}`);
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    process.stderr.write(`[core] ${chunk.toString()}`);
  });
  child.on("exit", (code, signal) => {
    if (code !== 0 && signal !== "SIGTERM") {
      console.error(`[core] exited with code=${code} signal=${signal ?? "none"}`);
    }
  });

  return {
    process: child,
    stop: () => {
      stopCoreProcessTree(child);
    }
  };
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

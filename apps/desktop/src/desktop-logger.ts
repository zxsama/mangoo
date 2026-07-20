import {
  ProcessLogBuffer,
  type ProcessLogLevel,
  type ProcessLogMetadata,
  type ProcessLogSource,
  formatProcessLogValue
} from "./process-log-buffer.js";

type ConsoleMethod = (...args: unknown[]) => void;
type ConsoleSink = Partial<Record<ProcessLogLevel, ConsoleMethod>> & {
  log?: ConsoleMethod;
};

interface NormalizedLog {
  readonly source: ProcessLogSource;
  readonly level: ProcessLogLevel;
  readonly message: string;
  readonly metadata?: ProcessLogMetadata;
}

export interface DesktopLoggerOptions {
  readonly buffer?: ProcessLogBuffer;
  readonly console?: ConsoleSink;
}

export class DesktopLogger {
  readonly buffer: ProcessLogBuffer;
  private readonly console: ConsoleSink;
  private pending: NormalizedLog[] = [];
  private scheduled = false;
  private closed = false;
  private readonly flushWaiters = new Set<() => void>();

  constructor(options: DesktopLoggerOptions = {}) {
    this.buffer = options.buffer ?? new ProcessLogBuffer();
    this.console = options.console ?? console;
  }

  log(source: ProcessLogSource, level: ProcessLogLevel, ...args: unknown[]): void {
    this.writeConsole(level, args);
    if (this.closed) return;
    const normalized = normalizeLog(source, level, args);
    this.pending.push(normalized);
    this.scheduleDrain();
  }

  trace(source: ProcessLogSource, ...args: unknown[]): void {
    this.log(source, "trace", ...args);
  }

  debug(source: ProcessLogSource, ...args: unknown[]): void {
    this.log(source, "debug", ...args);
  }

  info(source: ProcessLogSource, ...args: unknown[]): void {
    this.log(source, "info", ...args);
  }

  warn(source: ProcessLogSource, ...args: unknown[]): void {
    this.log(source, "warn", ...args);
  }

  error(source: ProcessLogSource, ...args: unknown[]): void {
    this.log(source, "error", ...args);
  }

  async flush(): Promise<void> {
    if (!this.scheduled && this.pending.length === 0) {
      await this.buffer.flush();
      return;
    }
    await new Promise<void>((resolve) => {
      this.flushWaiters.add(resolve);
      this.scheduleDrain();
    });
    await this.buffer.flush();
  }

  async close(): Promise<void> {
    await this.flush();
    this.closed = true;
    await this.buffer.flush();
  }

  private writeConsole(level: ProcessLogLevel, args: unknown[]): void {
    const method = this.console[level] ?? this.console.log;
    if (!method) return;
    try {
      method(...args);
    } catch {
      // Console failures are not allowed to affect browsing or child processes.
    }
  }

  private scheduleDrain(): void {
    if (this.scheduled || this.closed) return;
    this.scheduled = true;
    queueMicrotask(() => this.drain());
  }

  private drain(): void {
    this.scheduled = false;
    const pending = this.pending;
    this.pending = [];
    for (const line of pending) {
      try {
        this.buffer.append(line.source, line.level, line.message, line.metadata);
      } catch {
        // Logging must be best-effort and never break the application.
      }
    }
    if (this.pending.length > 0) {
      this.scheduleDrain();
      return;
    }
    for (const resolve of this.flushWaiters) resolve();
    this.flushWaiters.clear();
  }
}

function normalizeLog(source: ProcessLogSource, level: ProcessLogLevel, args: unknown[]): NormalizedLog {
  const metadata: Record<string, unknown> = {};
  const rendered: string[] = [];
  for (const [index, arg] of args.entries()) {
    if (arg instanceof Error) {
      rendered.push(formatProcessLogValue(arg));
      metadata.error = arg;
      continue;
    }
    rendered.push(formatProcessLogValue(arg));
    if (typeof arg === "object" && arg !== null) {
      metadata[`argument${index + 1}`] = arg;
    }
  }
  return {
    source,
    level,
    message: rendered.join(" "),
    metadata: Object.keys(metadata).length > 0 ? metadata : undefined
  };
}

export const processLogBuffer = new ProcessLogBuffer();
export const desktopLogger = new DesktopLogger({ buffer: processLogBuffer });

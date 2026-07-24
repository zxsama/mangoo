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
  private readonly consoleMethods: Partial<Record<ProcessLogLevel, ConsoleMethod>>;
  private closed = false;
  private logging = false;

  constructor(options: DesktopLoggerOptions = {}) {
    this.buffer = options.buffer ?? new ProcessLogBuffer();
    this.consoleMethods = bindConsoleMethods(options.console ?? console);
  }

  log(source: ProcessLogSource, level: ProcessLogLevel, ...args: unknown[]): void {
    if (this.logging) return;
    this.logging = true;
    try {
      this.writeConsole(level, args);
      if (this.closed) return;
      let normalized: NormalizedLog;
      try {
        normalized = normalizeLog(source, level, args);
      } catch {
        normalized = {
          source,
          level,
          message: "[log serialization failed]"
        };
      }
      try {
        this.buffer.append(normalized.source, normalized.level, normalized.message, normalized.metadata);
      } catch {
        // Logging must be best-effort and never break the application.
      }
    } finally {
      this.logging = false;
    }
  }

  structured(
    source: ProcessLogSource,
    level: ProcessLogLevel,
    message: string,
    metadata?: ProcessLogMetadata
  ): void {
    if (this.logging) return;
    this.logging = true;
    try {
      this.writeConsole(level, metadata ? [message, metadata] : [message]);
      if (!this.closed) this.buffer.append(source, level, message, metadata);
    } catch {
      // Structured diagnostics must remain best-effort.
    } finally {
      this.logging = false;
    }
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
    await this.buffer.flush();
  }

  async close(): Promise<void> {
    this.closed = true;
    await this.buffer.flush();
  }

  private writeConsole(level: ProcessLogLevel, args: unknown[]): void {
    const method = this.consoleMethods[level];
    if (!method) return;
    try {
      method(...args);
    } catch {
      // Console failures are not allowed to affect browsing or child processes.
    }
  }

}

function bindConsoleMethods(sink: ConsoleSink): Partial<Record<ProcessLogLevel, ConsoleMethod>> {
  const methods: Partial<Record<ProcessLogLevel, ConsoleMethod>> = {};
  const levels: readonly ProcessLogLevel[] = ["trace", "debug", "info", "warn", "error"];
  for (const level of levels) {
    try {
      const method = sink[level] ?? sink.log;
      if (typeof method === "function") methods[level] = method.bind(sink);
    } catch {
      // A hostile console object must not prevent logger construction.
    }
  }
  return methods;
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

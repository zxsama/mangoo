import type {
  ProcessLogLevel,
  ProcessLogMetadata,
  ProcessLogSource
} from "./process-log-buffer.js";

export interface StartupLogSink {
  structured(
    source: ProcessLogSource,
    level: ProcessLogLevel,
    message: string,
    metadata?: ProcessLogMetadata
  ): void;
}

export type StartupLogEmitter = (metadata?: ProcessLogMetadata) => boolean;

export function createStartupLogEmitter(
  sink: StartupLogSink,
  source: ProcessLogSource,
  message: string,
  baseMetadata: ProcessLogMetadata = {}
): StartupLogEmitter {
  let emitted = false;
  return (metadata = {}) => {
    if (emitted) return false;
    emitted = true;
    sink.structured(source, "info", message, {
      ...baseMetadata,
      ...metadata,
      eventKind: "startup_ready"
    });
    return true;
  };
}

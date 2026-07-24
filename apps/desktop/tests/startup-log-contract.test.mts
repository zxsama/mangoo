import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { parseCoreProcessLogLine } from "../src/core-process.ts";
import { DesktopLogger } from "../src/desktop-logger.ts";
import { ProcessLogBuffer } from "../src/process-log-buffer.ts";
import { createStartupLogEmitter } from "../src/startup-logging.ts";

describe("clean startup log contract", () => {
  it("appends exactly one bounded startup entry for Main, Core, and Renderer", () => {
    const buffer = new ProcessLogBuffer({ maxEntries: 3, maxBytes: 4_096, now: () => 100 });
    const logger = new DesktopLogger({ buffer, console: {} });
    const mainReady = createStartupLogEmitter(logger, "main", "Megle desktop ready");
    const coreReady = createStartupLogEmitter(logger, "core", "Megle Core ready");
    const rendererReady = createStartupLogEmitter(logger, "renderer", "Megle renderer ready");

    for (const emit of [mainReady, coreReady, rendererReady]) {
      assert.equal(emit(), true);
      assert.equal(emit(), false);
    }

    const lines = buffer.snapshot();
    assert.equal(lines.length, 3);
    assert.deepEqual(lines.map((line) => line.source), ["main", "core", "renderer"]);
    assert.ok(lines.every((line) => line.level === "info"));
    assert.ok(lines.every((line) => line.metadata?.eventKind === "startup_ready"));
  });

  it("parses the Core ready event as a normal structured info log", () => {
    assert.deepEqual(parseCoreProcessLogLine(JSON.stringify({
      timestamp: "2026-07-25T00:00:00Z",
      level: "INFO",
      target: "megle_core",
      fields: {
        message: "Megle Core ready",
        event_kind: "startup_ready",
        bind_addr: "127.0.0.1:47321"
      }
    }), "warn"), {
      level: "info",
      message: "Megle Core ready",
      metadata: {
        eventKind: "startup_ready",
        bind_addr: "127.0.0.1:47321",
        target: "megle_core"
      }
    });
  });
});

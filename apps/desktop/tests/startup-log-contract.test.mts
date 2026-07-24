import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { parseCoreProcessLogLine } from "../src/core-process.ts";

const read = (relativePath: string) =>
  readFileSync(new URL(`../../../${relativePath}`, import.meta.url), "utf8");

describe("clean startup log contract", () => {
  it("keeps one normal startup emission wired for Main, Core, and Renderer", () => {
    assert.match(
      read("apps/desktop/src/main.ts"),
      /desktopLogger\.structured\("main", "info", "Megle desktop ready"/
    );
    assert.match(
      read("crates/core/src/main.rs"),
      /tracing::info!\([\s\S]*event_kind = "startup_ready"[\s\S]*"Megle Core ready"/
    );
    assert.match(
      read("apps/web/src/app/App.tsx"),
      /emitRendererStartupLog\(\)/
    );
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

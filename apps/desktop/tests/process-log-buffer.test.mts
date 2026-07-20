import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DEFAULT_PROCESS_LOG_MAX_BYTES,
  ProcessLogBuffer,
  sanitizeProcessLogMessage
} from "../src/process-log-buffer.ts";
import { DesktopLogger } from "../src/desktop-logger.ts";
import { createCoreOutputLineSplitter } from "../src/core-process.ts";

test("keeps the bounded ring in sequence order and evicts the oldest lines", () => {
  const buffer = new ProcessLogBuffer({ maxEntries: 3, maxBytes: 1024 });
  buffer.append("main", "info", "one");
  buffer.append("core", "warn", "two");
  buffer.append("renderer", "error", "three");
  buffer.append("main", "debug", "four");

  assert.deepEqual(buffer.snapshot({}).map((line) => line.message), ["two", "three", "four"]);
  assert.deepEqual(buffer.snapshot({}).map((line) => line.sequence), [2, 3, 4]);
  assert.equal(buffer.size, 3);
  assert.ok(buffer.bytes <= 1024);
});

test("enforces the UTF-8 byte cap, including multibyte text", () => {
  const buffer = new ProcessLogBuffer({ maxEntries: 100, maxBytes: 512 });
  for (let index = 0; index < 100; index += 1) {
    buffer.append("core", "info", `第${index}条日志 `.repeat(20));
  }

  assert.ok(buffer.bytes <= 512);
  assert.ok(buffer.size > 0);
  assert.ok(buffer.snapshot({}).every((line) => line.message.length > 0));
});

test("supports source, level, sequence and text filters", () => {
  const buffer = new ProcessLogBuffer({ now: () => 1234 });
  buffer.append("main", "info", "startup complete");
  buffer.append("core", "warn", "scan warning");
  buffer.append("renderer", "error", "preview failed");
  buffer.append("core", "debug", "scan details");

  assert.deepEqual(buffer.snapshot({ source: "core" }).map((line) => line.message), [
    "scan warning",
    "scan details"
  ]);
  assert.deepEqual(buffer.snapshot({ level: "error" }).map((line) => line.source), ["renderer"]);
  assert.deepEqual(buffer.snapshot({ search: "SCAN" }).map((line) => line.sequence), [2, 4]);
  assert.deepEqual(buffer.snapshot({ afterSequence: 1, beforeSequence: 4 }).map((line) => line.sequence), [2, 3]);
  assert.equal(buffer.snapshot({})[0]?.timestamp, 1234);
});

test("provides bounded pages and preserves the cursor for the next page", () => {
  const buffer = new ProcessLogBuffer();
  for (let index = 0; index < 5; index += 1) buffer.append("main", "info", String(index));

  const page = buffer.page({ limit: 2 });
  assert.deepEqual(page.lines.map((line) => line.message), ["3", "4"]);
  assert.equal(page.hasMore, true);
  assert.equal(page.nextBeforeSequence, 4);
  assert.deepEqual(buffer.snapshot({ beforeSequence: page.nextBeforeSequence ?? undefined, limit: 2 }).map((line) => line.message), ["1", "2"]);
});

test("redacts session tokens and sensitive paths from text and metadata", () => {
  const buffer = new ProcessLogBuffer();
  const circular: Record<string, unknown> = {
    sessionToken: "session-secret",
    filePath: "C:\\Users\\84460\\Pictures\\secret.jpg",
    nested: { value: "kept" }
  };
  circular.circular = circular;
  const line = buffer.append(
    "main",
    "error",
    "token=secret-value path=C:\\Users\\84460\\Megle\\library.sqlite",
    circular
  );

  assert.ok(line);
  assert.equal(line?.message.includes("secret-value"), false);
  assert.equal(line?.message.includes("C:\\Users"), false);
  assert.equal(line?.metadata?.sessionToken, "[redacted]");
  assert.equal(line?.metadata?.filePath, "[redacted]");
  assert.equal((line?.metadata?.circular as string | undefined), "[circular]");
  assert.equal(sanitizeProcessLogMessage("MEGLE_SESSION_TOKEN=abc"), "MEGLE_SESSION_TOKEN=[redacted]");
});

test("delivers subscriber lines asynchronously in append order", async () => {
  const buffer = new ProcessLogBuffer();
  const received: number[] = [];
  buffer.subscribe((line) => {
    received.push(line.sequence);
    if (line.sequence === 2) throw new Error("consumer failure");
  });
  buffer.append("main", "info", "one");
  buffer.append("main", "info", "two");
  buffer.append("main", "info", "three");
  assert.deepEqual(received, []);
  await buffer.flush();
  assert.deepEqual(received, [1, 2, 3]);
});

test("logger preserves the original console sink without monkey-patching it", async () => {
  const output: unknown[][] = [];
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({
    buffer,
    console: {
      log: (...args) => output.push(args),
      info: (...args) => output.push(args),
      warn: (...args) => output.push(args),
      error: (...args) => output.push(args)
    }
  });
  const originalConsoleLog = console.log;

  logger.info("main", "hello", { count: 2 });
  logger.error("renderer", new Error("render failed"));
  await logger.flush();

  assert.equal(console.log, originalConsoleLog);
  assert.deepEqual(output[0], ["hello", { count: 2 }]);
  assert.equal(buffer.snapshot({ source: "renderer" })[0]?.message, "Error: render failed");
  assert.deepEqual(buffer.snapshot({ source: "main" })[0]?.metadata?.argument2, { count: 2 });
});

test("logging remains best effort when the console sink or metadata is broken", async () => {
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({
    buffer,
    console: { error: () => { throw new Error("console unavailable"); } }
  });
  const circular: Record<string, unknown> = {};
  circular.self = circular;
  logger.error("main", "still recorded", circular);
  await logger.close();

  assert.equal(buffer.snapshot({})[0]?.message.includes("still recorded"), true);
  assert.equal(buffer.snapshot({})[0]?.message.includes("[circular]"), true);
  await buffer.shutdown();
});

test("splits Core stdout and stderr chunks without losing partial lines", () => {
  const stdout: string[] = [];
  const stderr: string[] = [];
  const stdoutSplitter = createCoreOutputLineSplitter((line) => stdout.push(line));
  const stderrSplitter = createCoreOutputLineSplitter((line) => stderr.push(line));

  stdoutSplitter.push("first\nsec");
  stdoutSplitter.push("ond\r\n");
  stderrSplitter.push("warning");
  stderrSplitter.flush();
  stdoutSplitter.flush();

  assert.deepEqual(stdout, ["first", "second"]);
  assert.deepEqual(stderr, ["warning"]);
});

test("5000-line default boundary remains bounded", () => {
  const buffer = new ProcessLogBuffer({ maxBytes: DEFAULT_PROCESS_LOG_MAX_BYTES });
  for (let index = 0; index < 5_001; index += 1) {
    buffer.append("core", "info", `line-${index}`);
  }
  assert.equal(buffer.size, 5_000);
  assert.equal(buffer.snapshot({})[0]?.message, "line-1");
  assert.ok(buffer.bytes <= DEFAULT_PROCESS_LOG_MAX_BYTES);
});

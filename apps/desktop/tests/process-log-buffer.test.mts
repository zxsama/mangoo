import assert from "node:assert/strict";
import fs from "node:fs";
import { spawn } from "node:child_process";
import { test } from "node:test";
import {
  DEFAULT_PROCESS_LOG_MAX_BYTES,
  ProcessLogBuffer,
  sanitizeProcessLogMessage
} from "../src/process-log-buffer.ts";
import { DesktopLogger } from "../src/desktop-logger.ts";
import {
  attachCoreProcessLifecycle,
  createCoreOutputLineSplitter,
  stopCoreProcesses
} from "../src/core-process.ts";

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

test("isolates metadata returned by append, snapshot, and page from the ring", () => {
  const buffer = new ProcessLogBuffer({ maxBytes: 1024 });
  const appended = buffer.append("main", "info", "stable", { nested: { value: "original" }, list: ["one"] });
  assert.ok(appended);
  (appended?.metadata?.nested as Record<string, unknown>).value = "changed";
  (appended?.metadata?.list as string[]).push("two");

  const snapshot = buffer.snapshot({})[0];
  assert.equal((snapshot?.metadata?.nested as Record<string, unknown>).value, "original");
  assert.deepEqual(snapshot?.metadata?.list, ["one"]);
  (snapshot?.metadata?.nested as Record<string, unknown>).value = "changed-again";

  const page = buffer.page({ limit: 1 }).lines[0];
  assert.equal((page?.metadata?.nested as Record<string, unknown>).value, "original");
  assert.ok(buffer.bytes <= 1024);
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

test("uses the same recursive redaction for structured messages and metadata", async () => {
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined } });
  const payload = {
    sessionToken: "top-secret",
    nested: [{ token: "nested-secret" }, { auth: { bearer: "bearer-secret" } }]
  };

  logger.info("main", payload);
  await logger.flush();
  const line = buffer.snapshot({})[0];
  assert.ok(line);
  assert.equal(line.message.includes("top-secret"), false);
  assert.equal(line.message.includes("nested-secret"), false);
  assert.equal(line.message.includes("bearer-secret"), false);
  assert.equal(JSON.stringify(line.metadata).includes("top-secret"), false);
  assert.equal(JSON.stringify(line.metadata).includes("nested-secret"), false);
  assert.equal(JSON.stringify(line.metadata).includes("bearer-secret"), false);
});

test("keeps append alive when getters, invalid dates, proxies, or structured clone fail", () => {
  const getter = {} as Record<string, unknown>;
  Object.defineProperty(getter, "throws", {
    enumerable: true,
    get: () => {
      throw new Error("getter failed");
    }
  });
  const unreadableProxy = new Proxy(
    {},
    {
      ownKeys: () => {
        throw new Error("proxy failed");
      }
    }
  );
  const cloneFailure = { callback: () => undefined, symbol: Symbol("not cloneable") };
  assert.throws(() => structuredClone(cloneFailure));

  const buffer = new ProcessLogBuffer();
  const line = buffer.append("main", "error", "retained", {
    getter,
    unreadableProxy,
    invalidDate: new Date(Number.NaN),
    cloneFailure
  });

  assert.ok(line);
  assert.equal(line?.message, "retained");
  assert.equal((line?.metadata?.getter as Record<string, unknown>)?.throws, "[unreadable]");
  assert.equal(line?.metadata?.unreadableProxy, "[unreadable]");
  assert.equal(line?.metadata?.invalidDate, "[invalid date]");
  assert.equal((line?.metadata?.cloneFailure as Record<string, unknown>)?.callback, "[unserializable]");
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

test("clear fences queued subscriber events while allowing later lines", async () => {
  const buffer = new ProcessLogBuffer();
  const received: string[] = [];
  buffer.subscribe((line) => received.push(line.message));

  buffer.append("main", "info", "before-clear");
  buffer.clear();
  await buffer.flush();
  assert.deepEqual(received, []);

  buffer.append("main", "info", "after-clear");
  await buffer.flush();
  assert.deepEqual(received, ["after-clear"]);
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
  assert.equal(buffer.snapshot({ source: "renderer" })[0]?.message.includes('"message":"render failed"'), true);
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

test("closed logger safely drops later buffer writes", async () => {
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined } });
  logger.info("main", "before-close");
  await logger.close();
  logger.info("main", "after-close");
  await logger.flush();

  assert.deepEqual(buffer.snapshot({}).map((line) => line.message), ["before-close"]);
});

test("splits Core stdout and stderr chunks without losing partial lines", () => {
  const stdout: string[] = [];
  const stderr: string[] = [];
  const stdoutSplitter = createCoreOutputLineSplitter((line) => stdout.push(line));
  const stderrSplitter = createCoreOutputLineSplitter((line) => stderr.push(line));

  stdoutSplitter.push("first\nsec");
  stdoutSplitter.push("ond\r\n");
  stdoutSplitter.push(Buffer.from([0xe4]));
  stdoutSplitter.push(Buffer.from([0xb8, 0xad, 0xf0, 0x9f]));
  stdoutSplitter.push(Buffer.from([0x98, 0x80, 0x0a]));
  stderrSplitter.push(Buffer.from([0xe8]));
  stderrSplitter.push(Buffer.from([0xad, 0xa6, 0xe5, 0x91]));
  stderrSplitter.push(Buffer.from([0x8a, 0x0a]));
  stderrSplitter.flush();
  stdoutSplitter.flush();

  assert.deepEqual(stdout, ["first", "second", "中😀"]);
  assert.deepEqual(stderr, ["警告"]);
});

test("real Core lifecycle drains both streams and records non-zero exit before stop resolves", async () => {
  const logs: Array<{ level: string; message: string }> = [];
  const child = spawn(
    process.execPath,
    [
      "-e",
      "process.stdout.write(Buffer.from([0xe4])); setTimeout(() => { process.stdout.write(Buffer.from([0xb8,0xad,0xf0,0x9f])); process.stderr.write(Buffer.from([0xe8])); setTimeout(() => { process.stdout.write(Buffer.from([0x98,0x80,0x0a])); process.stderr.write(Buffer.from([0xad,0xa6,0xe5,0x91,0x8a,0x0a])); process.exit(3); }, 5); }, 5);"
    ],
    { stdio: ["ignore", "pipe", "pipe"] }
  );
  const lifecycle = attachCoreProcessLifecycle(child, (_source, level, message) => {
    logs.push({ level, message });
  });

  await lifecycle.waitForExit;
  await lifecycle.stop();

  assert.deepEqual(logs.slice(0, 2), [
    { level: "info", message: "中😀" },
    { level: "warn", message: "警告" }
  ]);
  assert.equal(logs.at(-1)?.level, "error");
  assert.equal(logs.at(-1)?.message.includes("code=3"), true);
});

test("Core shutdown completes before the before-quit logger close step", async () => {
  const events: string[] = [];
  const handle = {
    process: null,
    waitForExit: Promise.resolve(),
    stop: async () => {
      await new Promise<void>((resolve) => setImmediate(resolve));
      events.push("core-stop-complete");
    }
  };

  await stopCoreProcesses([handle]);
  events.push("logger-close");
  assert.deepEqual(events, ["core-stop-complete", "logger-close"]);

  const main = fs.readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const beforeQuit = main.slice(main.indexOf('app.on("before-quit"'));
  assert.ok(beforeQuit.indexOf("stopCoreProcesses") < beforeQuit.indexOf("desktopLogger.close"));
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

import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { PassThrough } from "node:stream";
import { test } from "node:test";
import {
  DEFAULT_PROCESS_LOG_MAX_BYTES,
  ProcessLogBuffer,
  safeSerializeProcessLogValue,
  sanitizeProcessLogMessage,
  type ProcessLogBatch,
  type ProcessLogLine
} from "../src/process-log-buffer.ts";
import { DesktopLogger } from "../src/desktop-logger.ts";
import {
  attachCoreProcessLifecycle,
  createCoreOutputLineSplitter
} from "../src/core-process.ts";
import type { ProcessLogEntry } from "../src/notification-types.ts";

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

test("freezes append results and isolates snapshot and page clones from the ring", () => {
  const buffer = new ProcessLogBuffer({ maxBytes: 1024 });
  const appended = buffer.append("main", "info", "stable", { nested: { value: "original" }, list: ["one"] });
  assert.ok(appended);
  assert.equal(Object.isFrozen(appended), true);
  assert.equal(Object.isFrozen(appended?.metadata?.nested), true);
  assert.throws(() => {
    (appended?.metadata?.nested as Record<string, unknown>).value = "changed";
  });

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

test("redacts x-megle-session and synonymous headers in nested objects and raw text", async () => {
  const secrets = [
    "megle-secret",
    "megle-token-secret",
    "session-token-secret",
    "authorization-secret",
    "proxy-secret",
    "cookie-secret",
    "api-key-secret",
    "env-secret",
    "raw-megle-secret",
    "raw-auth-secret"
  ];
  const payload = {
    nested: {
      headers: {
        "x-megle-session": secrets[0],
        "X-Megle-Session-Token": secrets[1],
        "x-session-token": secrets[2],
        Authorization: `Bearer ${secrets[3]}`,
        "Proxy-Authorization": `Basic ${secrets[4]}`,
        Cookie: `sid=${secrets[5]}`,
        "X-Api-Key": secrets[6]
      }
    },
    list: [{ MEGLE_SESSION_TOKEN: secrets[7] }]
  };
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined, warn: () => undefined } });

  logger.info("renderer", payload);
  logger.warn(
    "renderer",
    `X-Megle-Session: ${secrets[8]}\nAuthorization: Bearer ${secrets[9]}`
  );
  await logger.flush();

  const snapshot = buffer.snapshot({});
  const serialized = JSON.stringify(snapshot);
  for (const secret of secrets) {
    assert.equal(snapshot.some((line) => line.message.includes(secret)), false);
    assert.equal(snapshot.some((line) => (JSON.stringify(line.metadata) ?? "").includes(secret)), false);
    assert.equal(serialized.includes(secret), false);
  }
});

test("redacts sensitive header tuples in nested metadata and JSON messages while preserving ordinary arrays", async () => {
  const secrets = [
    "tuple-session-secret",
    "tuple-auth-secret",
    "tuple-cookie-secret",
    "tuple-api-secret",
    "nested-session-secret",
    "nested-token-secret"
  ];
  const payload = {
    headers: [
      ["x-megle-session", secrets[0]],
      ["Authorization", secrets[1]],
      ["Cookie", secrets[2]],
      ["X-Api-Key", secrets[3]],
      ["content-type", "application/json"]
    ],
    nested: {
      headers: [
        ["x-session-token", secrets[4]],
        ["MEGLE_SESSION_TOKEN", secrets[5]],
        ["accept", "image/png"]
      ]
    },
    ordinary: ["kept", ["name", "value"]]
  };
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined } });

  logger.info("renderer", payload);
  await logger.flush();

  const line = buffer.snapshot({})[0];
  assert.ok(line);
  const metadata = line.metadata?.argument1 as Record<string, unknown>;
  const snapshot = JSON.stringify(buffer.snapshot({}));
  const exported = safeSerializeProcessLogValue(payload);
  for (const secret of secrets) {
    assert.equal(line.message.includes(secret), false);
    assert.equal(JSON.stringify(metadata).includes(secret), false);
    assert.equal(snapshot.includes(secret), false);
    assert.equal(exported.includes(secret), false);
  }
  assert.deepEqual(metadata.headers, [
    ["x-megle-session", "[redacted]"],
    ["Authorization", "[redacted]"],
    ["Cookie", "[redacted]"],
    ["X-Api-Key", "[redacted]"],
    ["content-type", "application/json"]
  ]);
  assert.deepEqual((metadata.nested as Record<string, unknown>).headers, [
    ["x-session-token", "[redacted]"],
    ["MEGLE_SESSION_TOKEN", "[redacted]"],
    ["accept", "image/png"]
  ]);
  assert.deepEqual(metadata.ordinary, ["kept", ["name", "value"]]);
});

test("redacts normalized credential keys, paired JSON values, and raw bearer tokens", async () => {
  const secrets = [
    "auth-token-secret",
    "id-token-secret",
    "csrf-token-secret",
    "client-secret-value",
    "password-secret",
    "api-key-secret",
    "raw-bearer-secret",
    "assignment-bearer-secret"
  ];
  const payload = {
    authToken: secrets[0],
    id_token: secrets[1],
    nested: {
      csrfToken: secrets[2],
      clientSecret: secrets[3],
      password: secrets[4],
      apiKey: secrets[5]
    }
  };
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined } });

  logger.info("renderer", payload);
  logger.info(
    "renderer",
    `json={"authToken":"${secrets[0]}"} tail Bearer ${secrets[6]}`
  );
  logger.info("renderer", `Authorization=Bearer ${secrets[7]}`);
  await logger.flush();

  const serialized = JSON.stringify(buffer.snapshot({}));
  for (const secret of secrets) assert.equal(serialized.includes(secret), false);
  assert.equal(
    buffer.snapshot({})[1]?.message,
    'json={"authToken":"[redacted]"} tail Bearer [redacted]'
  );
});

test("ProcessLogEntry shares the trace-capable buffer contract", () => {
  const entry: ProcessLogEntry = {
    sequence: 1,
    timestamp: 1,
    source: "main",
    level: "trace",
    message: "trace line",
    metadata: { retained: true }
  };
  const line: ProcessLogLine = entry;
  assert.equal(line.level, "trace");
  assert.deepEqual(line.metadata, { retained: true });
});

test("sanitizes descriptors without executing getters and survives descriptor traps", () => {
  let getterCalls = 0;
  const getter = {} as Record<string, unknown>;
  Object.defineProperty(getter, "throws", {
    enumerable: true,
    get: () => {
      getterCalls += 1;
      return "must-not-run";
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
  assert.equal(getterCalls, 0);
  assert.equal((line?.metadata?.getter as Record<string, unknown>)?.throws, "[accessor]");
  assert.equal(line?.metadata?.unreadableProxy, "[unreadable]");
  assert.equal(line?.metadata?.invalidDate, "[invalid date]");
  assert.equal((line?.metadata?.cloneFailure as Record<string, unknown>)?.callback, "[unserializable]");
});

test("coalesces a 100k burst into one bounded sequence batch", async () => {
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined } });
  const batches: ProcessLogBatch[] = [];
  buffer.subscribe((batch) => {
    batches.push(batch);
  }, { afterSequence: 0 });

  const startedAt = performance.now();
  for (let index = 1; index <= 100_000; index += 1) {
    logger.info("core", `line-${index}`);
  }
  const appendDurationMs = performance.now() - startedAt;

  assert.equal(batches.length, 0);
  assert.equal(buffer.size, 5_000);
  assert.ok(buffer.bytes <= DEFAULT_PROCESS_LOG_MAX_BYTES);
  assert.ok(appendDurationMs < 5_000, `100k saturated append took ${appendDurationMs.toFixed(1)}ms`);
  await logger.flush();
  assert.equal(batches.length, 1);
  assert.equal(batches[0]?.lines.length, 5_000);
  assert.equal(batches[0]?.lines[0]?.sequence, 95_001);
  assert.equal(batches[0]?.afterSequence, 100_000);
  assert.equal(batches[0]?.droppedBeforeSequence, 95_001);
});

test("snapshotAndSubscribe uses the snapshot newest sequence as an atomic cursor", async () => {
  const buffer = new ProcessLogBuffer();
  buffer.append("main", "info", "before-one");
  buffer.append("main", "info", "before-two");
  const batches: ProcessLogBatch[] = [];

  const subscription = buffer.snapshotAndSubscribe((batch) => {
    batches.push(batch);
  });
  buffer.append("main", "info", "after-one");
  await buffer.flush();

  assert.deepEqual(subscription.snapshot.map((line) => line.message), ["before-one", "before-two"]);
  assert.equal(subscription.afterSequence, 2);
  assert.deepEqual(batches.flatMap((batch) => batch.lines.map((line) => line.message)), ["after-one"]);
  assert.equal(batches[0]?.droppedBeforeSequence, null);
  subscription.unsubscribe();
});

test("slow subscribers receive a bounded gap after the ring evicts their cursor", async () => {
  const buffer = new ProcessLogBuffer({ maxEntries: 3, maxBytes: 1024 });
  const batches: ProcessLogBatch[] = [];
  let releaseFirst!: () => void;
  const firstDelivery = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  buffer.subscribe(async (batch) => {
    batches.push(batch);
    if (batches.length === 1) await firstDelivery;
  }, { afterSequence: 0 });

  buffer.append("main", "info", "one");
  await buffer.flush();
  for (let index = 2; index <= 10; index += 1) buffer.append("main", "info", String(index));
  await buffer.flush();
  assert.equal(batches.length, 1);

  releaseFirst();
  await new Promise<void>((resolve) => setImmediate(resolve));
  await buffer.flush();
  assert.equal(batches.length, 2);
  assert.deepEqual(batches[1]?.lines.map((line) => line.message), ["8", "9", "10"]);
  assert.equal(batches[1]?.droppedBeforeSequence, 8);
  assert.equal(batches[1]?.afterSequence, 10);
});

test("delivers independent deep-cloned batches to each subscriber", async () => {
  const buffer = new ProcessLogBuffer();
  let secondBatch: ProcessLogBatch | undefined;
  buffer.subscribe((batch) => {
    const mutableLine = batch.lines[0] as ProcessLogLine;
    (mutableLine.metadata?.nested as Record<string, unknown>).value = "mutated";
  });
  buffer.subscribe((batch) => {
    secondBatch = batch;
  });

  buffer.append("main", "info", "one", { nested: { value: "original" } });
  await buffer.flush();

  assert.equal(
    (secondBatch?.lines[0]?.metadata?.nested as Record<string, unknown>).value,
    "original"
  );
  assert.equal(
    (buffer.snapshot({})[0]?.metadata?.nested as Record<string, unknown>).value,
    "original"
  );
});

test("clear fences queued subscriber events while allowing later lines", async () => {
  const buffer = new ProcessLogBuffer();
  const received: string[] = [];
  buffer.subscribe((batch) => {
    received.push(...batch.lines.map((line) => line.message));
  });

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

test("logger binds original console methods and drops reentrant console forwarding", async () => {
  const output: unknown[][] = [];
  let logger: DesktopLogger;
  const sink: { info: (...args: unknown[]) => void } = {
    info: (...args) => {
      output.push(args);
      logger?.info("main", "reentrant-original");
    }
  };
  const buffer = new ProcessLogBuffer();
  logger = new DesktopLogger({ buffer, console: sink });
  sink.info = (...args) => logger.info("main", "patched", ...args);

  logger.info("main", "outer");
  await logger.flush();

  assert.deepEqual(output, [["outer"]]);
  assert.deepEqual(buffer.snapshot({}).map((line) => line.message), ["outer"]);
});

test("structured logging never invokes a getter that would reenter the logger", async () => {
  const buffer = new ProcessLogBuffer();
  const logger = new DesktopLogger({ buffer, console: { info: () => undefined } });
  let getterCalls = 0;
  const payload = {} as Record<string, unknown>;
  Object.defineProperty(payload, "recursive", {
    enumerable: true,
    get: () => {
      getterCalls += 1;
      logger.info("main", "recursive");
      return "secret";
    }
  });

  logger.info("main", payload);
  await logger.flush();

  assert.equal(getterCalls, 0);
  assert.equal(buffer.snapshot({}).length, 1);
  assert.equal(buffer.snapshot({})[0]?.message.includes("[accessor]"), true);
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

test("Core stop escalates from ignored SIGTERM to SIGKILL and waits for close", async () => {
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  const signals: Array<NodeJS.Signals | number | undefined> = [];
  const child = Object.assign(new EventEmitter(), {
    stdout,
    stderr,
    kill: (signal?: NodeJS.Signals | number) => {
      signals.push(signal);
      if (signal === "SIGKILL") {
        queueMicrotask(() => {
          stdout.end();
          stderr.end();
          child.emit("exit", null, "SIGKILL");
          child.emit("close", null, "SIGKILL");
        });
      }
      return true;
    }
  });
  const logs: Array<{ level: string; message: string }> = [];
  const lifecycle = attachCoreProcessLifecycle(
    child as unknown as ChildProcess,
    (_source, level, message) => logs.push({ level, message }),
    { platform: "linux", gracefulTimeoutMs: 5, forceTimeoutMs: 50 }
  );
  stdout.write("mock partial");

  await lifecycle.stop();

  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
  assert.equal(logs.some((line) => line.level === "error"), false);
  assert.equal(logs.some((line) => line.message.includes("mock partial")), true);
  assert.equal(logs.some((line) => line.message.includes("stopped intentionally")), true);
});

test("Core stop timeout rejects and clears stopPromise for a later retry", async () => {
  const child = Object.assign(new EventEmitter(), {
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    closeOnSignal: false,
    kill: (_signal?: NodeJS.Signals | number) => {
      if (child.closeOnSignal) {
        queueMicrotask(() => {
          child.emit("exit", null, "SIGTERM");
          child.emit("close", null, "SIGTERM");
        });
      }
      return true;
    }
  });
  const lifecycle = attachCoreProcessLifecycle(
    child as unknown as ChildProcess,
    () => undefined,
    { platform: "linux", gracefulTimeoutMs: 5, forceTimeoutMs: 5 }
  );

  await assert.rejects(lifecycle.stop(), /timed out/i);
  child.closeOnSignal = true;
  await lifecycle.stop();
  await lifecycle.waitForExit;
});

test("Windows Core stop requests taskkill and resolves only after child close", async () => {
  const events: string[] = [];
  const child = Object.assign(new EventEmitter(), {
    pid: 4242,
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    kill: () => {
      events.push("fallback-kill");
      return true;
    }
  });
  const lifecycle = attachCoreProcessLifecycle(
    child as unknown as ChildProcess,
    () => undefined,
    {
      platform: "win32",
      forceTimeoutMs: 50,
      requestWindowsStop: (target) => {
        events.push(`taskkill:${target.pid}`);
        queueMicrotask(() => {
          events.push("close");
          target.emit("exit", 1, null);
          target.emit("close", 1, null);
        });
      }
    }
  );

  await lifecycle.stop();

  assert.deepEqual(events, ["taskkill:4242", "close"]);
});

test("intentional real running Core stop is informational on Windows and POSIX", async () => {
  const logs: Array<{ level: string; message: string }> = [];
  const child = spawn(
    process.execPath,
    ["-e", "process.stdout.write('real partial'); setInterval(() => {}, 1000);"],
    { stdio: ["ignore", "pipe", "pipe"] }
  );
  const lifecycle = attachCoreProcessLifecycle(child, (_source, level, message) => {
    logs.push({ level, message });
  });
  await once(child.stdout!, "data");

  await lifecycle.stop();

  assert.equal(logs.some((line) => line.level === "error"), false);
  assert.equal(logs.some((line) => line.message.includes("real partial")), true);
  assert.equal(logs.some((line) => line.message.includes("stopped intentionally")), true);
});

test("natural non-zero Core exit remains an error before a later stop call", async () => {
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

  assert.deepEqual(logs.filter((line) => line.level === "info").map((line) => line.message), ["中😀"]);
  assert.deepEqual(logs.filter((line) => line.level === "warn").map((line) => line.message), ["警告"]);
  const lastLog = logs[logs.length - 1];
  assert.equal(lastLog?.level, "error");
  assert.equal(lastLog?.message.includes("code=3"), true);
});

test("runtime shutdown hook drains Core before closing the logger", async () => {
  const coreProcess = await import("../src/core-process.ts") as typeof import("../src/core-process.ts") & {
    stopCoreProcessesAndCloseLogger?: (
      handles: readonly unknown[],
      closeLogger: () => Promise<void>
    ) => Promise<void>;
  };
  assert.equal(typeof coreProcess.stopCoreProcessesAndCloseLogger, "function");
  const events: string[] = [];
  const handle = {
    process: null,
    waitForExit: Promise.resolve(),
    stop: async () => {
      await new Promise<void>((resolve) => setImmediate(resolve));
      events.push("core-stop-complete");
    }
  };

  await coreProcess.stopCoreProcessesAndCloseLogger?.([handle], async () => {
    events.push("logger-close");
  });
  assert.deepEqual(events, ["core-stop-complete", "logger-close"]);
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

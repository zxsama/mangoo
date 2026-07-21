import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath, pathToFileURL } from "node:url";
import { performance } from "node:perf_hooks";

const SOURCE_MODE_ARGUMENT = "--notification-center-source-mode";
const INJECTED_FAILURE_ARGUMENT = "--notification-center-injected-failure";
const CORE_PROBE_PREFIX = "MEGLE_NOTIFICATION_BENCH_JSON:";
const BENCHMARK_DIRECTORY = path.dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = path.resolve(BENCHMARK_DIRECTORY, "../..");
const BENCHMARK_SCRATCH_ROOT = path.join(REPOSITORY_ROOT, ".tmp", "notification-center-bench");
const LATEST_EVIDENCE_PATH = path.join(BENCHMARK_SCRATCH_ROOT, "latest.json");
const LATEST_EVIDENCE_TEMP_PATH = path.join(BENCHMARK_SCRATCH_ROOT, "latest.json.tmp");
const STORE_MODULE_URL = pathToFileURL(
  path.join(REPOSITORY_ROOT, "apps", "desktop", "src", "notification-store.ts")
);
const WORKER_URL = pathToFileURL(
  path.join(REPOSITORY_ROOT, "apps", "desktop", "src", "notification-store-worker.ts")
);
const REPORTING_MODULE_URL = pathToFileURL(
  path.join(REPOSITORY_ROOT, "apps", "web", "src", "core", "notificationReporting.ts")
);

const THUMBNAIL_COMPLETIONS = 10_000;
const THUMBNAIL_COMPLETIONS_PER_LIBRARY = 2_500;
const THUMBNAIL_BLOB_BYTES_PER_LIBRARY = 10_000;
const RAPID_PROGRESS_UPDATES = 10_000;
const EXPECTED_PROGRESS_EVENTS = 10;
const COMPLETED_OPERATIONS = 2_100;
const SEEDED_ACTIVE_ROWS = 20;
const FIXED_NOW = Date.UTC(2026, 6, 20, 12, 0, 0);
const LIBRARIES = Array.from({ length: 4 }, (_, index) => ({
  id: `bench-library-${index + 1}`,
  name: `Benchmark Library ${index + 1}`
}));

function clearPublishedEvidence() {
  fs.rmSync(LATEST_EVIDENCE_PATH, { force: true });
  fs.rmSync(LATEST_EVIDENCE_TEMP_PATH, { force: true });
}

clearPublishedEvidence();

if (!process.argv.includes(SOURCE_MODE_ARGUMENT)) {
  const child = spawnSync(
    process.execPath,
    [
      "--no-warnings",
      "--experimental-strip-types",
      fileURLToPath(import.meta.url),
      SOURCE_MODE_ARGUMENT
    ],
    {
      cwd: REPOSITORY_ROOT,
      env: process.env,
      stdio: "inherit"
    }
  );
  if (child.error) throw child.error;
  process.exit(child.status ?? 1);
}

if (process.argv.includes(INJECTED_FAILURE_ARGUMENT)) {
  throw new Error("injected benchmark failure before evidence publication");
}

function elapsedSince(startedAt) {
  return Number((performance.now() - startedAt).toFixed(3));
}

function completedOperation(index) {
  const completedAt = FIXED_NOW - (COMPLETED_OPERATIONS - index) * 1_000;
  return {
    operationId: `bench:completed:${index}`,
    revision: 1,
    kind: index % 3 === 0 ? "move" : index % 3 === 1 ? "rename" : "delete",
    category: "file_operation",
    origin: "user",
    presentation: "history_only",
    status: "succeeded",
    phase: "completed",
    title: `Benchmark completed operation ${index}`,
    summary: "Completed by deterministic benchmark seed",
    taskId: null,
    rootId: null,
    folderId: null,
    fileId: index + 1,
    currentValue: 1,
    totalValue: 1,
    unit: "items",
    errorCode: null,
    errorDetail: null,
    cancellable: false,
    retryable: false,
    attempt: 1,
    occurrenceCount: 1,
    startedAt: completedAt - 250,
    updatedAt: completedAt,
    completedAt
  };
}

function scalar(database, sql, ...parameters) {
  const row = database.prepare(sql).get(...parameters);
  return Number(row?.value ?? 0);
}

function assertInsideRepository(targetPath) {
  const relative = path.relative(REPOSITORY_ROOT, targetPath);
  assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative));
}

function removeRunDirectory(runDirectory) {
  assertInsideRepository(runDirectory);
  assert.equal(path.dirname(runDirectory), BENCHMARK_SCRATCH_ROOT);
  fs.rmSync(runDirectory, { recursive: true, force: true });
}

function assertRunDirectoryRemoved(runDirectory, label) {
  assert.ok(runDirectory, `${label} must create a run directory`);
  assert.equal(fs.existsSync(runDirectory), false, `${label} run directory must be removed`);
}

function notificationStoreWorker(store) {
  return store?.worker;
}

function assertStoreWorkerStopped(store, label) {
  const worker = notificationStoreWorker(store);
  assert.ok(worker, `${label} must expose its production Worker instance`);
  assert.equal(worker.threadId, -1, `${label} Worker thread must be terminated`);
}

function processExists(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error?.code === "ESRCH") return false;
    throw error;
  }
}

function assertChildExited(result, label) {
  assert.ok(Number.isSafeInteger(result.pid) && result.pid > 0, `${label} must expose a child pid`);
  assert.equal(processExists(result.pid), false, `${label} child process must not remain running`);
}

async function withNotificationStoreRun({
  prefix,
  storeModuleUrl = STORE_MODULE_URL,
  databasePathForRun = (runDirectory) => path.join(runDirectory, "notifications.sqlite"),
  storeFactory,
  onRunDirectory,
  onStoreCreated,
  execute
}) {
  let runDirectory;
  let store;
  try {
    fs.mkdirSync(BENCHMARK_SCRATCH_ROOT, { recursive: true });
    runDirectory = fs.mkdtempSync(path.join(BENCHMARK_SCRATCH_ROOT, prefix));
    assertInsideRepository(runDirectory);
    onRunDirectory?.(runDirectory);
    const databasePath = databasePathForRun(runDirectory);
    const startupStartedAt = performance.now();
    const storeModule = await import(storeModuleUrl.href);
    const options = { databasePath, workerUrl: WORKER_URL, startupTimeoutMs: 5_000 };
    if (storeFactory) {
      store = await storeFactory(storeModule, options, (candidate) => {
        store = candidate;
      });
    } else {
      store = await storeModule.createNotificationStore(options);
    }
    onStoreCreated?.(store);
    const startupElapsedMs = elapsedSince(startupStartedAt);
    const closeStore = async () => {
      if (!store) return;
      await store.close();
      store = undefined;
    };
    return await execute({
      store,
      closeStore,
      runDirectory,
      databasePath,
      startupElapsedMs
    });
  } finally {
    let cleanupError;
    if (store) {
      try {
        await store.close();
      } catch (error) {
        cleanupError = error;
      }
    }
    if (runDirectory) {
      try {
        removeRunDirectory(runDirectory);
      } catch (error) {
        cleanupError ??= error;
      }
    }
    if (cleanupError) throw cleanupError;
  }
}

function spawnCargo(args) {
  const result = spawnSync("cargo", args, {
    cwd: REPOSITORY_ROOT,
    env: process.env,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    windowsHide: true
  });
  if (result.error) throw result.error;
  assertChildExited(result, `cargo ${args.join(" ")}`);
  return result;
}

function notificationProbeTempDirectories() {
  return fs
    .readdirSync(os.tmpdir(), { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.startsWith("megle-notification-benchmark-"))
    .map((entry) => entry.name)
    .sort();
}

function runCoreProbe() {
  const tempDirectoriesBefore = notificationProbeTempDirectories();
  const startedAt = performance.now();
  const result = spawnCargo([
    "test",
    "-p",
    "megle-core",
    "db::tests::notification_center_benchmark_probe",
    "--",
    "--ignored",
    "--exact",
    "--nocapture",
    "--test-threads=1"
  ]);
  const processElapsedMs = elapsedSince(startedAt);
  if (result.status !== 0) {
    throw new Error(
      `production Core outbox/thumbnail probe failed (exit ${result.status})\n${result.stdout}\n${result.stderr}`
    );
  }
  assert.deepEqual(
    notificationProbeTempDirectories(),
    tempDirectoriesBefore,
    "production Core probe must remove its internal temp directory and SQLite sidecars"
  );
  const payloadLines = result.stdout
    .split(/\r?\n/)
    .filter((line) => line.includes(CORE_PROBE_PREFIX));
  assert.equal(payloadLines.length, 1, "production Core probe must emit exactly one JSON payload");
  const payload = payloadLines[0].slice(
    payloadLines[0].indexOf(CORE_PROBE_PREFIX) + CORE_PROBE_PREFIX.length
  );
  const probe = JSON.parse(payload);

  assert.equal(probe.metrics.rapidProgressUpdates, RAPID_PROGRESS_UPDATES);
  assert.equal(probe.metrics.logicalProgressRevisionCount, RAPID_PROGRESS_UPDATES);
  assert.equal(probe.metrics.expectedProgressEvents, EXPECTED_PROGRESS_EVENTS);
  assert.equal(probe.metrics.persistedProgressEvents, EXPECTED_PROGRESS_EVENTS);
  assert.equal(probe.metrics.activeOperationSeconds, EXPECTED_PROGRESS_EVENTS);
  assert.equal(probe.metrics.terminalEventCount, 1);
  assert.deepEqual(
    probe.metrics.sampledProgressOffsetsMs,
    Array.from({ length: EXPECTED_PROGRESS_EVENTS }, (_, index) => (index + 1) * 1_000)
  );
  assert.equal(probe.metrics.outboxRowCount, 53);
  assert.equal(probe.events.length, 53);
  assert.equal(probe.latestSequence, 53);
  assert.equal(probe.activeOperationIds.length, SEEDED_ACTIVE_ROWS);
  assert.equal(probe.metrics.thumbnailCompletionCount, THUMBNAIL_COMPLETIONS);
  assert.equal(probe.thumbnailLibraries.length, LIBRARIES.length);

  for (let index = 0; index < LIBRARIES.length; index += 1) {
    const library = probe.thumbnailLibraries[index];
    assert.equal(library.libraryId, LIBRARIES[index].id);
    assert.equal(library.libraryName, LIBRARIES[index].name);
    assert.equal(library.completionCount, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
    assert.equal(library.completedTaskRows, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
    assert.equal(library.readyThumbnailRows, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
    assert.equal(library.blobRows, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
    assert.equal(library.perItemOutboxRows, 0);
    assert.deepEqual(library.stats, {
      cachedCount: THUMBNAIL_COMPLETIONS_PER_LIBRARY,
      missingCount: 0,
      staleCount: 0,
      failedCount: 0,
      pendingCandidateCount: 0,
      totalBlobBytes: THUMBNAIL_BLOB_BYTES_PER_LIBRARY,
      activeBulkTaskCount: 0
    });
  }

  return {
    probe,
    processElapsedMs,
    childPid: result.pid,
    tempDirectoriesRemoved: true
  };
}

function runPublishedEvidenceFailureSelfTest() {
  fs.mkdirSync(BENCHMARK_SCRATCH_ROOT, { recursive: true });
  fs.writeFileSync(LATEST_EVIDENCE_PATH, '{"stale":true}\n', "utf8");
  let result;
  try {
    result = spawnSync(
      process.execPath,
      [
        "--no-warnings",
        "--experimental-strip-types",
        fileURLToPath(import.meta.url),
        SOURCE_MODE_ARGUMENT,
        INJECTED_FAILURE_ARGUMENT
      ],
      {
        cwd: REPOSITORY_ROOT,
        env: process.env,
        encoding: "utf8",
        maxBuffer: 4 * 1024 * 1024,
        windowsHide: true
      }
    );
    if (result.error) throw result.error;
    assertChildExited(result, "injected benchmark failure");
    assert.notEqual(result.status, 0);
    assert.equal(fs.existsSync(LATEST_EVIDENCE_PATH), false);
    assert.equal(fs.existsSync(LATEST_EVIDENCE_TEMP_PATH), false);
    return {
      staleSuccessRemoved: true,
      failedRunPublishedNoEvidence: true,
      childStopped: true,
      exitCode: result.status
    };
  } finally {
    clearPublishedEvidence();
  }
}

async function runCleanupSelfTest() {
  const suffix = `${process.pid}-${Date.now()}-`;

  let importRunDirectory;
  let importFailure;
  try {
    await withNotificationStoreRun({
      prefix: `cleanup-import-${suffix}`,
      storeModuleUrl: pathToFileURL(path.join(REPOSITORY_ROOT, "missing-notification-store.ts")),
      onRunDirectory(runDirectory) {
        importRunDirectory = runDirectory;
      },
      async execute() {
        assert.fail("missing module import should not execute the benchmark callback");
      }
    });
  } catch (error) {
    importFailure = error;
  }
  assert.ok(importFailure, "fault-injected dynamic import must fail");
  assertRunDirectoryRemoved(importRunDirectory, "dynamic import failure");

  let startupRunDirectory;
  let startupStore;
  let startupFailure;
  try {
    await withNotificationStoreRun({
      prefix: `cleanup-startup-${suffix}`,
      databasePathForRun: (runDirectory) => runDirectory,
      onRunDirectory(runDirectory) {
        startupRunDirectory = runDirectory;
      },
      async storeFactory(storeModule, options, capture) {
        const candidate = new storeModule.NotificationStore(options);
        startupStore = candidate;
        capture(candidate);
        await candidate.ready();
        return candidate;
      },
      async execute() {
        assert.fail("invalid SQLite startup path must not execute the benchmark callback");
      }
    });
  } catch (error) {
    startupFailure = error;
  }
  assert.ok(startupFailure, "fault-injected store startup must fail");
  assertRunDirectoryRemoved(startupRunDirectory, "store startup failure");
  assertStoreWorkerStopped(startupStore, "store startup failure");

  let storeRunDirectory;
  let postCreateStore;
  let sidecarsObservedBeforeFailure = [];
  let storeFailure;
  try {
    await withNotificationStoreRun({
      prefix: `cleanup-store-${suffix}`,
      onRunDirectory(runDirectory) {
        storeRunDirectory = runDirectory;
      },
      onStoreCreated(store) {
        postCreateStore = store;
      },
      async execute({ store, databasePath }) {
        await store.publishThumbnailSummary({
          libraryId: "cleanup-self-test",
          libraryName: "Cleanup self-test",
          observedAt: FIXED_NOW,
          cachedCount: 1,
          totalCandidateCount: 1,
          missingCount: 0,
          staleCount: 0,
          failedCount: 0,
          pendingCount: 0,
          activeBulkTaskCount: 0,
          totalCacheBytes: 1,
          completionPercent: 100
        });
        sidecarsObservedBeforeFailure = [databasePath, `${databasePath}-wal`, `${databasePath}-shm`]
          .filter((candidate) => fs.existsSync(candidate))
          .map((candidate) => path.basename(candidate));
        throw new Error("injected failure after notification store creation");
      }
    });
  } catch (error) {
    storeFailure = error;
  }
  assert.match(String(storeFailure), /injected failure after notification store creation/);
  assert.deepEqual(sidecarsObservedBeforeFailure.sort(), [
    "notifications.sqlite",
    "notifications.sqlite-shm",
    "notifications.sqlite-wal"
  ]);
  assertRunDirectoryRemoved(storeRunDirectory, "post-create failure");
  assertStoreWorkerStopped(postCreateStore, "post-create failure");

  let cargoRunDirectory;
  let cargoStore;
  let cargoFailure;
  let cargoFailureResult;
  try {
    await withNotificationStoreRun({
      prefix: `cleanup-cargo-${suffix}`,
      onRunDirectory(runDirectory) {
        cargoRunDirectory = runDirectory;
      },
      onStoreCreated(store) {
        cargoStore = store;
      },
      async execute() {
        cargoFailureResult = spawnCargo(["test", "-p", "megle-core-missing-package"]);
        assert.notEqual(cargoFailureResult.status, 0, "fault-injected Cargo child must fail");
        throw new Error(`injected Cargo child nonzero exit ${cargoFailureResult.status}`);
      }
    });
  } catch (error) {
    cargoFailure = error;
  }
  assert.match(String(cargoFailure), /injected Cargo child nonzero exit/);
  assert.ok(cargoFailureResult && cargoFailureResult.status !== 0);
  assert.equal(processExists(cargoFailureResult.pid), false);
  assertRunDirectoryRemoved(cargoRunDirectory, "Cargo child failure");
  assertStoreWorkerStopped(cargoStore, "Cargo child failure");

  return {
    importFailureRunRemoved: true,
    startupFailureRunRemoved: true,
    postCreateFailureRunRemoved: true,
    cargoChildFailureRunRemoved: true,
    startupFailureWorkerStopped: true,
    postCreateFailureWorkerStopped: true,
    cargoFailureWorkerStopped: true,
    cargoFailureChildStopped: true,
    cargoFailureExitCode: cargoFailureResult.status,
    sidecarsObservedBeforeFailure
  };
}

function installReportingBridge(store, reportedSummaries) {
  const previousDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: {
      megleDesktop: {
        windowRole: "main",
        notifications: {
          async publishThumbnailSummary(summary) {
            reportedSummaries.push(summary);
            await store.publishThumbnailSummary(summary);
            return true;
          }
        }
      }
    }
  });
  return () => {
    if (previousDescriptor) {
      Object.defineProperty(globalThis, "window", previousDescriptor);
    } else {
      delete globalThis.window;
    }
  };
}

function expectedThumbnailSummary(library, observedAt) {
  return {
    libraryId: library.id,
    libraryName: library.name,
    observedAt,
    cachedCount: THUMBNAIL_COMPLETIONS_PER_LIBRARY,
    totalCandidateCount: THUMBNAIL_COMPLETIONS_PER_LIBRARY,
    missingCount: 0,
    staleCount: 0,
    failedCount: 0,
    pendingCount: 0,
    activeBulkTaskCount: 0,
    totalCacheBytes: THUMBNAIL_BLOB_BYTES_PER_LIBRARY,
    completionPercent: 100
  };
}

async function publishCoreThumbnailStats(store, thumbnailLibraries) {
  const reportedSummaries = [];
  const restoreBridge = installReportingBridge(store, reportedSummaries);
  const { publishThumbnailCacheStats } = await import(REPORTING_MODULE_URL.href);
  const startedAt = performance.now();
  try {
    for (let index = 0; index < thumbnailLibraries.length; index += 1) {
      const library = thumbnailLibraries[index];
      const accepted = await publishThumbnailCacheStats(
        library.stats,
        { libraryId: library.libraryId, libraryName: library.libraryName },
        FIXED_NOW + index
      );
      assert.equal(accepted, true);
    }
  } finally {
    restoreBridge();
  }
  assert.equal(reportedSummaries.length, LIBRARIES.length);
  assert.deepEqual(
    reportedSummaries,
    LIBRARIES.map((library, index) => expectedThumbnailSummary(library, FIXED_NOW + index))
  );
  return {
    reportedSummaries,
    elapsedMs: elapsedSince(startedAt)
  };
}

async function runBenchmark() {
  const publishedEvidenceFailureSelfTest = runPublishedEvidenceFailureSelfTest();
  const cleanupSelfTest = await runCleanupSelfTest();
  const totalStartedAt = performance.now();
  return withNotificationStoreRun({
    prefix: "run-",
    async execute({ store, closeStore, databasePath, startupElapsedMs }) {
      const coreProbe = runCoreProbe();

      const progressStartedAt = performance.now();
      await store.applyOutboxBatch(
        LIBRARIES[0],
        coreProbe.probe.events,
        coreProbe.probe.latestSequence
      );
      const progressElapsedMs = elapsedSince(progressStartedAt);

      const thumbnailReporting = await publishCoreThumbnailStats(
        store,
        coreProbe.probe.thumbnailLibraries
      );

      const syntheticCompletedCount =
        COMPLETED_OPERATIONS - coreProbe.probe.metrics.terminalEventCount;
      assert.equal(syntheticCompletedCount, 2_099);
      const completedByLibrary = LIBRARIES.map(() => []);
      for (let index = 0; index < syntheticCompletedCount; index += 1) {
        completedByLibrary[index % LIBRARIES.length].push(completedOperation(index));
      }
      const terminalStartedAt = performance.now();
      for (let libraryIndex = 0; libraryIndex < LIBRARIES.length; libraryIndex += 1) {
        const library = LIBRARIES[libraryIndex];
        const sequenceOffset = libraryIndex === 0 ? coreProbe.probe.latestSequence : 0;
        const events = completedByLibrary[libraryIndex].map((operation, index) => ({
          sequence: sequenceOffset + index + 1,
          operation
        }));
        await store.applyOutboxBatch(library, events, sequenceOffset + events.length);
      }
      const terminalElapsedMs = elapsedSince(terminalStartedAt);

      const retentionStartedAt = performance.now();
      const retention = await store.cleanupRetention(FIXED_NOW);
      const retentionElapsedMs = elapsedSince(retentionStartedAt);
      assert.deepEqual(retention, { deletedExpired: 0, deletedOverflow: 100 });

      const compactStartedAt = performance.now();
      const compact = await store.compactSnapshot();
      const compactElapsedMs = elapsedSince(compactStartedAt);
      assert.equal(compact.active.length, SEEDED_ACTIVE_ROWS);
      assert.deepEqual(
        compact.active.map((operation) => operation.operationId).sort(),
        [...coreProbe.probe.activeOperationIds].sort()
      );
      assert.equal(compact.thumbnailSummaries.length, LIBRARIES.length);
      assert.deepEqual(
        compact.thumbnailSummaries,
        LIBRARIES.map((library, index) => expectedThumbnailSummary(library, FIXED_NOW + index))
      );

      const closedStore = store;
      const closeStartedAt = performance.now();
      await closeStore();
      const closeElapsedMs = elapsedSince(closeStartedAt);
      assertStoreWorkerStopped(closedStore, "successful benchmark");
      const benchmarkElapsedMs = elapsedSince(totalStartedAt);
      const storeWorkerElapsedMs = Number(
        (
          startupElapsedMs +
          progressElapsedMs +
          thumbnailReporting.elapsedMs +
          terminalElapsedMs +
          retentionElapsedMs +
          compactElapsedMs +
          closeElapsedMs
        ).toFixed(3)
      );

      const database = new DatabaseSync(databasePath, { readOnly: true });
      let rows;
      let persistedThumbnailSummaries;
      try {
        persistedThumbnailSummaries = database
          .prepare(
            "SELECT payload_json FROM thumbnail_summaries ORDER BY library_id ASC"
          )
          .all()
          .map((row) => JSON.parse(row.payload_json));
        rows = {
          operationRows: scalar(database, "SELECT COUNT(*) AS value FROM operations"),
          perThumbnailNotificationRows: scalar(
            database,
            `SELECT COUNT(*) AS value FROM operations
             WHERE category = 'thumbnail_cache'
                OR operation_id LIKE 'thumbnail:%'
                OR json_extract(payload_json, '$.kind') LIKE 'thumbnail%'`
          ),
          thumbnailAggregateRows: scalar(
            database,
            "SELECT COUNT(*) AS value FROM thumbnail_summaries"
          ),
          persistedProgressEvents: coreProbe.probe.metrics.persistedProgressEvents,
          completedHistoryRows: scalar(
            database,
            "SELECT COUNT(*) AS value FROM operations WHERE status NOT IN ('pending', 'running')"
          ),
          activeRowsRetained: scalar(
            database,
            "SELECT COUNT(*) AS value FROM operations WHERE status IN ('pending', 'running')"
          )
        };
      } finally {
        database.close();
      }

      const expectedSummaries = LIBRARIES.map((library, index) =>
        expectedThumbnailSummary(library, FIXED_NOW + index)
      );
      assert.equal(rows.operationRows, 2_020);
      assert.equal(rows.perThumbnailNotificationRows, 0);
      assert.equal(rows.thumbnailAggregateRows, LIBRARIES.length);
      assert.deepEqual(persistedThumbnailSummaries, expectedSummaries);
      assert.equal(rows.persistedProgressEvents, EXPECTED_PROGRESS_EVENTS);
      assert.equal(coreProbe.probe.metrics.terminalEventCount, 1);
      assert.equal(rows.completedHistoryRows, 2_000);
      assert.equal(retention.deletedOverflow, 100);
      assert.equal(rows.activeRowsRetained, SEEDED_ACTIVE_ROWS);

      return {
        benchmark: "unified-notification-center",
        boundary:
          "Core task/thumbnail production DB paths -> Web notification reporting -> NotificationStore -> worker_threads -> Desktop SQLite WAL",
        seed: {
          libraryCount: LIBRARIES.length,
          thumbnailCompletions: THUMBNAIL_COMPLETIONS,
          thumbnailCompletionsPerLibrary: THUMBNAIL_COMPLETIONS_PER_LIBRARY,
          rapidProgressUpdates: RAPID_PROGRESS_UPDATES,
          expectedProgressEvents: EXPECTED_PROGRESS_EVENTS,
          completedOperations: COMPLETED_OPERATIONS,
          syntheticCompletedOperations: syntheticCompletedCount,
          activeOperations: SEEDED_ACTIVE_ROWS,
          activeOperationSeconds: coreProbe.probe.metrics.activeOperationSeconds,
          terminalEventCount: coreProbe.probe.metrics.terminalEventCount
        },
        coreProbe: {
          childPid: coreProbe.childPid,
          childStopped: true,
          tempDirectoriesRemoved: coreProbe.tempDirectoriesRemoved,
          processElapsedMs: coreProbe.processElapsedMs,
          progress: {
            inputUpdateCount: RAPID_PROGRESS_UPDATES,
            logicalRevisionCount: coreProbe.probe.metrics.logicalProgressRevisionCount,
            persistedEventCount: coreProbe.probe.metrics.persistedProgressEvents,
            sampledOffsetsMs: coreProbe.probe.metrics.sampledProgressOffsetsMs,
            terminalEventCount: coreProbe.probe.metrics.terminalEventCount,
            totalOutboxEventCount: coreProbe.probe.metrics.outboxRowCount,
            inputElapsedMs: Number(coreProbe.probe.metrics.inputElapsedMs.toFixed(3))
          },
          thumbnails: {
            completionCount: coreProbe.probe.metrics.thumbnailCompletionCount,
            elapsedMs: Number(coreProbe.probe.metrics.thumbnailElapsedMs.toFixed(3)),
            libraries: coreProbe.probe.thumbnailLibraries.map((library) => ({
              ...library,
              completionElapsedMs: Number(library.completionElapsedMs.toFixed(3))
            }))
          }
        },
        reportingStore: {
          reportingCallCount: thumbnailReporting.reportedSummaries.length,
          persistedAggregateCount: persistedThumbnailSummaries.length,
          persistedSummaries: persistedThumbnailSummaries,
          elapsedMs: thumbnailReporting.elapsedMs
        },
        storeWorker: {
          notificationEventCount: coreProbe.probe.events.length + syntheticCompletedCount,
          coreOutboxEventCount: coreProbe.probe.events.length,
          thumbnailSummaryUpdateCount: LIBRARIES.length,
          writeRequestCount: 1 + LIBRARIES.length + LIBRARIES.length + 1,
          readRequestCount: 1,
          closeRequestCount: 1,
          totalRequestCount: 1 + LIBRARIES.length + LIBRARIES.length + 3,
          startupElapsedMs,
          progressElapsedMs,
          thumbnailReportingElapsedMs: thumbnailReporting.elapsedMs,
          terminalElapsedMs,
          retentionElapsedMs,
          compactElapsedMs,
          closeElapsedMs,
          elapsedMs: storeWorkerElapsedMs,
          workerStopped: true
        },
        cleanupSelfTest,
        publishedEvidenceFailureSelfTest,
        benchmarkElapsedMs,
        retention,
        rows,
        assertions: {
          coreThumbnailCompletionsExact: true,
          coreThumbnailPerItemOutboxRowsZero: true,
          reportingCallsEqualLibraryCount: true,
          aggregateRowsAndCountsExact: true,
          progressSamplingExact: true,
          terminalEventPresent: true,
          completedHistoryExact: true,
          retentionOverflowExact: true,
          allActiveRowsRetained: true,
          allFailurePathsCleaned: true,
          workerAndChildProcessesStopped: true
        }
      };
    }
  });
}

function publishLatestEvidence(metrics) {
  fs.mkdirSync(BENCHMARK_SCRATCH_ROOT, { recursive: true });
  try {
    fs.writeFileSync(LATEST_EVIDENCE_TEMP_PATH, `${JSON.stringify(metrics, null, 2)}\n`, "utf8");
    fs.renameSync(LATEST_EVIDENCE_TEMP_PATH, LATEST_EVIDENCE_PATH);
  } finally {
    fs.rmSync(LATEST_EVIDENCE_TEMP_PATH, { force: true });
  }
}

const metrics = await runBenchmark();
publishLatestEvidence(metrics);
console.log(JSON.stringify({ ...metrics, evidencePath: LATEST_EVIDENCE_PATH }, null, 2));

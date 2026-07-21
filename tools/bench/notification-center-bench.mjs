import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath, pathToFileURL } from "node:url";
import { performance } from "node:perf_hooks";

const SOURCE_MODE_ARGUMENT = "--notification-center-source-mode";
const BENCHMARK_DIRECTORY = path.dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = path.resolve(BENCHMARK_DIRECTORY, "../..");
const BENCHMARK_SCRATCH_ROOT = path.join(REPOSITORY_ROOT, ".tmp", "notification-center-bench");
const STORE_MODULE_URL = pathToFileURL(
  path.join(REPOSITORY_ROOT, "apps", "desktop", "src", "notification-store.ts")
);
const WORKER_URL = pathToFileURL(
  path.join(REPOSITORY_ROOT, "apps", "desktop", "src", "notification-store-worker.ts")
);

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

const THUMBNAIL_COMPLETIONS = 10_000;
const RAPID_PROGRESS_UPDATES = 10_000;
const COMPLETED_OPERATIONS = 2_100;
const SEEDED_ACTIVE_ROWS = 20;
const FIXED_NOW = Date.UTC(2026, 6, 20, 12, 0, 0);
const LIBRARIES = Array.from({ length: 4 }, (_, index) => ({
  id: `bench-library-${index + 1}`,
  name: `Benchmark Library ${index + 1}`
}));

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

async function withNotificationStoreRun({
  prefix,
  storeModuleUrl = STORE_MODULE_URL,
  onRunDirectory,
  execute
}) {
  let runDirectory;
  let store;
  try {
    fs.mkdirSync(BENCHMARK_SCRATCH_ROOT, { recursive: true });
    runDirectory = fs.mkdtempSync(path.join(BENCHMARK_SCRATCH_ROOT, prefix));
    assertInsideRepository(runDirectory);
    onRunDirectory?.(runDirectory);
    const databasePath = path.join(runDirectory, "notifications.sqlite");
    const startupStartedAt = performance.now();
    const { createNotificationStore } = await import(storeModuleUrl.href);
    store = await createNotificationStore({ databasePath, workerUrl: WORKER_URL });
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
  assert.ok(importRunDirectory && !fs.existsSync(importRunDirectory));

  let storeRunDirectory;
  let sidecarsObservedBeforeFailure = [];
  let storeFailure;
  try {
    await withNotificationStoreRun({
      prefix: `cleanup-store-${suffix}`,
      onRunDirectory(runDirectory) {
        storeRunDirectory = runDirectory;
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
  assert.ok(storeRunDirectory && !fs.existsSync(storeRunDirectory));

  return {
    importFailureRunRemoved: true,
    postCreateFailureRunRemoved: true,
    sidecarsObservedBeforeFailure
  };
}

function runCoreProbe(runDirectory) {
  const probePath = path.join(runDirectory, "core-outbox-probe.json");
  const coreDatabasePath = path.join(runDirectory, "core.sqlite");
  const startedAt = performance.now();
  const result = spawnSync(
    "cargo",
    [
      "test",
      "-p",
      "megle-core",
      "db::tests::notification_center_benchmark_probe",
      "--",
      "--ignored",
      "--exact",
      "--nocapture",
      "--test-threads=1"
    ],
    {
      cwd: REPOSITORY_ROOT,
      env: {
        ...process.env,
        MEGLE_NOTIFICATION_BENCH_CORE_DB_PATH: coreDatabasePath,
        MEGLE_NOTIFICATION_BENCH_PROBE_PATH: probePath
      },
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
      windowsHide: true
    }
  );
  const processElapsedMs = elapsedSince(startedAt);
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `production Core outbox probe failed (exit ${result.status})\n${result.stdout}\n${result.stderr}`
    );
  }
  assert.ok(fs.existsSync(probePath), "production Core probe must write its event evidence");
  const probe = JSON.parse(fs.readFileSync(probePath, "utf8"));

  const coreDatabase = new DatabaseSync(coreDatabasePath, { readOnly: true });
  let rows;
  try {
    rows = {
      persistedProgressEvents: scalar(
        coreDatabase,
        `SELECT COUNT(*) AS value FROM notification_outbox
         WHERE operation_id = ? AND sequence > ? AND sequence <= ?`,
        probe.rapidOperationId,
        probe.metrics.baselineSequence,
        probe.metrics.progressLastSequence
      ),
      terminalEventCount: scalar(
        coreDatabase,
        `SELECT COUNT(*) AS value FROM notification_outbox
         WHERE operation_id = ? AND sequence > ? AND sequence <= ?`,
        probe.rapidOperationId,
        probe.metrics.progressLastSequence,
        probe.metrics.terminalLastSequence
      ),
      rapidFinalRevision: scalar(
        coreDatabase,
        "SELECT revision AS value FROM notification_operation_heads WHERE operation_id = ?",
        probe.rapidOperationId
      ),
      outboxRowCount: scalar(
        coreDatabase,
        "SELECT COUNT(*) AS value FROM notification_outbox"
      )
    };
  } finally {
    coreDatabase.close();
  }

  assert.equal(probe.metrics.rapidProgressUpdates, RAPID_PROGRESS_UPDATES);
  assert.equal(probe.metrics.logicalProgressRevisionCount, RAPID_PROGRESS_UPDATES);
  assert.equal(rows.persistedProgressEvents, probe.metrics.persistedProgressEvents);
  assert.equal(rows.terminalEventCount, probe.metrics.terminalEventCount);
  assert.equal(rows.rapidFinalRevision, probe.metrics.rapidHeadRevision + rows.terminalEventCount);
  assert.equal(rows.outboxRowCount, probe.metrics.outboxRowCount);
  assert.equal(probe.events.length, rows.outboxRowCount);
  assert.equal(probe.activeOperationIds.length, SEEDED_ACTIVE_ROWS);
  assert.ok(
    rows.persistedProgressEvents <=
      probe.metrics.activeOperationSeconds + rows.terminalEventCount,
    "production Core persisted too many progress outbox events"
  );

  return { probe, rows, processElapsedMs };
}

async function runBenchmark() {
  const cleanupSelfTest = await runCleanupSelfTest();
  const totalStartedAt = performance.now();
  return withNotificationStoreRun({
    prefix: "run-",
    async execute({ store, closeStore, runDirectory, databasePath, startupElapsedMs }) {
      const coreProbe = runCoreProbe(runDirectory);

      const thumbnailStartedAt = performance.now();
      for (let index = 0; index < THUMBNAIL_COMPLETIONS; index += 1) {
        const library = LIBRARIES[index % LIBRARIES.length];
        const cachedCount = Math.floor(index / LIBRARIES.length) + 1;
        const libraryCandidateCount = Math.ceil(THUMBNAIL_COMPLETIONS / LIBRARIES.length);
        await store.publishThumbnailSummary({
          libraryId: library.id,
          libraryName: library.name,
          observedAt: FIXED_NOW + index,
          cachedCount,
          totalCandidateCount: libraryCandidateCount,
          missingCount: libraryCandidateCount - cachedCount,
          staleCount: 0,
          failedCount: 0,
          pendingCount: 0,
          activeBulkTaskCount: 0,
          totalCacheBytes: cachedCount * 1_024,
          completionPercent: (cachedCount / libraryCandidateCount) * 100
        });
      }
      const thumbnailElapsedMs = elapsedSince(thumbnailStartedAt);

      const progressStartedAt = performance.now();
      await store.applyOutboxBatch(
        LIBRARIES[0],
        coreProbe.probe.events,
        coreProbe.probe.latestSequence
      );
      const progressElapsedMs = elapsedSince(progressStartedAt);

      const syntheticCompletedCount =
        COMPLETED_OPERATIONS - coreProbe.rows.terminalEventCount;
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
      const compactStartedAt = performance.now();
      const compact = await store.compactSnapshot();
      const compactElapsedMs = elapsedSince(compactStartedAt);

      assert.equal(compact.active.length, SEEDED_ACTIVE_ROWS);
      assert.deepEqual(
        compact.active.map((operation) => operation.operationId).sort(),
        [...coreProbe.probe.activeOperationIds].sort()
      );
      assert.ok(compact.thumbnailSummaries.length <= LIBRARIES.length);

      const closeStartedAt = performance.now();
      await closeStore();
      const closeElapsedMs = elapsedSince(closeStartedAt);
      const benchmarkElapsedMs = elapsedSince(totalStartedAt);
      const storeWorkerElapsedMs = Number(
        (
          startupElapsedMs +
          thumbnailElapsedMs +
          progressElapsedMs +
          terminalElapsedMs +
          retentionElapsedMs +
          compactElapsedMs +
          closeElapsedMs
        ).toFixed(3)
      );

      const database = new DatabaseSync(databasePath, { readOnly: true });
      let rows;
      try {
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
          persistedProgressEvents: coreProbe.rows.persistedProgressEvents,
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

      const activeOperationSeconds = coreProbe.probe.metrics.activeOperationSeconds;
      const terminalEventCount = coreProbe.rows.terminalEventCount;
      assert.equal(rows.perThumbnailNotificationRows, 0);
      assert.ok(rows.thumbnailAggregateRows <= LIBRARIES.length);
      assert.ok(rows.persistedProgressEvents <= activeOperationSeconds + terminalEventCount);
      assert.ok(rows.completedHistoryRows <= 2_000);
      assert.equal(rows.activeRowsRetained, SEEDED_ACTIVE_ROWS);

      const metrics = {
        benchmark: "unified-notification-center",
        boundary:
          "Core task progress -> production operation head/outbox -> NotificationStore -> worker_threads -> NotificationStoreDatabase -> SQLite WAL",
        seed: {
          libraryCount: LIBRARIES.length,
          thumbnailCompletions: THUMBNAIL_COMPLETIONS,
          rapidProgressUpdates: RAPID_PROGRESS_UPDATES,
          completedOperations: COMPLETED_OPERATIONS,
          syntheticCompletedOperations: syntheticCompletedCount,
          activeOperations: SEEDED_ACTIVE_ROWS,
          activeOperationSeconds,
          terminalEventCount
        },
        coreOutbox: {
          inputProgressUpdateCount: RAPID_PROGRESS_UPDATES,
          logicalProgressRevisionCount: coreProbe.probe.metrics.logicalProgressRevisionCount,
          persistedProgressEventCount: coreProbe.rows.persistedProgressEvents,
          terminalEventCount,
          totalOutboxEventCount: coreProbe.rows.outboxRowCount,
          inputElapsedMs: Number(coreProbe.probe.metrics.inputElapsedMs.toFixed(3)),
          probeProcessElapsedMs: coreProbe.processElapsedMs
        },
        storeWorker: {
          notificationEventCount: coreProbe.probe.events.length + syntheticCompletedCount,
          coreOutboxEventCount: coreProbe.probe.events.length,
          thumbnailSummaryUpdateCount: THUMBNAIL_COMPLETIONS,
          writeRequestCount: THUMBNAIL_COMPLETIONS + 1 + LIBRARIES.length + 1,
          readRequestCount: 1,
          closeRequestCount: 1,
          totalRequestCount: THUMBNAIL_COMPLETIONS + 1 + LIBRARIES.length + 3,
          startupElapsedMs,
          thumbnailElapsedMs,
          progressElapsedMs,
          terminalElapsedMs,
          retentionElapsedMs,
          compactElapsedMs,
          closeElapsedMs,
          elapsedMs: storeWorkerElapsedMs
        },
        cleanupSelfTest,
        benchmarkElapsedMs,
        retention,
        rows,
        assertions: {
          perThumbnailRowsZero: true,
          aggregateRowsWithinLibraryCount: true,
          progressEventsWithinOperationSecondBudget: true,
          completedHistoryWithinLimit: true,
          allActiveRowsRetained: true,
          importFailureCleanupVerified: true,
          postCreateFailureCleanupVerified: true
        }
      };

      const evidencePath = path.join(BENCHMARK_SCRATCH_ROOT, "latest.json");
      fs.writeFileSync(evidencePath, `${JSON.stringify(metrics, null, 2)}\n`, "utf8");
      console.log(JSON.stringify({ ...metrics, evidencePath }, null, 2));
      return metrics;
    }
  });
}

await runBenchmark();

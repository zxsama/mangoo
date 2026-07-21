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
const ACTIVE_SECONDS = 10;
const FIXED_NOW = Date.UTC(2026, 6, 20, 12, 0, 0);
const LIBRARIES = Array.from({ length: 4 }, (_, index) => ({
  id: `bench-library-${index + 1}`,
  name: `Benchmark Library ${index + 1}`
}));

function elapsedSince(startedAt) {
  return Number((performance.now() - startedAt).toFixed(3));
}

function progressOperation(operationIndex, revision, second, currentValue) {
  const updatedAt = FIXED_NOW + second * 1_000;
  return {
    operationId: `bench:progress:${operationIndex}`,
    revision,
    kind: "root_scan",
    category: "task",
    origin: "background",
    presentation: "status_only",
    status: "running",
    phase: "scan",
    title: `Benchmark progress ${operationIndex}`,
    summary: null,
    taskId: operationIndex + 1,
    rootId: operationIndex + 1,
    folderId: null,
    fileId: null,
    currentValue,
    totalValue: RAPID_PROGRESS_UPDATES,
    unit: "files",
    errorCode: null,
    errorDetail: null,
    cancellable: true,
    retryable: false,
    attempt: 1,
    occurrenceCount: 1,
    startedAt: FIXED_NOW,
    updatedAt,
    completedAt: null
  };
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

function scalar(database, sql) {
  const row = database.prepare(sql).get();
  return Number(row?.value ?? 0);
}

function assertInsideRepository(targetPath) {
  const relative = path.relative(REPOSITORY_ROOT, targetPath);
  assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative));
}

async function runBenchmark() {
  fs.mkdirSync(BENCHMARK_SCRATCH_ROOT, { recursive: true });
  const runDirectory = fs.mkdtempSync(path.join(BENCHMARK_SCRATCH_ROOT, "run-"));
  assertInsideRepository(runDirectory);
  const databasePath = path.join(runDirectory, "notifications.sqlite");
  const workerUrl = pathToFileURL(
    path.join(REPOSITORY_ROOT, "apps", "desktop", "src", "notification-store-worker.ts")
  );
  const storeModuleUrl = pathToFileURL(
    path.join(REPOSITORY_ROOT, "apps", "desktop", "src", "notification-store.ts")
  );

  const totalStartedAt = performance.now();
  const { createNotificationStore } = await import(storeModuleUrl.href);
  const startupStartedAt = performance.now();
  const store = await createNotificationStore({ databasePath, workerUrl });
  const startupElapsedMs = elapsedSince(startupStartedAt);
  let storeClosed = false;

  try {
    const thumbnailStartedAt = performance.now();
    for (let index = 0; index < THUMBNAIL_COMPLETIONS; index += 1) {
      const library = LIBRARIES[index % LIBRARIES.length];
      const cachedCount = Math.floor(index / LIBRARIES.length) + 1;
      await store.publishThumbnailSummary({
        libraryId: library.id,
        libraryName: library.name,
        observedAt: FIXED_NOW + index,
        cachedCount,
        totalCandidateCount: Math.ceil(THUMBNAIL_COMPLETIONS / LIBRARIES.length),
        missingCount: Math.ceil(THUMBNAIL_COMPLETIONS / LIBRARIES.length) - cachedCount,
        staleCount: 0,
        failedCount: 0,
        pendingCount: 0,
        activeBulkTaskCount: 0,
        totalCacheBytes: cachedCount * 1_024,
        completionPercent:
          (cachedCount / Math.ceil(THUMBNAIL_COMPLETIONS / LIBRARIES.length)) * 100
      });
    }
    const thumbnailElapsedMs = elapsedSince(thumbnailStartedAt);

    const progressEvents = [];
    for (let index = 0; index < RAPID_PROGRESS_UPDATES; index += 1) {
      const operationIndex = index % SEEDED_ACTIVE_ROWS;
      const revision = Math.floor(index / SEEDED_ACTIVE_ROWS) + 1;
      const second = Math.min(
        ACTIVE_SECONDS - 1,
        Math.floor(index / (RAPID_PROGRESS_UPDATES / ACTIVE_SECONDS))
      );
      progressEvents.push({
        sequence: index + 1,
        operation: progressOperation(operationIndex, revision, second, index + 1)
      });
    }
    const progressStartedAt = performance.now();
    await store.applyOutboxBatch(
      LIBRARIES[0],
      progressEvents,
      RAPID_PROGRESS_UPDATES
    );
    const progressElapsedMs = elapsedSince(progressStartedAt);

    const completedByLibrary = LIBRARIES.map(() => []);
    for (let index = 0; index < COMPLETED_OPERATIONS; index += 1) {
      completedByLibrary[index % LIBRARIES.length].push(completedOperation(index));
    }
    const terminalStartedAt = performance.now();
    for (let libraryIndex = 0; libraryIndex < LIBRARIES.length; libraryIndex += 1) {
      const library = LIBRARIES[libraryIndex];
      const sequenceOffset = libraryIndex === 0 ? RAPID_PROGRESS_UPDATES : 0;
      const events = completedByLibrary[libraryIndex].map((operation, index) => ({
        sequence: sequenceOffset + index + 1,
        operation
      }));
      await store.applyOutboxBatch(
        library,
        events,
        sequenceOffset + events.length
      );
    }
    const terminalElapsedMs = elapsedSince(terminalStartedAt);

    const retentionStartedAt = performance.now();
    const retention = await store.cleanupRetention(FIXED_NOW);
    const retentionElapsedMs = elapsedSince(retentionStartedAt);
    const compact = await store.compactSnapshot();

    assert.equal(compact.active.length, SEEDED_ACTIVE_ROWS);
    assert.deepEqual(
      compact.active.map((operation) => operation.operationId).sort(),
      Array.from({ length: SEEDED_ACTIVE_ROWS }, (_, index) => `bench:progress:${index}`).sort()
    );
    assert.ok(compact.thumbnailSummaries.length <= LIBRARIES.length);

    await store.close();
    storeClosed = true;
    const storeWorkerElapsedMs = elapsedSince(totalStartedAt);

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
        persistedProgressEvents: scalar(
          database,
          `SELECT COUNT(*) AS value FROM operations
           WHERE status IN ('pending', 'running') AND json_extract(payload_json, '$.kind') = 'root_scan'`
        ),
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

    const activeOperationSeconds = SEEDED_ACTIVE_ROWS * ACTIVE_SECONDS;
    const terminalEventCount = COMPLETED_OPERATIONS;
    assert.equal(rows.perThumbnailNotificationRows, 0);
    assert.ok(rows.thumbnailAggregateRows <= LIBRARIES.length);
    assert.ok(rows.persistedProgressEvents <= activeOperationSeconds + terminalEventCount);
    assert.ok(rows.persistedProgressEvents <= activeOperationSeconds);
    assert.ok(rows.completedHistoryRows <= 2_000);
    assert.equal(rows.activeRowsRetained, SEEDED_ACTIVE_ROWS);

    const metrics = {
      benchmark: "unified-notification-center",
      boundary: "NotificationStore -> worker_threads -> NotificationStoreDatabase -> SQLite WAL",
      seed: {
        libraryCount: LIBRARIES.length,
        thumbnailCompletions: THUMBNAIL_COMPLETIONS,
        rapidProgressUpdates: RAPID_PROGRESS_UPDATES,
        completedOperations: COMPLETED_OPERATIONS,
        activeOperations: SEEDED_ACTIVE_ROWS,
        activeOperationSeconds,
        terminalEventCount
      },
      storeWorker: {
        notificationEventCount: RAPID_PROGRESS_UPDATES + COMPLETED_OPERATIONS,
        thumbnailSummaryUpdateCount: THUMBNAIL_COMPLETIONS,
        writeRequestCount: THUMBNAIL_COMPLETIONS + 1 + LIBRARIES.length + 1,
        totalRequestCount: THUMBNAIL_COMPLETIONS + 1 + LIBRARIES.length + 3,
        startupElapsedMs,
        thumbnailElapsedMs,
        progressElapsedMs,
        terminalElapsedMs,
        retentionElapsedMs,
        elapsedMs: storeWorkerElapsedMs
      },
      retention,
      rows,
      assertions: {
        perThumbnailRowsZero: true,
        aggregateRowsWithinLibraryCount: true,
        progressRowsWithinOperationSecondBudget: true,
        completedHistoryWithinLimit: true,
        allActiveRowsRetained: true
      }
    };

    const evidencePath = path.join(BENCHMARK_SCRATCH_ROOT, "latest.json");
    fs.writeFileSync(evidencePath, `${JSON.stringify(metrics, null, 2)}\n`, "utf8");
    console.log(JSON.stringify({ ...metrics, evidencePath }, null, 2));
  } finally {
    if (!storeClosed) await store.close().catch(() => undefined);
    fs.rmSync(runDirectory, { recursive: true, force: true });
  }
}

await runBenchmark();

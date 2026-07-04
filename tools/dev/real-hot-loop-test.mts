// Real hot-loop stress driver for the disclosure-scan / thumbnail priority flow
// against a huge real directory (millions of files):
//
// 1. Spawn megle-core with a known session token + dynamic data dir.
// 2. addRoot pointing at a large real directory and wait until the index has
//    grown past a threshold (or the scan finishes), so the loop below runs
//    against a realistic, large media set instead of an empty/tiny one.
// 3. Run TOTAL_OPERATIONS API operations mirroring real desktop browsing
//    (scroll / folderSwitch / priorityScope / thumbnailState / preview /
//    thumbnailBlob) built from a deterministic weighted plan, with a mid-run
//    bulk thumbnail-cache write storm to simulate write contention.
// 4. Settle every thumbnail touched by the last scroll window and assert
//    they all reach a terminal state, that ready blobs are valid WebP with
//    the byte size the state reported, and that none failed with
//    "database is locked".
// 5. Summarize p50/p95/max latency per operation category and, if
//    tools/dev/hot-loop/budgets.json exists, gate the run on it.
//
// This file is standalone: it does not import from real-load-test.mts, only
// from the pure tools/dev/hot-loop/* modules.
//
// Run via: node --experimental-strip-types tools/dev/real-hot-loop-test.mts

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

import { buildOperationPlan, type Operation } from "./hot-loop/operationPlan.mts";
import { summarize, checkBudgets, type Budgets, type Summary } from "./hot-loop/metrics.mts";

const TOKEN = "real-load-test-token";
const ADDR = process.env.MEGLE_REAL_LOAD_CORE_ADDR ?? "127.0.0.1:47391";
const BASE = `http://${ADDR}/api`;
const ROOT_PATH = (process.env.MEGLE_REAL_LOAD_ROOT ?? "Y:/Repository/Billfish/Artists").replace(/\\/g, "/");
const TOTAL_OPERATIONS = Number(process.env.MEGLE_HOTLOOP_OPS ?? 100);
const INDEX_THRESHOLD = Number(process.env.MEGLE_HOTLOOP_INDEX_THRESHOLD ?? 20000);
const INDEX_WAIT_TIMEOUT_MS = Number(process.env.MEGLE_HOTLOOP_INDEX_TIMEOUT_MS ?? 600_000);
const FOLDER_DISCOVERY_TARGET = Number(process.env.MEGLE_HOTLOOP_FOLDER_TARGET ?? 8);
const FOLDER_DISCOVERY_VISIT_CAP = 250;
const SETTLE_TIMEOUT_MS = Number(process.env.MEGLE_HOTLOOP_SETTLE_TIMEOUT_MS ?? 60_000);
const SETTLE_POLL_INTERVAL_MS = Number(process.env.MEGLE_HOTLOOP_SETTLE_POLL_INTERVAL_MS ?? 200);
const WRITE_STORM_OPERATION_INDEX = 20;
const WINDOW_LIMIT = 96;

const TERMINAL_TASK_STATUSES = new Set(["succeeded", "failed", "cancelled"]);
const TERMINAL_THUMBNAIL_STATES = new Set(["ready", "failed", "skipped_small"]);
const closedChildren = new WeakSet<ChildProcess>();

function log(...args: unknown[]) {
  // eslint-disable-next-line no-console
  console.log("[hotloop]", ...args);
}

// ---------------------------------------------------------------------------
// Process + HTTP helpers (copied verbatim from tools/dev/real-load-test.mts;
// this file must remain standalone, so they are duplicated rather than
// imported).
// ---------------------------------------------------------------------------

function isExited(child: ChildProcess): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

async function waitForClose(child: ChildProcess, timeoutMs: number): Promise<boolean> {
  if (closedChildren.has(child)) {
    return true;
  }
  return new Promise<boolean>((resolve) => {
    const onClose = () => {
      clearTimeout(timeout);
      resolve(true);
    };
    const timeout = setTimeout(() => {
      child.off("close", onClose);
      resolve(false);
    }, timeoutMs);
    child.once("close", onClose);
  });
}

async function runTaskkill(pid: number): Promise<void> {
  await new Promise<void>((resolve) => {
    const killer = spawn("taskkill", ["/pid", String(pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true
    });
    killer.once("error", () => resolve());
    killer.once("close", () => resolve());
  });
}

async function terminateProcessTree(child: ChildProcess, timeoutMs = 10_000): Promise<void> {
  if (isExited(child)) {
    await waitForClose(child, timeoutMs);
    return;
  }

  if (process.platform === "win32" && child.pid) {
    await runTaskkill(child.pid);
  } else if (child.pid) {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  } else {
    child.kill("SIGTERM");
  }

  if (await waitForClose(child, timeoutMs)) {
    return;
  }

  if (process.platform !== "win32" && child.pid) {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
    if (await waitForClose(child, timeoutMs)) {
      return;
    }
  }

  throw new Error(`process ${child.pid ?? "<unknown>"} did not exit after termination request`);
}

function spawnManaged(command: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv }): ChildProcess {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env,
    stdio: ["ignore", "pipe", "pipe"],
    shell: false,
    windowsHide: true,
    detached: process.platform !== "win32"
  });
  child.once("close", () => {
    closedChildren.add(child);
  });
  return child;
}

async function fetchJson<T>(apiPath: string, init: RequestInit = {}): Promise<T> {
  const response = await fetchResponse(apiPath, init);
  const body = await response.text();
  return body ? (JSON.parse(body) as T) : (null as unknown as T);
}

async function fetchResponse(apiPath: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set("x-megle-session", TOKEN);
  if (init.body && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const response = await fetch(`${BASE}${apiPath}`, { ...init, headers });
  if (!response.ok) {
    const body = await response.text();
    throw new Error(`${apiPath} -> ${response.status} ${response.statusText}: ${body}`);
  }
  return response;
}

async function waitForHealth(timeoutMs = 30_000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      await fetchJson<{ status?: string }>("/health");
      return;
    } catch {
      await delay(250);
    }
  }
  throw new Error("Core did not become healthy in time");
}

interface AcceptedRoot {
  accepted: boolean;
  taskId: number | null;
  rootId: number | null;
}

interface RootRecord {
  id: number;
  path: string;
  displayName: string;
  enabled: boolean;
  rootFolderId: number | null;
}

interface TaskRecord {
  id: number;
  kind: string;
  status: string;
  rootId: number | null;
  folderId: number | null;
  fileId: number | null;
  itemsSeen: number;
  foldersSeen: number;
  mediaFilesSeen: number;
  error: string | null;
}

interface Page<T> {
  items: T[];
  nextCursor: string | null;
}

interface FolderRecord {
  id: number;
  rootId: number;
  parentId: number | null;
  name: string;
  status: string;
}

interface MediaRecord {
  id: number;
  rootId: number;
  folderId: number;
  name: string;
  ext: string;
  size: number;
  width: number | null;
  height: number | null;
  thumbnailState: string | null;
  previewPlaceholder?: number[] | null;
}

interface ThumbnailResponse {
  fileId: number;
  target: string;
  state: string;
  width: number | null;
  height: number | null;
  byteSize: number | null;
  servedBy: string | null;
  asset: { width: number; height: number; byteSize: number } | null;
  error: string | null;
  updatedAt: number | null;
}

async function getTask(taskId: number): Promise<TaskRecord | null> {
  const tasks = await fetchJson<Page<TaskRecord>>("/tasks");
  return tasks.items.find((task) => task.id === taskId) ?? null;
}

// ---------------------------------------------------------------------------
// Hot-loop-specific response shapes / helpers.
// ---------------------------------------------------------------------------

// /media list responses additionally carry totalCount (present on the first
// page; may be null/omitted on later pages since Core only computes
// COUNT(*) once per query shape).
interface MediaPage extends Page<MediaRecord> {
  totalCount: number | null;
}

function isRetryableThumbnailFailure(thumbnail: ThumbnailResponse): boolean {
  return thumbnail.state === "failed" && /database is locked/i.test(thumbnail.error ?? "");
}

async function listMediaPage(
  rootId: number,
  options: { folderId?: number; limit?: number; offset?: number } = {}
): Promise<MediaPage> {
  const params = new URLSearchParams({
    rootId: String(rootId),
    limit: String(options.limit ?? WINDOW_LIMIT),
    sort: "mtime_desc"
  });
  if (options.folderId !== undefined) {
    params.set("folderId", String(options.folderId));
  }
  if (options.offset !== undefined) {
    params.set("offset", String(options.offset));
  }
  return fetchJson<MediaPage>(`/media?${params.toString()}`);
}

async function listFolderChildren(folderId: number, limit = 100): Promise<FolderRecord[]> {
  return (await fetchJson<Page<FolderRecord>>(`/folders/${folderId}/children?limit=${limit}`)).items;
}

async function waitForRootRecord(rootId: number, timeoutMs = 30_000): Promise<RootRecord> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const roots = await fetchJson<Page<RootRecord>>("/roots");
    const root = roots.items.find((item) => item.id === rootId);
    if (root?.rootFolderId) {
      return root;
    }
    await delay(250);
  }
  throw new Error(`root ${rootId} did not expose a rootFolderId`);
}

async function addRoot(): Promise<AcceptedRoot> {
  const root = await fetchJson<AcceptedRoot>("/roots", {
    method: "POST",
    body: JSON.stringify({ path: ROOT_PATH })
  });
  if (!root.accepted || !root.taskId || !root.rootId) {
    throw new Error(`addRoot did not produce a scan task: ${JSON.stringify(root)}`);
  }
  return root;
}

// Polls until the media index has grown past INDEX_THRESHOLD rows, or the
// root scan reaches a terminal status (whichever happens first). Returns the
// last observed totalCount so the operation plan can be sized against real
// data instead of guessing.
//
// With MEGLE_HOTLOOP_WAIT_SCAN_DONE=1 it instead waits for the root scan to
// finish, so the 100-op loop measures STEADY-STATE browsing (background
// thumbnail generation only) rather than browsing during the one-time initial
// scan. Without it, the loop runs while the scan is still writing — the harsher
// "browse while indexing" worst case.
async function waitForIndexThreshold(rootId: number, scanTaskId: number): Promise<number> {
  const waitScanDone = process.env.MEGLE_HOTLOOP_WAIT_SCAN_DONE === "1";
  const start = Date.now();
  let lastTotalCount = 0;
  while (Date.now() - start < INDEX_WAIT_TIMEOUT_MS) {
    const page = await listMediaPage(rootId, { limit: 1, offset: 0 });
    if (typeof page.totalCount === "number") {
      lastTotalCount = page.totalCount;
    }
    const scanTask = await getTask(scanTaskId);
    if (scanTask && TERMINAL_TASK_STATUSES.has(scanTask.status)) {
      log(
        `scan reached terminal status ${scanTask.status} ` +
          `(observed totalCount=${lastTotalCount})`
      );
      return lastTotalCount;
    }
    if (!waitScanDone && lastTotalCount >= INDEX_THRESHOLD) {
      return lastTotalCount;
    }
    const mode = waitScanDone ? "waiting for scan to finish" : `threshold=${INDEX_THRESHOLD}`;
    log(`indexing... totalCount=${lastTotalCount} (${mode}, scan=${scanTask?.status ?? "unknown"})`);
    await delay(500);
  }
  throw new Error(
    `index wait timed out after ${INDEX_WAIT_TIMEOUT_MS}ms (last observed totalCount=${lastTotalCount}, ` +
      `waitScanDone=${waitScanDone})`
  );
}

// BFS from the root folder looking for real subfolders that already have
// media, for use by folderSwitch operations. Caps traversal at 250 visited
// folders so this stays cheap even on a directory with huge fan-out.
async function discoverFoldersWithMedia(rootId: number, rootFolderId: number, desired: number): Promise<FolderRecord[]> {
  const result: FolderRecord[] = [];
  const queue: number[] = [rootFolderId];
  const visited = new Set<number>();

  while (queue.length > 0 && result.length < desired && visited.size < FOLDER_DISCOVERY_VISIT_CAP) {
    const parentId = queue.shift();
    if (parentId === undefined || visited.has(parentId)) {
      continue;
    }
    visited.add(parentId);

    let children: FolderRecord[];
    try {
      children = await listFolderChildren(parentId, 100);
    } catch {
      continue;
    }

    for (const child of children) {
      if (result.length >= desired) {
        break;
      }
      try {
        const page = await listMediaPage(rootId, { folderId: child.id, limit: 1 });
        if (page.items.length > 0) {
          result.push(child);
        }
      } catch {
        // ignore and keep exploring
      }
      queue.push(child.id);
    }
  }

  return result;
}

async function verifyThumbnailBlob(fileId: number, thumb: ThumbnailResponse): Promise<number> {
  if (thumb.fileId !== fileId) {
    throw new Error(`thumbnail response fileId=${thumb.fileId} did not match requested id=${fileId}`);
  }
  if (thumb.target !== "grid_320") {
    throw new Error(`ready thumbnail ${fileId} returned unexpected target: ${thumb.target}`);
  }
  const t0 = performance.now();
  const response = await fetchResponse(`/media/${fileId}/thumbnail/blob?target=grid_320`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  const elapsed = performance.now() - t0;
  const contentType = response.headers.get("content-type");
  if (contentType !== "image/webp") {
    throw new Error(`thumbnail blob ${fileId} content-type=${contentType ?? "null"}`);
  }
  if (thumb.byteSize !== null && bytes.length !== thumb.byteSize) {
    throw new Error(`thumbnail ${fileId} byte size mismatch: state=${thumb.byteSize} blob=${bytes.length}`);
  }
  const magic = Buffer.from(bytes.subarray(0, 4)).toString("ascii");
  const subtype = Buffer.from(bytes.subarray(8, 12)).toString("ascii");
  if (magic !== "RIFF" || subtype !== "WEBP") {
    throw new Error(`thumbnail ${fileId} has invalid header: ${magic}/${subtype} (${bytes.length} bytes)`);
  }
  return elapsed;
}

// ---------------------------------------------------------------------------
// Operation executors. Each returns elapsed ms for the single call it makes.
// ---------------------------------------------------------------------------

// Mirrors the UI's windowed keyset pagination: remembers the cursor that
// fetches the page after each loaded window, so sequential deep scroll pages by
// cursor (O(1)) instead of OFFSET (O(offset)). A scroll to an offset with no
// recorded cursor (a random jump) falls back to OFFSET, exactly as the UI does.
const scrollCursorByStartIndex = new Map<number, string>();

async function opScroll(rootId: number, offset: number, limit: number): Promise<number> {
  const cursor = scrollCursorByStartIndex.get(offset);
  const query = cursor
    ? `/media?rootId=${rootId}&limit=${limit}&sort=mtime_desc&cursor=${encodeURIComponent(cursor)}`
    : `/media?rootId=${rootId}&limit=${limit}&offset=${offset}&sort=mtime_desc`;
  const t0 = performance.now();
  const page = await fetchJson<Page<MediaRecord> & { nextCursor: string | null }>(query);
  const elapsed = performance.now() - t0;
  if (page.nextCursor) {
    scrollCursorByStartIndex.set(offset + page.items.length, page.nextCursor);
  }
  return elapsed;
}

async function opFolderSwitch(rootId: number, folder: { id: number }): Promise<number> {
  const t0 = performance.now();
  await fetchJson(`/media?rootId=${rootId}&folderId=${folder.id}&limit=96&sort=mtime_desc`);
  return performance.now() - t0;
}

async function opPriorityScope(
  rootId: number,
  ids: { selected: number[]; visible: number[]; ahead: number[] }
): Promise<number> {
  const t0 = performance.now();
  await fetchJson(`/tasks/thumbnail-priority-scope`, {
    method: "POST",
    body: JSON.stringify({
      rootId,
      selectedFileIds: ids.selected,
      visibleFileIds: ids.visible,
      aheadFileIds: ids.ahead
    })
  });
  return performance.now() - t0;
}

async function opThumbnailState(
  fileIds: number[],
  priority: string,
  readyFileIds: Set<number>
): Promise<number> {
  const t0 = performance.now();
  const responses = await Promise.all(
    fileIds.map((id) => fetchJson<ThumbnailResponse>(`/media/${id}/thumbnail?target=grid_320&priority=${priority}`))
  );
  for (const thumbnail of responses) {
    if (thumbnail.state === "ready") {
      readyFileIds.add(thumbnail.fileId);
    }
  }
  return performance.now() - t0;
}

async function opPreview(fileId: number): Promise<number> {
  const t0 = performance.now();
  const response = await fetchResponse(`/media/${fileId}/preview`);
  await response.body?.cancel();
  return performance.now() - t0;
}

// Returns elapsed ms on success, or null when the blob is not yet available
// (404). A blob only exists once a thumbnail's state is "ready"; during an
// active scan most tiles are not ready, so a 404 here is expected (the real UI
// only fetches blobs for ready tiles) and must not abort the run. Other
// non-2xx statuses are genuine errors and still throw.
async function opThumbnailBlob(fileId: number): Promise<number | null> {
  const t0 = performance.now();
  const response = await fetch(`${BASE}/media/${fileId}/thumbnail/blob?target=grid_320`, {
    headers: { "x-megle-session": TOKEN }
  });
  if (response.status === 404) {
    await response.body?.cancel();
    return null;
  }
  if (!response.ok) {
    const body = await response.text();
    throw new Error(`/media/${fileId}/thumbnail/blob -> ${response.status} ${response.statusText}: ${body}`);
  }
  await response.arrayBuffer();
  return performance.now() - t0;
}

// ---------------------------------------------------------------------------
// Main.
// ---------------------------------------------------------------------------

async function main() {
  if (!existsSync(ROOT_PATH)) {
    throw new Error(`Test directory does not exist: ${ROOT_PATH}`);
  }

  const dataDir = await mkdtemp(path.join(tmpdir(), "megle-hotloop-"));
  const dbPath = path.join(dataDir, "megle.sqlite");
  log(`Source dir: ${ROOT_PATH}`);
  log(`Data dir: ${dataDir}`);

  const env = {
    ...process.env,
    MEGLE_SESSION_TOKEN: TOKEN,
    MEGLE_CORE_ADDR: ADDR,
    MEGLE_DB_PATH: dbPath,
    RUST_LOG: "warn"
  };

  log("Spawning megle-core...");
  const core = spawnManaged("cargo", ["run", "--release", "-q", "-p", "megle-core"], {
    cwd: process.cwd(),
    env
  });
  core.stdout?.on("data", (chunk) => process.stdout.write(`[core] ${chunk}`));
  core.stderr?.on("data", (chunk) => process.stderr.write(`[core] ${chunk}`));

  let stopPromise: Promise<void> | null = null;
  const stop = () => {
    stopPromise ??= terminateProcessTree(core);
    return stopPromise;
  };
  const handleStopSignal = (signal: NodeJS.Signals) => {
    stop()
      .catch((error) => {
        // eslint-disable-next-line no-console
        console.error("[hotloop] FAILED TO STOP CORE:", error);
      })
      .finally(() => process.exit(signal === "SIGINT" ? 130 : 143));
  };
  process.once("SIGINT", handleStopSignal);
  process.once("SIGTERM", handleStopSignal);

  try {
    await waitForHealth();
    log("Core is healthy");

    log(`addRoot ${ROOT_PATH}`);
    const root = await addRoot();
    log(`scan task ${root.taskId} queued for root ${root.rootId}`);
    const rootId = root.rootId!;
    const scanTaskId = root.taskId!;

    const rootRecord = await waitForRootRecord(rootId);
    const mediaTotal = await waitForIndexThreshold(rootId, scanTaskId);
    log(`index threshold reached: totalCount=${mediaTotal}`);

    const folders = rootRecord.rootFolderId
      ? await discoverFoldersWithMedia(rootId, rootRecord.rootFolderId, FOLDER_DISCOVERY_TARGET)
      : [];
    log(`discovered ${folders.length} media-bearing folders for folderSwitch ops`);

    const plan: Operation[] = buildOperationPlan({ totalOperations: TOTAL_OPERATIONS, mediaTotal });

    const windowCache = new Map<number, MediaRecord[]>();
    async function windowAt(offset: number): Promise<MediaRecord[]> {
      const cached = windowCache.get(offset);
      if (cached) {
        return cached;
      }
      const page = await listMediaPage(rootId, { offset, limit: WINDOW_LIMIT });
      windowCache.set(offset, page.items);
      return page.items;
    }

    function nearestCachedOffset(windowStart: number): number | null {
      if (windowCache.size === 0) {
        return null;
      }
      let best: number | null = null;
      let bestDistance = Infinity;
      for (const offset of windowCache.keys()) {
        const distance = Math.abs(offset - windowStart);
        if (distance < bestDistance) {
          bestDistance = distance;
          best = offset;
        }
      }
      return best;
    }

    async function idsNearWindow(windowStart: number, count: number): Promise<number[]> {
      const nearestOffset = nearestCachedOffset(windowStart);
      const items =
        nearestOffset !== null && windowCache.has(nearestOffset)
          ? windowCache.get(nearestOffset)!
          : await windowAt(windowStart);
      return items.slice(0, count).map((item) => item.id);
    }

    // Seed the cache with an initial window so early ops (before the first
    // scroll executes) have ids to work with.
    await windowAt(0);

    const byCategory: Record<string, number[]> = {};
    function record(kind: string, elapsedMs: number) {
      (byCategory[kind] ??= []).push(elapsedMs);
    }

    let lastWindowStart = 0;
    let lastWindowIds: number[] = (windowCache.get(0) ?? []).map((item) => item.id);
    let writeStormFired = false;
    // Ids whose grid_320 thumbnail has been observed "ready", so the blob op
    // only fetches blobs that actually exist (mirrors the real UI, which never
    // requests a blob for a not-yet-ready tile).
    const readyFileIds = new Set<number>();

    for (let i = 0; i < plan.length; i += 1) {
      const op = plan[i];

      if (i === WRITE_STORM_OPERATION_INDEX && TOTAL_OPERATIONS > WRITE_STORM_OPERATION_INDEX && !writeStormFired) {
        writeStormFired = true;
        fetchJson(`/tasks/thumbnail-cache`, {
          method: "POST",
          body: JSON.stringify({
            rootId,
            includeDescendants: true,
            refreshMode: "staleOrMissing",
            limit: 4096
          })
        }).catch((error) => {
          log(`WARN: mid-run write storm request failed (continuing loop): ${String(error)}`);
        });
      }

      switch (op.kind) {
        case "scroll": {
          const elapsed = await opScroll(rootId, op.offset, op.limit);
          record("scroll", elapsed);
          const items = await windowAt(op.offset);
          lastWindowStart = op.offset;
          lastWindowIds = items.map((item) => item.id);
          break;
        }
        case "folderSwitch": {
          const folder = folders[op.folderIndex % Math.max(1, folders.length)];
          if (folder) {
            const elapsed = await opFolderSwitch(rootId, folder);
            record("folderSwitch", elapsed);
          } else {
            // No real subfolders discovered; fall back to a scroll so the
            // loop still exercises the API instead of crashing.
            const elapsed = await opScroll(rootId, lastWindowStart, WINDOW_LIMIT);
            record("folderSwitch", elapsed);
          }
          break;
        }
        case "priorityScope": {
          const ids = await idsNearWindow(op.windowStart, 17);
          const selected = ids.slice(0, 1);
          const visible = ids.slice(1, 9);
          const ahead = ids.slice(9, 17);
          const elapsed = await opPriorityScope(rootId, { selected, visible, ahead });
          record("priorityScope", elapsed);
          break;
        }
        case "thumbnailState": {
          const ids = await idsNearWindow(op.windowStart, 8);
          const elapsed = await opThumbnailState(ids, "visible", readyFileIds);
          record("thumbnailState", elapsed);
          break;
        }
        case "preview": {
          const ids = await idsNearWindow(op.windowStart, 1);
          if (ids.length > 0) {
            const elapsed = await opPreview(ids[0]);
            record("preview", elapsed);
          }
          break;
        }
        case "thumbnailBlob": {
          const ids = await idsNearWindow(op.windowStart, WINDOW_LIMIT);
          // Ensure we have a known-ready id in this window before fetching a
          // blob: a blob only exists once its thumbnail is ready, so poll the
          // window's state first (this also populates readyFileIds) and target a
          // ready id. This mirrors the UI, which only fetches a blob for a tile
          // whose state is ready, and guarantees the blob op is actually
          // measured in steady state instead of silently 404ing.
          if (!ids.some((id) => readyFileIds.has(id))) {
            await opThumbnailState(ids.slice(0, 8), "visible", readyFileIds);
          }
          const targetId = ids.find((id) => readyFileIds.has(id)) ?? ids[0];
          if (targetId !== undefined) {
            const elapsed = await opThumbnailBlob(targetId);
            if (elapsed !== null) {
              record("thumbnailBlob", elapsed);
            }
          }
          break;
        }
      }
    }

    log(`completed ${plan.length} operations`);

    // --- Task 4: settle-and-verify pass -------------------------------------

    let idsToSettle = lastWindowIds;
    if (idsToSettle.length === 0) {
      idsToSettle = await idsNearWindow(lastWindowStart, WINDOW_LIMIT);
    }

    const stuckIds: number[] = [];
    const lockedFailures: ThumbnailResponse[] = [];
    const terminalStates: ThumbnailResponse[] = [];

    if (idsToSettle.length === 0) {
      log("settle pass skipped: no ids available from the last window");
    } else {
      const pendingIds = new Set(idsToSettle);
      const settleStart = Date.now();
      while (pendingIds.size > 0 && Date.now() - settleStart < SETTLE_TIMEOUT_MS) {
        const polled = await Promise.all(
          [...pendingIds].map(
            async (fileId) => fetchJson<ThumbnailResponse>(`/media/${fileId}/thumbnail?target=grid_320&priority=visible`)
          )
        );
        for (const thumbnail of polled) {
          if (isRetryableThumbnailFailure(thumbnail)) {
            lockedFailures.push(thumbnail);
            continue;
          }
          if (TERMINAL_THUMBNAIL_STATES.has(thumbnail.state)) {
            terminalStates.push(thumbnail);
            pendingIds.delete(thumbnail.fileId);
          }
        }
        if (pendingIds.size > 0) {
          await delay(SETTLE_POLL_INTERVAL_MS);
        }
      }
      for (const fileId of pendingIds) {
        stuckIds.push(fileId);
      }

      for (const thumbnail of terminalStates) {
        if (thumbnail.state === "ready") {
          // Records real blob-fetch latency for a known-ready thumbnail: the
          // in-loop thumbnailBlob op can legitimately find no ready id while
          // generation is still in flight, so the settle pass (all ids ready)
          // is where the blob-serve latency budget is reliably sampled.
          const elapsed = await verifyThumbnailBlob(thumbnail.fileId, thumbnail);
          record("thumbnailBlob", elapsed);
        }
      }
    }

    const settleSummary = {
      ready: terminalStates.filter((t) => t.state === "ready").length,
      skipped: terminalStates.filter((t) => t.state === "skipped_small").length,
      failed: terminalStates.filter((t) => t.state === "failed").length,
      stuck: stuckIds.length,
      lockedFailures: lockedFailures.length
    };
    log(`settle pass: ${JSON.stringify(settleSummary)}`);

    if (stuckIds.length > 0) {
      throw new Error(`thumbnails never reached terminal state: ${stuckIds.join(", ")}`);
    }
    if (lockedFailures.length > 0) {
      const uniqueLockedIds = new Set(lockedFailures.map((thumbnail) => thumbnail.fileId));
      throw new Error(
        `${lockedFailures.length} poll(s) across ${uniqueLockedIds.size} file id(s) failed with "database is locked": ` +
          `${[...uniqueLockedIds].join(", ")}`
      );
    }

    // --- Budget gate ---------------------------------------------------------

    const budgetsPath = new URL("./hot-loop/budgets.json", import.meta.url);
    const budgets: Budgets = existsSync(budgetsPath) ? JSON.parse(readFileSync(budgetsPath, "utf8")) : {};
    const summary: Summary = summarize(byCategory);
    console.log("[hotloop] summary:", JSON.stringify(summary, null, 2));
    const violations = checkBudgets(summary, budgets);
    if (violations.length > 0) {
      log(`budget violations:\n  ${violations.join("\n  ")}`);
    } else if (Object.keys(budgets).length > 0) {
      log("all budgets satisfied");
    } else {
      log("no budgets.json present; skipping budget gate");
    }

    if (Object.keys(budgets).length > 0 && violations.length > 0) {
      throw new Error(`hot-loop budget violations:\n  ${violations.join("\n  ")}`);
    }

    log("=== SUCCESS ===");
  } finally {
    process.off("SIGINT", handleStopSignal);
    process.off("SIGTERM", handleStopSignal);
    await stop();
    if (process.env.MEGLE_KEEP_DATA !== "1") {
      try {
        await rm(dataDir, { recursive: true, force: true });
        log(`Cleaned ${dataDir}`);
      } catch {
        // ignore
      }
    } else {
      log(`MEGLE_KEEP_DATA=1, keeping ${dataDir}`);
    }
  }
}

main().catch((error) => {
  // eslint-disable-next-line no-console
  console.error("[hotloop] FAILED:", error);
  process.exit(1);
});

// End-to-end real load test for the disclosure-scan priority flow:
// 1. Spawn megle-core with a known session token + dynamic data dir.
// 2. addRoot pointing at the real Stable Diffusion outputs directory.
// 3. Measure add-root -> first visible media before the root scan completes.
// 4. Switch between real folders during the active scan.
// 5. Request only current visible grid_320 thumbnails and prove background
//    folders stay uncleared until requested.
// 6. Verify the center preview path streams the original media bytes.
//
// Run via: node --experimental-strip-types tools/dev/real-load-test.mts

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import {
  addExistingMediaRoot,
  createAndActivateManagedLibrary,
  managedLibraryHostEnv
} from "./managed-library-harness.mjs";

const TOKEN = "real-load-test-token";
const ADDR = process.env.MEGLE_REAL_LOAD_CORE_ADDR ?? "127.0.0.1:47391";
const BASE = `http://${ADDR}/api`;
const ROOT_PATH = normalizeRootPath(
  process.env.MEGLE_REAL_LOAD_ROOT ??
    "G:/AI_Painter/stable-diffusion/stable-diffusion-webui/outputs"
);
const VISIBLE_LIMIT = Number(process.env.MEGLE_REAL_LOAD_VISIBLE_LIMIT ?? 24);
const PRIORITY_SCOPE_COUNT = Number(process.env.MEGLE_REAL_LOAD_PRIORITY_SCOPE_COUNT ?? 4);
const PRIORITY_SCOPE_PAGE_LIMIT = 1 + PRIORITY_SCOPE_COUNT * 2;
const SWITCH_SAMPLE_LIMIT = Number(process.env.MEGLE_REAL_LOAD_SWITCH_LIMIT ?? 24);
const SCAN_TIMEOUT_MS = Number(process.env.MEGLE_REAL_LOAD_SCAN_TIMEOUT_MS ?? 300_000);
const DISCLOSURE_TIMEOUT_MS = Number(process.env.MEGLE_REAL_LOAD_DISCLOSURE_TIMEOUT_MS ?? 120_000);
const THUMBNAIL_TIMEOUT_MS = Number(process.env.MEGLE_REAL_LOAD_THUMBNAIL_TIMEOUT_MS ?? 90_000);
const THUMBNAIL_POLL_INTERVAL_MS = Number(process.env.MEGLE_REAL_LOAD_THUMBNAIL_POLL_INTERVAL_MS ?? 100);
const MEDIA_EXTENSIONS = new Set([
  ".avif",
  ".avi",
  ".bmp",
  ".gif",
  ".heic",
  ".jpeg",
  ".jpg",
  ".m4v",
  ".mkv",
  ".mov",
  ".mp4",
  ".png",
  ".psd",
  ".raw",
  ".webm",
  ".wmv"
]);
const TERMINAL_TASK_STATUSES = new Set(["succeeded", "failed", "cancelled"]);
const TERMINAL_THUMBNAIL_STATES = new Set(["ready", "failed", "skipped_small"]);
const closedChildren = new WeakSet<ChildProcess>();

type ThumbnailPriority = "background" | "ahead" | "visible" | "selected";

function log(...args: unknown[]) {
  // eslint-disable-next-line no-console
  console.log("[test]", ...args);
}

function normalizeRootPath(input: string): string {
  return input.replace(/\\/g, "/");
}

function elapsedSince(startedAt: number): number {
  return Date.now() - startedAt;
}

function formatMs(ms: number): string {
  return `${ms}ms`;
}

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

async function runLifecycleSelfTest(): Promise<void> {
  const parentScript = [
    "const { spawn } = require('node:child_process');",
    "const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'inherit' });",
    "console.log(`[lifecycle-parent] ${process.pid} child ${child.pid}`);",
    "setInterval(() => {}, 1000);"
  ].join(" ");
  const child = spawnManaged(process.execPath, ["-e", parentScript], {
    cwd: process.cwd(),
    env: process.env
  });
  child.stdout?.on("data", (chunk) => process.stdout.write(`[self-test] ${chunk}`));
  child.stderr?.on("data", (chunk) => process.stderr.write(`[self-test] ${chunk}`));
  await delay(500);
  const startedAt = Date.now();
  await terminateProcessTree(child, 5_000);
  log(`Lifecycle self-test stopped process tree in ${Date.now() - startedAt}ms`);
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

async function fetchThumbnailBlob(
  fileId: number
): Promise<{ bytes: Uint8Array; contentType: string | null; servedBy: string | null }> {
  const response = await fetchResponse(`/media/${fileId}/thumbnail/blob?target=grid_320`);
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    contentType: response.headers.get("content-type"),
    servedBy: response.headers.get("x-megle-served-by")
  };
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

interface MediaFolder {
  folder: FolderRecord;
  media: MediaRecord[];
}

interface ScopeClearResult {
  label: string;
  itemCount: number;
  pendingOrQueuedAtStart: number;
  ready: number;
  skipped: number;
  failed: number;
  firstTerminalElapsedMs: number;
  settledElapsedMs: number;
}

interface PriorityScopePlan {
  currentFolder: MediaFolder;
  backgroundFolder: MediaFolder;
  selected: MediaRecord;
  visibleScope: MediaRecord[];
  aheadScope: MediaRecord[];
}

async function getTask(taskId: number): Promise<TaskRecord | null> {
  const tasks = await fetchJson<Page<TaskRecord>>("/tasks");
  return tasks.items.find((task) => task.id === taskId) ?? null;
}

async function pollTask(taskId: number, label: string, timeoutMs = 60_000): Promise<TaskRecord> {
  const start = Date.now();
  let last: TaskRecord | null = null;
  while (Date.now() - start < timeoutMs) {
    last = await getTask(taskId);
    if (!last) {
      await delay(200);
      continue;
    }
    if (TERMINAL_TASK_STATUSES.has(last.status)) {
      return last;
    }
    log(
      `${label} task ${taskId} -> ${last.status} ` +
        `(items=${last.itemsSeen}, folders=${last.foldersSeen}, media=${last.mediaFilesSeen})`
    );
    await delay(1_000);
  }
  if (!last) {
    throw new Error(`task ${taskId} never appeared`);
  }
  throw new Error(`${label} task ${taskId} timed out in status ${last.status}`);
}

async function requestThumbnail(
  fileId: number,
  priority: ThumbnailPriority = "background"
): Promise<ThumbnailResponse> {
  return fetchJson<ThumbnailResponse>(`/media/${fileId}/thumbnail?target=grid_320&priority=${priority}`);
}

async function enqueueInteractiveFolderScan(folderId: number): Promise<AcceptedRoot> {
  return fetchJson<AcceptedRoot>("/tasks/interactive-folder-scan", {
    method: "POST",
    body: JSON.stringify({ folderId })
  });
}

async function listRoots(): Promise<RootRecord[]> {
  const page = await fetchJson<Page<RootRecord>>("/roots");
  return page.items;
}

async function listFolderChildren(folderId: number, limit = 200): Promise<FolderRecord[]> {
  const folders: FolderRecord[] = [];
  let cursor: string | undefined;
  do {
    const page: Page<FolderRecord> = await fetchJson(
      `/folders/${folderId}/children?limit=${limit}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`
    );
    folders.push(...page.items);
    cursor = page.nextCursor ?? undefined;
  } while (cursor);
  return folders;
}

async function listMediaPage(rootId: number, folderId?: number, limit = 200): Promise<Page<MediaRecord>> {
  const params = new URLSearchParams({
    rootId: String(rootId),
    limit: String(limit),
    sort: "mtime_desc"
  });
  if (folderId !== undefined) {
    params.set("folderId", String(folderId));
  }
  return fetchJson<Page<MediaRecord>>(`/media?${params.toString()}`);
}

async function listAllMedia(rootId: number): Promise<MediaRecord[]> {
  const media: MediaRecord[] = [];
  let cursor: string | undefined;
  do {
    const params = new URLSearchParams({
      rootId: String(rootId),
      limit: "500",
      sort: "mtime_desc"
    });
    if (cursor) {
      params.set("cursor", cursor);
    }
    const page = await fetchJson<Page<MediaRecord>>(`/media?${params.toString()}`);
    media.push(...page.items);
    cursor = page.nextCursor ?? undefined;
  } while (cursor);
  return media;
}

async function waitForRootRecord(rootId: number, timeoutMs = 30_000): Promise<RootRecord> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const root = (await listRoots()).find((item) => item.id === rootId);
    if (root?.rootFolderId) {
      return root;
    }
    await delay(250);
  }
  throw new Error(`root ${rootId} did not expose a rootFolderId`);
}

async function waitForFirstVisibleMedia(
  rootId: number,
  scanTaskId: number,
  addRootStartedAt: number
): Promise<{ media: MediaRecord[]; elapsedMs: number; scanStatus: string }> {
  const start = Date.now();
  while (Date.now() - start < DISCLOSURE_TIMEOUT_MS) {
    const page = await listMediaPage(rootId, undefined, VISIBLE_LIMIT);
    const scanTask = await getTask(scanTaskId);
    if (page.items.length > 0) {
      return {
        media: page.items,
        elapsedMs: elapsedSince(addRootStartedAt),
        scanStatus: scanTask?.status ?? "unknown"
      };
    }
    if (scanTask && TERMINAL_TASK_STATUSES.has(scanTask.status)) {
      throw new Error(`scan reached ${scanTask.status} before any visible media was listed`);
    }
    await delay(250);
  }
  throw new Error("current root view did not show visible media during scan");
}

async function discoverMediaFoldersOnce(rootId: number, rootFolderId: number, desired: number): Promise<MediaFolder[]> {
  const result: MediaFolder[] = [];
  const queue = [rootFolderId];
  const visited = new Set<number>();

  while (queue.length > 0 && result.length < desired && visited.size < 250) {
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
      const page = await listMediaPage(rootId, child.id, SWITCH_SAMPLE_LIMIT);
      if (page.items.length > 0) {
        result.push({ folder: child, media: page.items });
      }
      queue.push(child.id);
    }
  }

  return result;
}

async function waitForFoldersWithMedia(
  rootId: number,
  rootFolderId: number,
  scanTaskId: number,
  desired = 2
): Promise<{ folders: MediaFolder[]; scanStatus: string }> {
  const start = Date.now();
  while (Date.now() - start < DISCLOSURE_TIMEOUT_MS) {
    const folders = await discoverMediaFoldersOnce(rootId, rootFolderId, desired);
    const scanTask = await getTask(scanTaskId);
    if (folders.length >= desired) {
      return { folders, scanStatus: scanTask?.status ?? "unknown" };
    }
    if (scanTask && TERMINAL_TASK_STATUSES.has(scanTask.status) && folders.length > 0) {
      return { folders, scanStatus: scanTask.status };
    }
    await delay(500);
  }
  throw new Error(`did not discover ${desired} media-bearing folders during scan`);
}

async function verifyFolderSwitching(rootId: number, folders: MediaFolder[]): Promise<number[]> {
  const timings: number[] = [];
  for (const candidate of folders.slice(0, 2)) {
    const startedAt = Date.now();
    const page = await listMediaPage(rootId, candidate.folder.id, SWITCH_SAMPLE_LIMIT);
    const elapsedMs = elapsedSince(startedAt);
    if (page.items.length === 0) {
      throw new Error(`folder switch to ${candidate.folder.name} returned no media`);
    }
    timings.push(elapsedMs);
    log(`folder switch: ${candidate.folder.name} -> ${page.items.length} media in ${formatMs(elapsedMs)}`);
  }
  return timings;
}

async function verifyThumbnailBlob(fileId: number, thumb: ThumbnailResponse): Promise<void> {
  if (thumb.target !== "grid_320") {
    throw new Error(`ready thumbnail ${fileId} returned unexpected target: ${thumb.target}`);
  }
  if (thumb.servedBy !== "db_blob") {
    throw new Error(`ready thumbnail ${fileId} servedBy=${thumb.servedBy ?? "null"}`);
  }
  if (!thumb.width || !thumb.height || !thumb.byteSize) {
    throw new Error(`ready thumbnail ${fileId} missing dimensions or byte size`);
  }
  const blob = await fetchThumbnailBlob(fileId);
  if (blob.servedBy !== "db_blob") {
    throw new Error(`thumbnail blob ${fileId} servedBy=${blob.servedBy ?? "null"}`);
  }
  if (blob.contentType !== "image/webp") {
    throw new Error(`thumbnail blob ${fileId} content-type=${blob.contentType ?? "null"}`);
  }
  if (blob.bytes.length !== thumb.byteSize) {
    throw new Error(`thumbnail ${fileId} byte size mismatch: state=${thumb.byteSize} blob=${blob.bytes.length}`);
  }
  const magic = Buffer.from(blob.bytes.subarray(0, 4)).toString("ascii");
  const subtype = Buffer.from(blob.bytes.subarray(8, 12)).toString("ascii");
  if (magic !== "RIFF" || subtype !== "WEBP") {
    throw new Error(`thumbnail ${fileId} has invalid header: ${magic}/${subtype} (${blob.bytes.length} bytes)`);
  }
}

async function hydrateFolderMediaSamples(
  rootId: number,
  folders: MediaFolder[],
  limit: number
): Promise<MediaFolder[]> {
  const samples = await Promise.all(
    folders.map(async ({ folder }) => ({
      folder,
      media: (await listMediaPage(rootId, folder.id, limit)).items
    }))
  );
  return samples.filter((sample) => sample.media.length > 0);
}

function planPriorityScopes(candidates: MediaFolder[]): PriorityScopePlan {
  const ranked = [...candidates].sort((left, right) => right.media.length - left.media.length);
  const currentFolder = ranked.find((candidate) => Math.floor((candidate.media.length - 1) / 2) >= 2);
  if (!currentFolder) {
    throw new Error(
      `did not find a folder with enough media for selected/visible/ahead samples (need 1 + 2 scopes, saw ${ranked
        .map((candidate) => `${candidate.folder.name}:${candidate.media.length}`)
        .join(", ")})`
    );
  }

  const backgroundFolder = ranked.find((candidate) => candidate.folder.id !== currentFolder.folder.id);
  if (!backgroundFolder) {
    throw new Error("did not find a second folder for background priority coverage");
  }

  const tierCount = Math.min(PRIORITY_SCOPE_COUNT, Math.floor((currentFolder.media.length - 1) / 2));
  const scopedMedia = currentFolder.media.slice(0, 1 + tierCount * 2);
  const [selected, ...remaining] = scopedMedia;
  if (!selected) {
    throw new Error(`current folder ${currentFolder.folder.name} produced no selected media sample`);
  }

  const visibleScope = remaining.slice(0, tierCount);
  const aheadScope = remaining.slice(tierCount, tierCount * 2);
  if (visibleScope.length !== tierCount || aheadScope.length !== tierCount) {
    throw new Error(
      `current folder ${currentFolder.folder.name} did not retain balanced visible/ahead scopes ` +
        `(visible=${visibleScope.length}, ahead=${aheadScope.length}, tierCount=${tierCount})`
    );
  }

  return {
    currentFolder,
    backgroundFolder,
    selected,
    visibleScope,
    aheadScope
  };
}

async function primeThumbnailScope(
  media: MediaRecord[],
  priority: ThumbnailPriority
): Promise<Map<number, ThumbnailResponse>> {
  const entries = await Promise.all(
    media.map(async (item) => [item.id, await requestThumbnail(item.id, priority)] as const)
  );
  return new Map(entries);
}

async function settleThumbnailScope(
  label: string,
  media: MediaRecord[],
  priority: ThumbnailPriority,
  startedAt: number,
  initialStates: Map<number, ThumbnailResponse>,
  timeoutMs = THUMBNAIL_TIMEOUT_MS
): Promise<ScopeClearResult> {
  const pendingIds = new Set(media.map((item) => item.id));
  const terminalStates = new Map<number, ThumbnailResponse>();
  const pendingOrQueuedAtStart = [...initialStates.values()].filter(
    (thumbnail) => !TERMINAL_THUMBNAIL_STATES.has(thumbnail.state)
  ).length;
  let firstTerminalElapsedMs: number | null = null;
  const start = Date.now();

  function recordTerminal(thumbnail: ThumbnailResponse): void {
    terminalStates.set(thumbnail.fileId, thumbnail);
    pendingIds.delete(thumbnail.fileId);
    if (firstTerminalElapsedMs === null) {
      firstTerminalElapsedMs = elapsedSince(startedAt);
    }
  }

  for (const thumbnail of initialStates.values()) {
    if (TERMINAL_THUMBNAIL_STATES.has(thumbnail.state) && !isRetryableThumbnailFailure(thumbnail)) {
      recordTerminal(thumbnail);
    }
  }

  while (pendingIds.size > 0) {
    if (Date.now() - start >= timeoutMs) {
      throw new Error(
        `${label} thumbnails timed out after ${timeoutMs}ms with pending ids ${[...pendingIds].join(", ")}`
      );
    }

    const polled = await Promise.all(
      [...pendingIds].map(async (fileId) => [fileId, await requestThumbnail(fileId, priority)] as const)
    );
    for (const [, thumbnail] of polled) {
      if (TERMINAL_THUMBNAIL_STATES.has(thumbnail.state) && !isRetryableThumbnailFailure(thumbnail)) {
        recordTerminal(thumbnail);
      }
    }

    if (pendingIds.size > 0) {
      await delay(THUMBNAIL_POLL_INTERVAL_MS);
    }
  }

  const settledElapsedMs = elapsedSince(startedAt);
  const summary = { ready: 0, skipped: 0, failed: 0 };
  for (const thumbnail of terminalStates.values()) {
    if (thumbnail.state === "ready") {
      summary.ready += 1;
    } else if (thumbnail.state === "skipped_small") {
      summary.skipped += 1;
    } else {
      summary.failed += 1;
      log(`  ${label} ${thumbnail.fileId} -> failed: ${thumbnail.error}`);
    }
  }

  await Promise.all(
    [...terminalStates.values()]
      .filter((thumbnail) => thumbnail.state === "ready")
      .map((thumbnail) => verifyThumbnailBlob(thumbnail.fileId, thumbnail))
  );

  return {
    label,
    itemCount: media.length,
    pendingOrQueuedAtStart,
    ready: summary.ready,
    skipped: summary.skipped,
    failed: summary.failed,
    firstTerminalElapsedMs: firstTerminalElapsedMs ?? settledElapsedMs,
    settledElapsedMs
  };
}

function isRetryableThumbnailFailure(thumbnail: ThumbnailResponse): boolean {
  return thumbnail.state === "failed" && /database is locked/i.test(thumbnail.error ?? "");
}

async function verifyOriginalPreview(media: MediaRecord): Promise<{ elapsedMs: number; contentType: string | null }> {
  const startedAt = Date.now();
  const response = await fetchResponse(`/media/${media.id}/preview`);
  const contentLength = Number(response.headers.get("content-length"));
  const servedBy = response.headers.get("x-megle-served-by");
  const cacheControl = response.headers.get("cache-control");
  const contentType = response.headers.get("content-type");
  await response.body?.cancel();

  if (contentLength !== media.size) {
    throw new Error(`preview ${media.id} content-length=${contentLength}, expected original size=${media.size}`);
  }
  if (servedBy === "db_blob") {
    throw new Error(`preview ${media.id} was served by thumbnail blob path`);
  }
  if (cacheControl !== "private, max-age=0, must-revalidate") {
    throw new Error(`preview ${media.id} cache-control=${cacheControl ?? "null"}`);
  }
  return { elapsedMs: elapsedSince(startedAt), contentType };
}

async function countMediaFiles(rootPath: string): Promise<number> {
  let count = 0;
  const queue = [rootPath];
  while (queue.length > 0) {
    const current = queue.shift();
    if (!current) {
      continue;
    }
    let entries;
    try {
      entries = await readdir(current, { withFileTypes: true });
    } catch (error) {
      log(`WARN: cannot read ${current}: ${String(error)}`);
      continue;
    }
    for (const entry of entries) {
      const child = path.join(current, entry.name);
      if (entry.isDirectory()) {
        queue.push(child);
      } else if (entry.isFile() && MEDIA_EXTENSIONS.has(path.extname(entry.name).toLowerCase())) {
        count++;
      }
    }
  }
  return count;
}

async function main() {
  if (!existsSync(ROOT_PATH)) {
    throw new Error(`Test directory does not exist: ${ROOT_PATH}`);
  }
  const expectedFiles = await countMediaFiles(ROOT_PATH);
  log(`Source dir: ${ROOT_PATH} (${expectedFiles} supported media files, recursive)`);

  const dataDir = await mkdtemp(path.join(tmpdir(), "megle-real-load-"));
  const managedRoot = path.join(dataDir, "library");
  log(`Data dir: ${dataDir}`);

  const env = {
    ...process.env,
    MEGLE_SESSION_TOKEN: TOKEN,
    MEGLE_CORE_ADDR: ADDR,
    ...managedLibraryHostEnv(dataDir),
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
        console.error("[test] FAILED TO STOP CORE:", error);
      })
      .finally(() => process.exit(signal === "SIGINT" ? 130 : 143));
  };
  process.once("SIGINT", handleStopSignal);
  process.once("SIGTERM", handleStopSignal);

  try {
    await waitForHealth();
    log("Core is healthy");

    const library = await createAndActivateManagedLibrary({
      baseUrl: BASE,
      token: TOKEN,
      name: "Real load test",
      managedRoot
    });

    log(`addRoot ${ROOT_PATH}`);
    const addRootStartedAt = Date.now();
    const root = await addExistingMediaRoot({
      baseUrl: BASE,
      token: TOKEN,
      contextRevision: library.contextRevision,
      mediaRoot: ROOT_PATH
    });
    if (!root.accepted || !root.taskId || !root.rootId) {
      throw new Error(`addRoot did not produce a scan task: ${JSON.stringify(root)}`);
    }
    log(`scan task ${root.taskId} queued for root ${root.rootId}`);

    const rootRecord = await waitForRootRecord(root.rootId);
    const firstVisible = await waitForFirstVisibleMedia(root.rootId, root.taskId, addRootStartedAt);
    log(
      `first visible current-root media: ${firstVisible.media.length} items in ` +
        `${formatMs(firstVisible.elapsedMs)} while scan=${firstVisible.scanStatus}`
    );

    const discovered = await waitForFoldersWithMedia(root.rootId, rootRecord.rootFolderId!, root.taskId, 4);
    log(
      `media folders discovered while scan=${discovered.scanStatus}: ` +
        discovered.folders.map((item) => `${item.folder.name}(${item.media.length})`).join(", ")
    );
    if (discovered.scanStatus === "succeeded") {
      log("WARN: real-directory scan completed before folder switching assertion could observe an active scan");
    }

    const switchTimings = await verifyFolderSwitching(root.rootId, discovered.folders);

    const folderSamples = await hydrateFolderMediaSamples(
      root.rootId,
      discovered.folders,
      Math.max(PRIORITY_SCOPE_PAGE_LIMIT, SWITCH_SAMPLE_LIMIT)
    );
    const priorityPlan = planPriorityScopes(folderSamples);
    const backgroundSample = priorityPlan.backgroundFolder.media[0];
    if (!backgroundSample) {
      throw new Error("background folder sample is empty");
    }

    const backgroundReadyBefore = priorityPlan.backgroundFolder.media.filter(
      (item) => item.thumbnailState === "ready"
    ).length;
    if (backgroundReadyBefore > 0) {
      throw new Error(
        `background folder ${priorityPlan.backgroundFolder.folder.name} already had ` +
          `${backgroundReadyBefore} ready thumbnails before being requested`
      );
    }

    log(
      `priority scopes: current=${priorityPlan.currentFolder.folder.name} ` +
        `(selected=1 visible=${priorityPlan.visibleScope.length} ahead=${priorityPlan.aheadScope.length}) ` +
        `background=${priorityPlan.backgroundFolder.folder.name}; ` +
        `background ready before request=${backgroundReadyBefore}/${priorityPlan.backgroundFolder.media.length}`
    );

    const interactiveScanStartedAt = Date.now();
    const interactiveScan = await enqueueInteractiveFolderScan(priorityPlan.currentFolder.folder.id);
    if (!interactiveScan.accepted || !interactiveScan.taskId) {
      throw new Error(
        `interactive folder scan was not accepted for ${priorityPlan.currentFolder.folder.name}: ` +
          `${JSON.stringify(interactiveScan)}`
      );
    }
    const interactiveTask = await pollTask(
      interactiveScan.taskId,
      "interactive folder scan",
      DISCLOSURE_TIMEOUT_MS
    );
    const interactiveTaskElapsedMs = elapsedSince(interactiveScanStartedAt);
    if (interactiveTask.kind !== "interactive_folder_scan") {
      throw new Error(`interactive folder scan task ${interactiveTask.id} reported kind=${interactiveTask.kind}`);
    }
    if (interactiveTask.folderId !== priorityPlan.currentFolder.folder.id) {
      throw new Error(
        `interactive folder scan task ${interactiveTask.id} targeted folder ${interactiveTask.folderId}, ` +
          `expected ${priorityPlan.currentFolder.folder.id}`
      );
    }
    if (interactiveTask.status !== "succeeded") {
      throw new Error(`interactive folder scan failed: ${interactiveTask.error}`);
    }
    if (interactiveTask.mediaFilesSeen === 0 && interactiveTask.itemsSeen === 0) {
      throw new Error("interactive folder scan did not report any current-folder work");
    }

    const priorityStartedAt = Date.now();
    const [selectedInitial, visibleInitial, aheadInitial] = await Promise.all([
      primeThumbnailScope([priorityPlan.selected], "selected"),
      primeThumbnailScope(priorityPlan.visibleScope, "visible"),
      primeThumbnailScope(priorityPlan.aheadScope, "ahead")
    ]);

    const [selectedClear, visibleClear, aheadClear] = await Promise.all([
      settleThumbnailScope("selected", [priorityPlan.selected], "selected", priorityStartedAt, selectedInitial),
      settleThumbnailScope("visible", priorityPlan.visibleScope, "visible", priorityStartedAt, visibleInitial),
      settleThumbnailScope("ahead", priorityPlan.aheadScope, "ahead", priorityStartedAt, aheadInitial)
    ]);

    if (selectedClear.ready + selectedClear.skipped !== 1) {
      throw new Error(`selected thumbnail did not settle cleanly: ${JSON.stringify(selectedClear)}`);
    }
    if (visibleClear.ready + visibleClear.skipped === 0) {
      throw new Error(`visible scope did not produce usable thumbnails: ${JSON.stringify(visibleClear)}`);
    }
    if (aheadClear.ready + aheadClear.skipped === 0) {
      throw new Error(`ahead scope did not produce usable thumbnails: ${JSON.stringify(aheadClear)}`);
    }
    if (visibleClear.pendingOrQueuedAtStart === 0) {
      throw new Error("visible scope did not expose a pending/queued placeholder phase");
    }

    const backgroundBeforeRequest = await listMediaPage(
      root.rootId,
      priorityPlan.backgroundFolder.folder.id,
      Math.max(PRIORITY_SCOPE_PAGE_LIMIT, SWITCH_SAMPLE_LIMIT)
    );
    const backgroundReadyBeforeRequest = backgroundBeforeRequest.items.filter(
      (item) => item.thumbnailState === "ready"
    ).length;
    if (backgroundReadyBeforeRequest > 0) {
      throw new Error(
        `background folder ${priorityPlan.backgroundFolder.folder.name} already had ` +
          `${backgroundReadyBeforeRequest} ready thumbnails before being requested`
      );
    }

    const backgroundStartedAt = Date.now();
    const backgroundInitial = await primeThumbnailScope([backgroundSample], "background");
    const backgroundClear = await settleThumbnailScope(
      "background",
      [backgroundSample],
      "background",
      backgroundStartedAt,
      backgroundInitial
    );
    if (backgroundClear.ready + backgroundClear.skipped !== 1) {
      throw new Error(`background request did not settle cleanly: ${JSON.stringify(backgroundClear)}`);
    }

    log(
      `selected clear: first=${formatMs(selectedClear.firstTerminalElapsedMs)} settled=${formatMs(selectedClear.settledElapsedMs)} ` +
        `(pending=${selectedClear.pendingOrQueuedAtStart}/${selectedClear.itemCount}, ready=${selectedClear.ready}, skipped=${selectedClear.skipped})`
    );
    log(
      `visible scope clear: first=${formatMs(visibleClear.firstTerminalElapsedMs)} settled=${formatMs(visibleClear.settledElapsedMs)} ` +
        `(pending=${visibleClear.pendingOrQueuedAtStart}/${visibleClear.itemCount}, ready=${visibleClear.ready}, skipped=${visibleClear.skipped}, failed=${visibleClear.failed})`
    );
    log(
      `ahead scope clear: first=${formatMs(aheadClear.firstTerminalElapsedMs)} settled=${formatMs(aheadClear.settledElapsedMs)} ` +
        `(pending=${aheadClear.pendingOrQueuedAtStart}/${aheadClear.itemCount}, ` +
        `ready=${aheadClear.ready}, skipped=${aheadClear.skipped}, failed=${aheadClear.failed})`
    );
    log(
      `background first clear after explicit request: ${formatMs(backgroundClear.firstTerminalElapsedMs)} ` +
        `(ready=${backgroundClear.ready}, skipped=${backgroundClear.skipped}, failed=${backgroundClear.failed})`
    );
    log(
      `interactive folder scan current folder: ${priorityPlan.currentFolder.folder.name} ` +
        `-> task ${interactiveTask.id} succeeded in ${formatMs(interactiveTaskElapsedMs)} ` +
        `(items=${interactiveTask.itemsSeen}, media=${interactiveTask.mediaFilesSeen})`
    );

    const previewResult = await verifyOriginalPreview(priorityPlan.selected);
    log(
      `center preview original: file=${priorityPlan.selected.name}, content-type=${previewResult.contentType ?? "null"}, ` +
        `size=${priorityPlan.selected.size}, loaded headers in ${formatMs(previewResult.elapsedMs)}`
    );

    const priorityOrder = {
      selectedFirstClearBeforeVisibleFirst:
        selectedClear.firstTerminalElapsedMs <= visibleClear.firstTerminalElapsedMs,
      visibleFirstClearBeforeAheadFirst:
        visibleClear.firstTerminalElapsedMs <= aheadClear.firstTerminalElapsedMs,
      backgroundFolderStayedDeferred: backgroundReadyBeforeRequest === 0,
      interactiveFolderScanCovered:
        interactiveTask.folderId === priorityPlan.currentFolder.folder.id &&
        interactiveTask.mediaFilesSeen > 0
    };

    if (!priorityOrder.selectedFirstClearBeforeVisibleFirst) {
      throw new Error("selected first clear did not happen before visible first clear");
    }
    if (!priorityOrder.visibleFirstClearBeforeAheadFirst) {
      throw new Error("visible first clear did not happen before ahead first clear");
    }
    if (!priorityOrder.backgroundFolderStayedDeferred) {
      throw new Error("unrequested background folder reached ready before explicit request");
    }
    if (!priorityOrder.interactiveFolderScanCovered) {
      throw new Error("interactive folder scan behavior was not covered");
    }

    const scanTask = await pollTask(root.taskId, "scan", SCAN_TIMEOUT_MS);
    log(
      `scan finished: status=${scanTask.status} folders=${scanTask.foldersSeen} ` +
        `media=${scanTask.mediaFilesSeen} error=${scanTask.error ?? "none"}`
    );
    if (scanTask.status !== "succeeded") {
      throw new Error(`scan failed: ${scanTask.error}`);
    }

    const indexedMedia = await listAllMedia(root.rootId);
    log(`listMedia returned ${indexedMedia.length} indexed items after scan`);
    if (indexedMedia.length !== scanTask.mediaFilesSeen) {
      throw new Error(`indexed media ${indexedMedia.length} did not match scan media count ${scanTask.mediaFilesSeen}`);
    }
    if (expectedFiles !== indexedMedia.length) {
      throw new Error(`indexed media ${indexedMedia.length} did not match on-disk media count ${expectedFiles}`);
    }

    log("");
    log("=== SUCCESS ===");
    log(`Root scanned: ${ROOT_PATH}`);
    log(`Add root -> first visible media: ${formatMs(firstVisible.elapsedMs)}`);
    log(`Folder switch timings: ${switchTimings.map(formatMs).join(", ")}`);
    log(
      `Foreground first clears: selected=${formatMs(selectedClear.firstTerminalElapsedMs)}, ` +
        `visible=${formatMs(visibleClear.firstTerminalElapsedMs)}, ahead=${formatMs(aheadClear.firstTerminalElapsedMs)}`
    );
    log(`Selected clear settled: ${formatMs(selectedClear.settledElapsedMs)}`);
    log(`Visible scope clear settled: ${formatMs(visibleClear.settledElapsedMs)} for ${visibleClear.itemCount} items`);
    log(`Ahead scope clear settled: ${formatMs(aheadClear.settledElapsedMs)} for ${aheadClear.itemCount} items`);
    log(`Background first clear after request: ${formatMs(backgroundClear.firstTerminalElapsedMs)}`);
    log(`Center preview original header load: ${formatMs(previewResult.elapsedMs)}`);
    log(`Media indexed: ${indexedMedia.length} (on disk: ${expectedFiles})`);
    log(`Data dir: ${dataDir}`);
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

const entrypoint =
  process.env.MEGLE_REAL_LOAD_TEST_LIFECYCLE_SELF_TEST === "1" ? runLifecycleSelfTest : main;

entrypoint().catch((error) => {
  // eslint-disable-next-line no-console
  console.error("[test] FAILED:", error);
  process.exit(1);
});

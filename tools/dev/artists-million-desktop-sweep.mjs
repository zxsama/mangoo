import { spawn } from "node:child_process";
import { createWriteStream, statSync } from "node:fs";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import WebSocket from "ws";

const root = "D:\\Megle";
const visualRoot = path.join(root, ".tmp", "visual-check");
const logDir = path.join(visualRoot, "logs");
const dataDir =
  process.env.MEGLE_ARTISTS_SWEEP_DATA_DIR ?? path.join(visualRoot, "data-artists-million");
const dbPath = path.join(dataDir, "megle.sqlite");
const electronUserDataDir = path.join(dataDir, "electron-user-data");
const startDesktopCommand = path.join(root, "start-desktop.cmd");
const autoRoot = process.env.MEGLE_ARTISTS_SWEEP_ROOT ?? "Y:\\Repository\\Billfish\\Artists";
const webUrl = process.env.MEGLE_ARTISTS_SWEEP_WEB_URL ?? "http://127.0.0.1:5181";
let debugPort = Number(process.env.MEGLE_ARTISTS_SWEEP_DEBUG_PORT ?? 9251);
const targetOperationCount = Number(process.env.MEGLE_ARTISTS_SWEEP_OPERATION_COUNT ?? 500);
const resetData = process.env.MEGLE_ARTISTS_SWEEP_RESET_DB !== "0";
const failOnSlow = process.env.MEGLE_ARTISTS_SWEEP_FAIL_ON_SLOW === "1";
const allowPartialCoverage = process.env.MEGLE_ARTISTS_SWEEP_ALLOW_PARTIAL_COVERAGE === "1";
const thumbnailCacheMode = process.env.MEGLE_ARTISTS_SWEEP_THUMBNAIL_CACHE_MODE ?? "none";
const thumbnailCacheBatchLimit = Number(
  process.env.MEGLE_ARTISTS_SWEEP_THUMBNAIL_CACHE_LIMIT ?? 256
);
const waitForBulkIdleBeforeSweep = process.env.MEGLE_ARTISTS_SWEEP_WAIT_FOR_BULK_IDLE_BEFORE_SWEEP === "1";
const waitForThumbnailTaskDrainAfterSweep =
  process.env.MEGLE_ARTISTS_SWEEP_WAIT_FOR_THUMBNAIL_TASK_DRAIN_AFTER_SWEEP === "1";
const warmSeenFileIdsPath = process.env.MEGLE_ARTISTS_SWEEP_WARM_FILE_IDS_PATH ?? "";
const FETCH_REPORT_LIMIT = 20;
const TILE_READY_SELECTOR = ".tile-thumb-ready, .tile-thumb-image, .tile-thumb-failed, img";
const MEDIA_TILE_SELECTOR = '.media-tile[role="button"][data-media-id]';
const INDEXED_MEDIA_PAGE_LIMIT = 200;
const DEV_APP_STOP_TIMEOUT_MS = 10_000;
const REQUIRED_LAYOUT_CLASSES = [
  "virtual-grid--adaptive",
  "virtual-grid--waterfall",
  "virtual-grid--grid",
  "virtual-grid--list"
];

const stdoutPath = path.join(logDir, "artists-million-desktop-sweep.stdout.log");
const stderrPath = path.join(logDir, "artists-million-desktop-sweep.stderr.log");
const summaryPath = path.join(logDir, "artists-million-desktop-sweep-summary.json");
const stdout = createWriteStream(stdoutPath, { flags: "w" });
const stderr = createWriteStream(stderrPath, { flags: "w" });

const operationRecords = [];
const failures = [];
const consoleErrors = [];
const networkProblems = [];
const startupOutput = [];
const indexedMediaFolderIdsByRoot = new Map();
const folderAncestorIdsByFolder = new Map();
let thumbnailCacheStatsBefore = null;
let thumbnailCacheStatsAfter = null;
let child;

if (resetData) {
  await rm(dataDir, { recursive: true, force: true });
}
await mkdir(logDir, { recursive: true });
await mkdir(dataDir, { recursive: true });
await mkdir(electronUserDataDir, { recursive: true });

function appendOutput(source, chunk) {
  const text = chunk.toString();
  startupOutput.push({ source, text });
  if (source === "stderr") stderr.write(text);
  else stdout.write(text);
}

function fileStats(filePath) {
  try {
    const stats = statSync(filePath);
    return {
      path: filePath,
      bytes: stats.size,
      mtimeUtc: stats.mtime.toISOString()
    };
  } catch (error) {
    if (error?.code === "ENOENT") {
      return {
        path: filePath,
        bytes: 0,
        missing: true
      };
    }
    return {
      path: filePath,
      error: String(error?.message ?? error)
    };
  }
}

function collectDatabaseFileStats() {
  return {
    sqlite: fileStats(dbPath),
    wal: fileStats(`${dbPath}-wal`),
    shm: fileStats(`${dbPath}-shm`)
  };
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function startDevApp() {
  child = spawn("cmd.exe", ["/d", "/c", startDesktopCommand], {
    cwd: root,
    env: {
      ...process.env,
      MEGLE_WEB_URL: webUrl,
      MEGLE_DB_PATH: dbPath,
      MEGLE_ELECTRON_USER_DATA_DIR: electronUserDataDir,
      MEGLE_AUTO_ADD_ROOT: autoRoot,
      MEGLE_REMOTE_DEBUG: "1",
      MEGLE_REMOTE_DEBUG_PORT: String(debugPort)
    },
    shell: process.platform === "win32",
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true
  });

  child.stdout.on("data", (chunk) => appendOutput("stdout", chunk));
  child.stderr.on("data", (chunk) => appendOutput("stderr", chunk));
}

async function stopDevApp() {
  if (!child || child.killed || child.pid === undefined) return;

  if (process.platform === "win32") {
    await new Promise((resolve) => {
      const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
        stdio: "ignore",
        windowsHide: true
      });
      let settled = false;
      const finish = () => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        resolve();
      };
      const timeout = setTimeout(() => {
        killer.kill();
        finish();
      }, DEV_APP_STOP_TIMEOUT_MS);
      killer.on("error", finish);
      killer.on("exit", finish);
    });
    return;
  }

  child.kill("SIGTERM");
  await delay(2000);
  if (!child.killed) child.kill("SIGKILL");
}

async function httpJson(url) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        body += chunk;
      });
      response.on("end", () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    request.on("error", reject);
    request.setTimeout(1000, () => request.destroy(new Error(`Timed out fetching ${url}`)));
  });
}

async function waitForUrl(url, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
      lastError = new Error(`${url} returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }
  throw new Error(`Timed out waiting for ${url}: ${String(lastError)}`);
}

function canBindPort(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.unref();
    server.once("error", () => {
      resolve(false);
    });
    server.listen({ host: "127.0.0.1", port }, () => {
      server.close(() => resolve(true));
    });
  });
}

async function resolveDebugPort(startPort, maxAttempts = 2000) {
  for (let candidate = startPort; candidate < startPort + maxAttempts; candidate += 1) {
    if (await canBindPort(candidate)) {
      return candidate;
    }
  }
  throw new Error(`Unable to find a bindable debug port starting at ${startPort}`);
}

async function waitForThumbnailCacheIdle(client, timeoutMs = 10 * 60_000) {
  const deadline = Date.now() + timeoutMs;
  let lastStats = null;
  while (Date.now() < deadline) {
    lastStats = await desktopApiJson(client, "/thumbnails/cache/stats");
    if (
      (lastStats.pendingCandidateCount ?? 0) === 0 &&
      (lastStats.activeBulkTaskCount ?? 0) === 0
    ) {
      return lastStats;
    }
    await delay(1_000);
  }
  throw new Error(
    `Timed out waiting for thumbnail cache to go idle: ${JSON.stringify(lastStats)}`
  );
}

async function waitForThumbnailTaskDrain(client, timeoutMs = 10 * 60_000) {
  const deadline = Date.now() + timeoutMs;
  let lastActiveTasks = null;
  while (Date.now() < deadline) {
    const tasksPage = await desktopApiJson(client, "/tasks");
    const tasks = Array.isArray(tasksPage?.items) ? tasksPage.items : [];
    const activeThumbnailTasks = tasks.filter(
      (task) =>
        task?.kind === "thumbnail" &&
        (task?.status === "pending" || task?.status === "running")
    );
    lastActiveTasks = activeThumbnailTasks.map((task) => ({
      id: task.id,
      priority: task.priority,
      status: task.status,
      fileId: task.fileId
    }));
    if (activeThumbnailTasks.length === 0) {
      return lastActiveTasks;
    }
    await delay(1_000);
  }
  throw new Error(
    `Timed out waiting for thumbnail tasks to drain: ${JSON.stringify(lastActiveTasks)}`
  );
}

function isTerminalThumbnailState(state) {
  return state === "ready" || state === "failed" || state === "skipped_small";
}

function takeRotatingWarmSeenBatch(pendingFileIds, cursor, batchSize) {
  const fileIds = Array.from(pendingFileIds);
  if (fileIds.length === 0) {
    return { batch: [], cursor: 0 };
  }
  const batch = [];
  const start = cursor % fileIds.length;
  const count = Math.min(batchSize, fileIds.length);
  for (let index = 0; index < count; index += 1) {
    batch.push(fileIds[(start + index) % fileIds.length]);
  }
  return { batch, cursor: cursor + count };
}

function addWarmSeenDiagnosticGroup(groups, groupName, fileId, detail = null) {
  const group = groups.get(groupName) ?? { count: 0, ids: [], details: [] };
  group.count += 1;
  if (group.ids.length < 8) {
    group.ids.push(fileId);
  }
  if (detail && group.details.length < 3 && !group.details.includes(detail)) {
    group.details.push(detail);
  }
  groups.set(groupName, group);
}

function compactErrorMessage(error) {
  return String(error?.message ?? error).replace(/\s+/g, " ").slice(0, 180);
}

async function summarizeWarmSeenRemaining(client, pendingFileIds) {
  const fileIds = Array.from(pendingFileIds);
  const groups = new Map();
  for (let offset = 0; offset < fileIds.length; offset += 32) {
    const batch = fileIds.slice(offset, offset + 32);
    const responses = await Promise.all(
      batch.map((fileId) =>
        desktopApiJson(client, `/media/${fileId}/thumbnail?target=grid_320&priority=background`)
          .then((thumbnail) => ({ fileId, thumbnail }))
          .catch((error) => ({ fileId, error }))
      )
    );
    for (const result of responses) {
      if ("error" in result) {
        addWarmSeenDiagnosticGroup(
          groups,
          "request_error",
          result.fileId,
          compactErrorMessage(result.error)
        );
        continue;
      }
      addWarmSeenDiagnosticGroup(
        groups,
        result.thumbnail?.state ?? "unknown",
        result.fileId,
        result.thumbnail?.error ? String(result.thumbnail.error).slice(0, 180) : null
      );
    }
  }
  return Object.fromEntries(
    [...groups.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([state, group]) => {
        if (group.details.length === 0) {
          return [state, { count: group.count, ids: group.ids }];
        }
        return [state, group];
      })
  );
}

async function prepareThumbnailCacheScenario(client) {
  thumbnailCacheStatsBefore = await desktopApiJson(client, "/thumbnails/cache/stats");
  if (thumbnailCacheMode === "clear" || thumbnailCacheMode === "bulk") {
    await desktopApiJson(client, "/thumbnails/cache/clear", { method: "POST", body: "" });
  }
  if (thumbnailCacheMode === "bulk") {
    const enqueueOnce = () =>
      desktopApiJson(client, "/tasks/thumbnail-cache", {
        method: "POST",
        body: JSON.stringify({
          refreshMode: "staleOrMissing",
          limit: thumbnailCacheBatchLimit
        })
      });

    if (waitForBulkIdleBeforeSweep) {
      while (true) {
        const response = await enqueueOnce();
        if ((response.acceptedCount ?? 0) === 0) {
          break;
        }
        await delay(50);
      }
      await waitForThumbnailCacheIdle(client);
    } else {
      await enqueueOnce();
    }
  }
  if (thumbnailCacheMode === "warmSeen") {
    const raw = warmSeenFileIdsPath ? await readFile(warmSeenFileIdsPath, "utf8") : "[]";
    const seenFileIds = JSON.parse(raw);
    if (!Array.isArray(seenFileIds)) {
      throw new Error("warm seen file id payload must be an array");
    }
    const pendingFileIds = new Set(
      seenFileIds.filter((fileId) => Number.isInteger(fileId) && fileId > 0)
    );
    for (let offset = 0; offset < seenFileIds.length; offset += 128) {
      const batch = seenFileIds
        .slice(offset, offset + 128)
        .filter((fileId) => Number.isInteger(fileId) && fileId > 0);
      if (batch.length === 0) {
        continue;
      }
      await desktopApiJson(client, "/tasks/thumbnail-cache", {
        method: "POST",
        body: JSON.stringify({
          fileIds: batch,
          refreshMode: "staleOrMissing",
          limit: batch.length
        })
      });
    }
    const deadline = Date.now() + 10 * 60_000;
    let pendingCursor = 0;
    while (pendingFileIds.size > 0) {
      if (Date.now() >= deadline) {
        const remainingStateSummary = await summarizeWarmSeenRemaining(client, pendingFileIds);
        throw new Error(
          `Timed out warming seen thumbnails: remaining=${pendingFileIds.size}; states=${JSON.stringify(
            remainingStateSummary
          )}`
        );
      }
      const rotated = takeRotatingWarmSeenBatch(pendingFileIds, pendingCursor, 32);
      pendingCursor = rotated.cursor;
      const batch = rotated.batch;
      const responses = await Promise.all(
        batch.map((fileId) =>
          desktopApiJson(client, `/media/${fileId}/thumbnail?target=grid_320&priority=background`)
            .then((thumbnail) => ({ fileId, thumbnail }))
            .catch((error) => ({ fileId, error }))
        )
      );
      for (const result of responses) {
        if ("error" in result) {
          continue;
        }
        if (isTerminalThumbnailState(result.thumbnail?.state)) {
          pendingFileIds.delete(result.fileId);
        }
      }
      await delay(250);
    }
  }
}

async function waitForTarget(timeoutMs = 180_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const targets = await httpJson(`http://127.0.0.1:${debugPort}/json/list`);
      const page = targets.find((target) => target.type === "page" && target.url.startsWith(webUrl));
      if (page?.webSocketDebuggerUrl) return page.webSocketDebuggerUrl;
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }
  throw new Error(`Timed out waiting for Electron CDP target: ${String(lastError)}`);
}

class CdpClient {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.requestUrls = new Map();
    this.ready = new Promise((resolve, reject) => {
      this.ws.once("open", resolve);
      this.ws.once("error", reject);
    });
    this.ws.on("message", (raw) => this.#handleMessage(raw));
  }

  #handleMessage(raw) {
    const message = JSON.parse(raw.toString());
    if (message.id) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(`${pending.method}: ${JSON.stringify(message.error)}`));
      else pending.resolve(message.result ?? {});
      return;
    }
    if (message.method === "Network.requestWillBeSent") {
      this.requestUrls.set(message.params.requestId, message.params.request?.url ?? null);
    } else if (message.method === "Network.loadingFinished") {
      this.requestUrls.delete(message.params.requestId);
    } else if (message.method === "Runtime.exceptionThrown") {
      consoleErrors.push(message.params?.exceptionDetails?.exception?.description ?? JSON.stringify(message.params));
    } else if (message.method === "Log.entryAdded") {
      const entry = message.params?.entry;
      if (entry?.level === "error") consoleErrors.push(entry.text);
    } else if (message.method === "Network.loadingFailed") {
      networkProblems.push({
        ...message.params,
        url: this.requestUrls.get(message.params.requestId) ?? null
      });
      this.requestUrls.delete(message.params.requestId);
    }
  }

  send(method, params = {}, timeoutMs = 30_000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.has(id)) return;
        this.pending.delete(id);
        reject(new Error(`CDP timeout: ${method}`));
      }, timeoutMs);
      timer.unref?.();
      this.pending.set(id, {
        method,
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        }
      });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.ws.close();
  }
}

async function evaluate(client, expression, timeoutMs = 30_000) {
  const result = await client.send(
    "Runtime.evaluate",
    {
      expression,
      awaitPromise: true,
      returnByValue: true
    },
    timeoutMs
  );
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result?.value;
}

async function desktopApiJson(client, pathname, init = {}) {
  const payload = JSON.stringify({
    pathname,
    method: init.method ?? "GET",
    headers: init.headers ?? {},
    body: init.body ?? null
  });
  return evaluate(
    client,
    `(() => {
      const input = ${payload};
      const bridge = window.megleDesktop;
      if (!bridge?.coreUrl || !bridge?.sessionToken) {
        throw new Error("desktop bridge core/session not ready");
      }
      const url = new URL(input.pathname.replace(/^\\//, ""), bridge.coreUrl.endsWith("/") ? bridge.coreUrl : bridge.coreUrl + "/").toString();
      return fetch(url, {
        method: input.method,
        headers: {
          "content-type": "application/json",
          "x-megle-session": bridge.sessionToken,
          ...input.headers
        },
        body: input.body
      }).then(async (response) => {
        const text = await response.text();
        if (!response.ok) {
          throw new Error(url + " returned " + response.status + ": " + text);
        }
        return text ? JSON.parse(text) : null;
      });
    })()`,
    60_000
  );
}

async function waitFor(client, expression, timeoutMs = 60_000, label = expression) {
  const deadline = Date.now() + timeoutMs;
  let lastValue;
  while (Date.now() < deadline) {
    lastValue = await evaluate(client, expression);
    if (lastValue) return lastValue;
    await delay(250);
  }
  throw new Error(`Timed out waiting for ${label}; last=${JSON.stringify(lastValue)}`);
}

async function installInstrumentation(client) {
  await evaluate(
    client,
    `(() => {
      if (
        window.__artistsSweep?.installed &&
        typeof window.__artistsSweepStart === "function" &&
        typeof window.__artistsSweepFinish === "function"
      ) {
        return true;
      }
      const state = {
        installed: true,
        nextFetchId: 1,
        opStart: null,
        fetches: [],
        longTasks: []
      };
      window.__artistsSweep = state;
      const originalFetch = window.fetch.bind(window);
      window.fetch = async (...args) => {
        const id = state.nextFetchId++;
        const startedAt = performance.now();
        const url = typeof args[0] === "string" ? args[0] : args[0]?.url ?? "";
        const record = {
          id,
          url,
          status: null,
          ok: null,
          pending: true,
          startedAt,
          completedAt: null,
          elapsedMs: 0,
          opLabelAtStart: state.opStart?.label ?? null
        };
        state.fetches.push(record);
        try {
          const response = await originalFetch(...args);
          const completedAt = performance.now();
          record.status = response.status;
          record.ok = response.ok;
          record.pending = false;
          record.completedAt = completedAt;
          record.elapsedMs = completedAt - startedAt;
          return response;
        } catch (error) {
          const completedAt = performance.now();
          record.status = 0;
          record.ok = false;
          record.pending = false;
          record.completedAt = completedAt;
          record.elapsedMs = completedAt - startedAt;
          record.error = String(error);
          throw error;
        }
      };
      try {
        const observer = new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) {
            state.longTasks.push({
              name: entry.name,
              duration: entry.duration,
              startTime: entry.startTime,
              at: performance.now()
            });
          }
        });
        observer.observe({ entryTypes: ["longtask"] });
        state.longTaskObserver = observer;
      } catch {
        state.longTaskObserver = null;
      }
      window.__artistsSweepStart = (label) => {
        state.opStart = {
          label,
          startedAt: performance.now(),
          longTaskIndex: state.longTasks.length
        };
        return state.opStart;
      };
      window.__artistsSweepFinish = () => {
        const opStart = state.opStart ?? {
          label: "unknown",
          startedAt: performance.now(),
          longTaskIndex: state.longTasks.length
        };
        const now = performance.now();
        const fetches = state.fetches
          .filter((item) => item.startedAt >= opStart.startedAt)
          .map((item) => ({
            ...item,
            elapsedMs: item.pending ? now - item.startedAt : item.elapsedMs
          }));
        const staleFetches = state.fetches
          .filter(
            (item) =>
              item.startedAt < opStart.startedAt &&
              item.completedAt !== null &&
              item.completedAt >= opStart.startedAt
          )
          .map((item) => ({ ...item }));
        const stalePendingFetches = state.fetches
          .filter((item) => item.startedAt < opStart.startedAt && item.pending)
          .map((item) => ({
            ...item,
            elapsedMs: now - item.startedAt
          }));
        const longTasks = state.longTasks.slice(opStart.longTaskIndex);
        const grid = document.querySelector(".virtual-grid");
        const visibleTiles = [...document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)})].filter((tile) => {
          const rect = tile.getBoundingClientRect();
          return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
        });
        const tileReadySelector = ${JSON.stringify(TILE_READY_SELECTOR)};
        const visibleReadyThumbs = visibleTiles.filter((tile) => tile.querySelector(tileReadySelector));
        const visibleNotReadyThumbDetails = visibleTiles
          .filter((tile) => !tile.querySelector(tileReadySelector))
          .slice(0, 40)
          .map((tile) => ({
            mediaId: tile.getAttribute("data-media-id"),
            thumbState: tile.getAttribute("data-thumb-state"),
            label: tile.getAttribute("aria-label"),
            classes: tile.className,
            hasLoading: Boolean(tile.querySelector(".tile-thumb-loading")),
            hasPlaceholder: Boolean(tile.querySelector("[data-preview-placeholder]")),
            imageCount: tile.querySelectorAll("img").length
          }));
        const visibleFolderCards = [...document.querySelectorAll(".subfolder-card")].filter((folder) => {
          const rect = folder.getBoundingClientRect();
          return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
        });
        const visibleReadyFolderCovers = visibleFolderCards.filter((folder) =>
          folder.querySelector('[data-cover-status="ready"], [data-cover-status="empty"], [data-cover-status="failed"]')
        );
        const selectedTile = document.querySelector(".media-tile.selected");
        const visibleMediaIds = visibleTiles
          .map((tile) => Number(tile.getAttribute("data-media-id")))
          .filter((value) => Number.isInteger(value) && value > 0);
        const previewReady = Boolean(document.querySelector(".central-preview-stage .preview-placeholder.ready, .central-preview-stage img, .central-preview-stage video"));
        const tree = document.querySelector(".tree-list");
        const selectedTreeItem = document.querySelector(".tree-item.selected");
        const selectedTreeLabel = selectedTreeItem?.querySelector(".tree-label")?.textContent?.trim() ?? null;
        const recursiveToggle = document.querySelector(".library-browser-content-toggle input");
        const visibleTreeItems = [...document.querySelectorAll(".tree-item")].filter((item) => {
          const rect = item.getBoundingClientRect();
          return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
        });
        return {
          browserElapsedMs: performance.now() - opStart.startedAt,
          longTasks,
          fetches,
          staleFetches,
          stalePendingFetches,
          dom: {
            treeItems: document.querySelectorAll(".tree-item").length,
            subfolderCards: document.querySelectorAll(".subfolder-card").length,
            mediaTiles: document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)}).length,
            visibleTiles: visibleTiles.length,
            visibleMediaIds,
            visibleReadyThumbs: visibleReadyThumbs.length,
            visibleNotReadyThumbDetails,
            visibleFolderCovers: visibleFolderCards.length,
            visibleReadyFolderCovers: visibleReadyFolderCovers.length,
            loadingFolderCovers: visibleFolderCards.filter((folder) => folder.querySelector('[data-cover-status="loading"]')).length,
            readyThumbs: document.querySelectorAll(tileReadySelector).length,
            loadingThumbs: document.querySelectorAll(".tile-thumb-loading").length,
            selectedReady: Boolean(selectedTile?.querySelector(tileReadySelector)),
            selectedMediaId: selectedTile?.getAttribute("data-media-id") ?? null,
            selectedThumbState: selectedTile?.getAttribute("data-thumb-state") ?? null,
            previewOpen: Boolean(document.querySelector(".central-preview-stage")),
            previewReady,
            layoutClass: [...(grid?.classList ?? [])].find((name) => name.startsWith("virtual-grid--")) ?? null,
            scrollTop: grid?.scrollTop ?? 0,
            scrollHeight: grid?.scrollHeight ?? 0,
            clientHeight: grid?.clientHeight ?? 0,
            treeVisibleItems: visibleTreeItems.length,
            treeScrollTop: tree?.scrollTop ?? 0,
            treeScrollHeight: tree?.scrollHeight ?? 0,
            treeClientHeight: tree?.clientHeight ?? 0,
            selectedTreeLabel,
            selectedRootId: selectedTreeItem?.getAttribute("data-root-id") ?? null,
            selectedFolderId: selectedTreeItem?.getAttribute("data-folder-id") ?? null,
            selectedDepth: selectedTreeItem?.getAttribute("data-depth") ?? null,
            recursiveChildContents: recursiveToggle instanceof HTMLInputElement ? recursiveToggle.checked : null,
            errorText: document.querySelector(".error-strip")?.textContent ?? null
          }
        };
      };
      return true;
    })()`
  );
}

function summarizeFetches(fetches) {
  const media = fetches.filter((item) => item.url.includes("/api/media?"));
  const folder = fetches.filter((item) => item.url.includes("/api/folders"));
  const thumbnail = fetches.filter((item) => item.url.includes("/thumbnail"));
  const preview = fetches.filter((item) => item.url.includes("/preview"));
  const pending = fetches.filter((item) => item.pending);
  const failed = fetches
    .filter((item) => item.ok === false)
    .sort((left, right) => right.elapsedMs - left.elapsedMs);
  const pendingRequests = [...pending].sort((left, right) => right.elapsedMs - left.elapsedMs);
  return {
    count: fetches.length,
    pending: pending.length,
    mediaPageMs: maxElapsed(media),
    folderChildrenMs: maxElapsed(folder),
    thumbnailMs: maxElapsed(thumbnail),
    previewMs: maxElapsed(preview),
    failedTotal: failed.length,
    failed: failed.slice(0, FETCH_REPORT_LIMIT).map((item) => ({
      url: item.url,
      status: item.status,
      elapsedMs: Math.round(item.elapsedMs)
    })),
    pendingRequestsTotal: pendingRequests.length,
    pendingRequests: pendingRequests.slice(0, FETCH_REPORT_LIMIT).map((item) => ({
      url: item.url,
      elapsedMs: Math.round(item.elapsedMs)
    }))
  };
}

function maxElapsed(items) {
  return items.length > 0 ? Math.round(Math.max(...items.map((item) => item.elapsedMs))) : 0;
}

function isExpectedCanceledNetworkProblem(problem) {
  return problem.errorText === "net::ERR_ABORTED" && problem.canceled === true;
}

function operationThreshold(label) {
  if (/preview open|deep scroll|scrollbar drag|root recursive/i.test(label)) return 3500;
  if (/folder|layout|search clear|close preview|toggle recursive|tree directory/i.test(label)) return 2500;
  return 3000;
}

function optionalGridScrollActionResult(scrollResult) {
  if (scrollResult?.ok !== false) {
    return { ok: true, noScrollableContent: false };
  }
  if (scrollResult?.skipped === true && scrollResult?.reason === "not scrollable") {
    return { ok: true, noScrollableContent: true };
  }
  return { ok: false, noScrollableContent: false };
}

async function runOperation(client, label, action, options = {}) {
  await installInstrumentation(client);
  const index = operationRecords.length + 1;
  await evaluate(client, `window.__artistsSweepStart(${JSON.stringify(label)})`);
  const startedAt = Date.now();
  let actionResult = null;
  let ok = true;
  let error = null;
  try {
    actionResult = await action();
    if (options.requireAction && (actionResult?.ok === false || actionResult?.skipped === true)) {
      ok = false;
      error = actionResult.reason ?? JSON.stringify(actionResult);
    }
    const shouldWaitForMedia = actionResult?.ok !== false && actionResult?.skipped !== true;
    if (options.waitVisible && shouldWaitForMedia) {
      actionResult = {
        ...(actionResult ?? {}),
        visibleReady: await waitForVisibleReady(client, options.visibleTimeoutMs ?? 3500)
      };
    }
    if (options.waitSelected && shouldWaitForMedia) {
      actionResult = {
        ...(actionResult ?? {}),
        selectedReady: await waitForSelectedReady(
          client,
          options.selectedTimeoutMs ?? 3500,
          actionResult?.clicked?.mediaId ?? actionResult?.mediaId ?? null
        )
      };
    }
    if (options.waitPreview && shouldWaitForMedia) {
      actionResult = {
        ...(actionResult ?? {}),
        previewReady: await waitForPreviewReady(client, options.previewTimeoutMs ?? 5000)
      };
    }
  } catch (cause) {
    ok = false;
    error = String(cause);
  }

  const metrics = await evaluate(client, "window.__artistsSweepFinish()");
  const elapsedMs = Date.now() - startedAt;
  const longTaskDurationMs = Math.round(metrics.longTasks.reduce((sum, item) => sum + item.duration, 0));
  const thresholdMs = operationThreshold(label);
  const slow = elapsedMs > thresholdMs || longTaskDurationMs > 1000;
  const api = summarizeFetches(metrics.fetches);
  actionResult = reconcileVisibleReadyWithFinalDom(actionResult, metrics.dom);
  const record = {
    index,
    label,
    ok,
    slow,
    thresholdMs,
    elapsedMs,
    browserElapsedMs: Math.round(metrics.browserElapsedMs),
    longTaskCount: metrics.longTasks.length,
    longTaskDurationMs,
    dom: metrics.dom,
    api,
    staleApi: summarizeFetches(metrics.stalePendingFetches ?? []),
    requireAction: Boolean(options.requireAction),
    actionResult,
    error
  };
  operationRecords.push(record);
  if (!ok) failures.push(record);
  console.log(
    `[artists-sweep] ${String(index).padStart(3, "0")} ${ok ? "OK" : "FAIL"} ${slow ? "SLOW" : "    "} ${elapsedMs}ms ${label}`
  );
  if (!ok || index % 10 === 0) {
    await writePartialSummary();
  }
  return record;
}

function reconcileVisibleReadyWithFinalDom(actionResult, dom) {
  if (!actionResult?.visibleReady || actionResult.visibleReady.complete !== false || !dom) {
    return actionResult;
  }
  const finalTotal = (dom.visibleTiles ?? 0) + (dom.visibleFolderCovers ?? 0);
  const finalReady = (dom.visibleReadyThumbs ?? 0) + (dom.visibleReadyFolderCovers ?? 0);
  if (finalTotal <= 0 || finalReady < finalTotal) {
    return actionResult;
  }
  return {
    ...actionResult,
    visibleReady: {
      ...actionResult.visibleReady,
      total: finalTotal,
      ready: finalReady,
      loading: 0,
      notReadyTiles: [],
      folders: dom.visibleFolderCovers ?? actionResult.visibleReady.folders,
      readyFolders: dom.visibleReadyFolderCovers ?? actionResult.visibleReady.readyFolders,
      loadingFolders: 0,
      complete: true,
      reconciledFromFinalDom: true
    }
  };
}

async function writePartialSummary() {
  try {
    await writeFile(summaryPath, JSON.stringify(buildSummary(failures.length === 0), null, 2));
  } catch (error) {
    console.warn(`[artists-sweep] failed to write partial summary: ${String(error)}`);
  }
}

async function waitForVisibleReady(client, timeoutMs) {
  const startedAt = Date.now();
  let last = null;
  let emptySince = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await evaluate(client, visibleReadyExpression());
    if (last.total === 0 && last.pendingMediaPageRequests === 0) {
      emptySince ??= Date.now();
    } else {
      emptySince = null;
    }
    if (
      (emptySince !== null && Date.now() - emptySince >= 800) ||
      (last.total > 0 && last.ready >= last.total)
    ) {
      return { ...last, elapsedMs: Date.now() - startedAt, complete: true };
    }
    await delay(120);
  }
  return { ...(last ?? { total: 0, ready: 0 }), elapsedMs: Date.now() - startedAt, complete: false };
}

async function waitForSelectedReady(client, timeoutMs, expectedMediaId = null) {
  const startedAt = Date.now();
  let status = null;
  while (Date.now() - startedAt < timeoutMs) {
    status = await selectedReadyStatus(client, expectedMediaId);
    if (status.ready) return { ...status, elapsedMs: Date.now() - startedAt };
    await delay(120);
  }
  return { ...(status ?? { ready: false }), elapsedMs: Date.now() - startedAt };
}

async function selectedReadyStatus(client, expectedMediaId = null) {
  return await evaluate(
    client,
    `(() => {
      const tileReadySelector = ${JSON.stringify(TILE_READY_SELECTOR)};
      const selectedTile = document.querySelector(".media-tile.selected");
      const expectedMediaId = ${JSON.stringify(expectedMediaId)};
      const actualMediaId = selectedTile?.getAttribute("data-media-id") ?? null;
      const matchesExpected = expectedMediaId === null || actualMediaId === String(expectedMediaId);
      const ready = Boolean(selectedTile?.querySelector(tileReadySelector)) && matchesExpected;
      return {
        ready,
        hasSelectedTile: Boolean(selectedTile),
        expectedMediaId,
        actualMediaId,
        matchesExpected,
        thumbState: selectedTile?.getAttribute("data-thumb-state") ?? null,
        hasReady: Boolean(selectedTile?.querySelector(".tile-thumb-ready")),
        hasImage: Boolean(selectedTile?.querySelector("img")),
        hasFailed: Boolean(selectedTile?.querySelector(".tile-thumb-failed")),
        hasLoading: Boolean(selectedTile?.querySelector(".tile-thumb-loading")),
        hasPlaceholder: Boolean(selectedTile?.querySelector("[data-preview-placeholder]")),
        classes: selectedTile?.className ?? null
      };
    })()`
  );
}

async function waitForPreviewReady(client, timeoutMs) {
  const startedAt = Date.now();
  let ready = false;
  while (Date.now() - startedAt < timeoutMs) {
    ready = await evaluate(
      client,
      `Boolean(document.querySelector(".central-preview-stage .preview-placeholder.ready, .central-preview-stage img, .central-preview-stage video"))`
    );
    if (ready) return { ready, elapsedMs: Date.now() - startedAt };
    await delay(120);
  }
  return { ready, elapsedMs: Date.now() - startedAt };
}

function visibleReadyExpression() {
  return `(() => {
    const now = performance.now();
    const tileReadySelector = ${JSON.stringify(TILE_READY_SELECTOR)};
    const pendingMediaPageFetches = (window.__artistsSweep?.fetches ?? []).filter((item) => item.pending && item.url.includes("/api/media?"));
    const grid = document.querySelector(".virtual-grid");
    const tiles = [...document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)})].filter((tile) => {
      const rect = tile.getBoundingClientRect();
      return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
    });
    const folders = [...document.querySelectorAll(".subfolder-card")].filter((folder) => {
      const rect = folder.getBoundingClientRect();
      return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
    });
    const readyTiles = tiles.filter((tile) => tile.querySelector(tileReadySelector));
    const loadingTiles = tiles.filter((tile) => tile.querySelector(".tile-thumb-loading"));
    const notReadyTiles = tiles
      .filter((tile) => !tile.querySelector(tileReadySelector))
      .slice(0, 40)
      .map((tile) => ({
        mediaId: tile.getAttribute("data-media-id"),
        thumbState: tile.getAttribute("data-thumb-state"),
        label: tile.getAttribute("aria-label"),
        classes: tile.className,
        hasLoading: Boolean(tile.querySelector(".tile-thumb-loading")),
        hasPlaceholder: Boolean(tile.querySelector("[data-preview-placeholder]")),
        imageCount: tile.querySelectorAll("img").length
      }));
    const readyFolderCovers = folders.filter((folder) =>
      folder.querySelector('[data-cover-status="ready"], [data-cover-status="empty"], [data-cover-status="failed"]')
    );
    const loadingFolderCovers = folders.filter((folder) =>
      folder.querySelector('[data-cover-status="loading"]')
    );
    const blankScrollableViewport =
      tiles.length === 0 &&
      folders.length === 0 &&
      grid instanceof HTMLElement &&
      grid.scrollTop > 0 &&
      grid.scrollHeight > grid.clientHeight + 32;
    return {
      total: blankScrollableViewport ? 1 : tiles.length + folders.length,
      ready: blankScrollableViewport
        ? 0
        : readyTiles.length + readyFolderCovers.length,
      loading: blankScrollableViewport
        ? 1
        : loadingTiles.length + loadingFolderCovers.length,
      notReadyTiles,
      folders: folders.length,
      readyFolders: readyFolderCovers.length,
      loadingFolders: loadingFolderCovers.length,
      blankScrollableViewport,
      pendingMediaPageRequests: pendingMediaPageFetches.length,
      oldestPendingMediaPageMs: pendingMediaPageFetches.length > 0
        ? Math.round(Math.max(...pendingMediaPageFetches.map((item) => now - item.startedAt)))
        : 0
    };
  })()`;
}

async function invokeSelector(client, selector) {
  const outcome = await evaluate(
    client,
    `(() => {
      const node = document.querySelector(${JSON.stringify(selector)});
      if (!(node instanceof HTMLElement)) return false;
      node.click();
      return true;
    })()`
  );
  if (!outcome) throw new Error(`Selector not found: ${selector}`);
  await delay(120);
}

async function invokeEnabledSelector(client, selector) {
  const outcome = await evaluate(
    client,
    `(() => {
      const node = document.querySelector(${JSON.stringify(selector)});
      if (!(node instanceof HTMLButtonElement)) return { ok: false, reason: "not found" };
      if (node.disabled) return { ok: false, skipped: true, reason: "disabled" };
      node.click();
      return { ok: true };
    })()`
  );
  if (outcome?.skipped) return outcome;
  if (!outcome?.ok) throw new Error(`Enabled selector not found: ${selector}; got=${JSON.stringify(outcome)}`);
  await delay(120);
  return outcome;
}

async function invokeContains(client, selector, text) {
  const outcome = await evaluate(
    client,
    `(() => {
      const nodes = [...document.querySelectorAll(${JSON.stringify(selector)})];
      const node = nodes.find((candidate) => (candidate.textContent || "").includes(${JSON.stringify(text)}));
      if (!(node instanceof HTMLElement)) return { ok: false, options: nodes.slice(0, 20).map((candidate) => (candidate.textContent || "").trim()) };
      node.click();
      return { ok: true, text: (node.textContent || "").trim() };
    })()`
  );
  if (!outcome?.ok) throw new Error(`Target not found: ${text}; got=${JSON.stringify(outcome)}`);
  await delay(160);
}

async function pressKey(client, key) {
  await client.send("Input.dispatchKeyEvent", { type: "keyDown", key });
  await client.send("Input.dispatchKeyEvent", { type: "keyUp", key });
  await delay(120);
}

async function setSearch(client, value) {
  await evaluate(
    client,
    `(() => {
      const input = document.querySelector(".search-bar-input");
      if (!(input instanceof HTMLInputElement)) return false;
      input.focus();
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(input, ${JSON.stringify(value)});
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    })()`
  );
  await delay(250);
}

async function scrollGrid(client, amount) {
  await evaluate(
    client,
    `(() => {
      const grid = document.querySelector(".virtual-grid");
      if (!(grid instanceof HTMLElement)) return false;
      grid.scrollTop = Math.max(0, grid.scrollTop + ${amount});
      grid.dispatchEvent(new Event("scroll", { bubbles: true }));
      return true;
    })()`
  );
  await delay(250);
}

async function scrollGridToRatio(client, ratio) {
  const result = await evaluate(
    client,
    `(() => {
      const grid = document.querySelector(".virtual-grid");
      if (!(grid instanceof HTMLElement)) return { ok: false, reason: "no grid" };
      const maxTop = Math.max(0, grid.scrollHeight - grid.clientHeight);
      grid.scrollTop = Math.round(maxTop * ${ratio});
      grid.dispatchEvent(new Event("scroll", { bubbles: true }));
      return { ok: true, scrollTop: grid.scrollTop, maxTop };
    })()`
  );
  await delay(250);
  return result;
}

async function visibleMediaStatus(client) {
  return await evaluate(
    client,
    `(() => {
      const now = performance.now();
      const pendingMediaPageFetches = (window.__artistsSweep?.fetches ?? []).filter((item) => item.pending && item.url.includes("/api/media?"));
      const visibleTiles = [...document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)})].filter((tile) => {
        const rect = tile.getBoundingClientRect();
        return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
      });
      const visibleReady = visibleTiles.filter((tile) => tile.querySelector(".tile-thumb-ready, .tile-thumb-image, .tile-thumb-failed, img"));
      const grid = document.querySelector(".virtual-grid");
      return {
        visibleMediaTiles: visibleTiles.length,
        visibleReadyThumbs: visibleReady.length,
        mediaTiles: document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)}).length,
        subfolderCards: document.querySelectorAll(".subfolder-card").length,
        scrollTop: grid instanceof HTMLElement ? grid.scrollTop : 0,
        scrollHeight: grid instanceof HTMLElement ? grid.scrollHeight : 0,
        clientHeight: grid instanceof HTMLElement ? grid.clientHeight : 0,
        pendingMediaPageRequests: pendingMediaPageFetches.length,
        oldestPendingMediaPageMs: pendingMediaPageFetches.length > 0
          ? Math.round(Math.max(...pendingMediaPageFetches.map((item) => now - item.startedAt)))
          : 0
      };
    })()`
  );
}

async function waitForAnyVisibleMedia(client, timeoutMs = 2500) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await visibleMediaStatus(client);
    if (last.visibleMediaTiles > 0) {
      return { ok: true, elapsedMs: Date.now() - startedAt, ...last };
    }
    await delay(120);
  }
  return { ok: false, reason: "no visible media tiles", elapsedMs: Date.now() - startedAt, ...(last ?? {}) };
}

async function scrollUntilVisibleMedia(client, seed, options = {}) {
  const ratios = [0, 0.08, 0.16, 0.28, 0.42, 0.58, 0.74, 0.9];
  const maxAttempts = Math.min(ratios.length, options.maxAttempts ?? ratios.length);
  const waitTimeoutMs = options.timeoutMs ?? 650;
  const attempts = [];
  for (let index = 0; index < maxAttempts; index += 1) {
    const ratio = ratios[(seed + index) % ratios.length];
    const scroll = await scrollGridToRatio(client, ratio);
    const visible = await waitForAnyVisibleMedia(client, waitTimeoutMs);
    attempts.push({ ratio, scroll, visible });
    if (visible.ok) return { ok: true, attempts };
  }
  return { ok: false, reason: "no visible media after scroll probes", attempts };
}

async function ensureVisibleMediaTiles(client, seed) {
  await ensureLibrary(client);
  let status = await waitForAnyVisibleMedia(client, 600);
  if (status.ok) return { ok: true, source: "current", status };

  const recursiveOff = await setRecursiveChildContents(client, false);
  status = await waitForAnyVisibleMedia(client, 600);
  if (status.ok) return { ok: true, source: "current-recursive-off", recursiveOff, status };

  const rootResult = await selectArtistsRoot(client);
  if (!rootResult?.ok || !rootResult.rootId || !rootResult.rootFolderId) {
    return {
      ok: false,
      skipped: true,
      reason: "Artists root is unavailable for indexed media selection",
      rootResult
    };
  }
  const indexedFolder = await selectIndexedMediaFolder(client, rootResult, seed);
  if (indexedFolder.ok) {
    return { ok: true, source: "indexed-media-folder", recursiveOff, rootResult, indexedFolder };
  }

  return {
    ok: false,
    skipped: true,
    reason: "unable to select an indexed folder containing media",
    recursiveOff,
    rootResult,
    indexedFolder,
    lastStatus: await visibleMediaStatus(client)
  };
}

async function loadIndexedMediaFolderIds(client, rootId, rootFolderId) {
  const cacheKey = String(rootId);
  const cached = indexedMediaFolderIdsByRoot.get(cacheKey);
  if (cached?.length) return cached;

  const query = new URLSearchParams({
    rootId: String(rootId),
    folderId: String(rootFolderId),
    includeDescendants: "true",
    limit: String(INDEXED_MEDIA_PAGE_LIMIT),
    offset: "0",
    sort: "mtime_desc"
  });
  const page = await desktopApiJson(client, `/media?${query}`);
  const folderIds = [
    ...new Set(
      (Array.isArray(page?.items) ? page.items : [])
        .map((item) => Number(item?.folderId))
        .filter((folderId) => Number.isInteger(folderId) && folderId > 0)
    )
  ];
  if (folderIds.length) indexedMediaFolderIdsByRoot.set(cacheKey, folderIds);
  return folderIds;
}

async function loadFolderAncestorIds(client, folderId) {
  const cacheKey = String(folderId);
  const cached = folderAncestorIdsByFolder.get(cacheKey);
  if (cached?.length) return cached;

  const page = await desktopApiJson(client, `/folders/${folderId}/ancestors`);
  const ancestorIds = (Array.isArray(page?.items) ? page.items : [])
    .map((item) => Number(item?.id))
    .filter((ancestorId) => Number.isInteger(ancestorId) && ancestorId > 0);
  if (ancestorIds.length) folderAncestorIdsByFolder.set(cacheKey, ancestorIds);
  return ancestorIds;
}

async function waitForTreeFolderItem(client, folderId, timeoutMs = 2500) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await evaluate(
      client,
      `(() => {
        const item = document.querySelector('.tree-item[data-folder-id="${folderId}"]');
        if (!(item instanceof HTMLElement)) return { ok: false, reason: "not rendered" };
        const label = item.querySelector(".tree-label");
        return {
          ok: label instanceof HTMLElement,
          reason: label instanceof HTMLElement ? null : "label missing",
          text: label?.textContent?.trim() ?? "",
          expanded: item.getAttribute("aria-expanded") === "true"
        };
      })()`
    );
    if (last?.ok) return last;
    await delay(100);
  }
  return { ...(last ?? {}), ok: false, folderId, reason: last?.reason ?? "not rendered" };
}

async function expandAndSelectTreeFolder(client, folderId) {
  const ancestorIds = await loadFolderAncestorIds(client, folderId);
  if (!ancestorIds.length || ancestorIds.at(-1) !== folderId) {
    return { ok: false, reason: "invalid folder ancestor path", folderId, ancestorIds };
  }

  const steps = [];
  for (let index = 0; index < ancestorIds.length; index += 1) {
    const ancestorId = ancestorIds[index];
    const rendered = await waitForTreeFolderItem(client, ancestorId);
    if (!rendered.ok) {
      return { ok: false, reason: "folder path is not rendered", folderId, ancestorIds, steps, rendered };
    }
    const step = await evaluate(
      client,
      `(() => {
        const item = document.querySelector('.tree-item[data-folder-id="${ancestorId}"]');
        const label = item?.querySelector(".tree-label");
        if (!(item instanceof HTMLElement) || !(label instanceof HTMLElement)) {
          return { ok: false, reason: "tree folder disappeared" };
        }
        item.scrollIntoView({ block: "nearest", inline: "nearest" });
        const shouldExpand = ${index < ancestorIds.length - 1};
        const disclosure = item.querySelector(".tree-disclosure");
        const expanded = item.getAttribute("aria-expanded") === "true";
        if (shouldExpand && !expanded && disclosure instanceof HTMLButtonElement && !disclosure.disabled) {
          disclosure.click();
        }
        if (!shouldExpand) label.click();
        return {
          ok: true,
          folderId: ${ancestorId},
          text: label.textContent?.trim() ?? "",
          expanded,
          clickedDisclosure: shouldExpand && !expanded,
          selected: !shouldExpand
        };
      })()`
    );
    steps.push(step);
    if (!step?.ok) return { ok: false, reason: step?.reason ?? "tree path step failed", folderId, ancestorIds, steps };
    if (step.clickedDisclosure) await delay(180);
  }

  const selected = await waitFor(
    client,
    `document.querySelector('.tree-item.selected')?.getAttribute('data-folder-id') === ${JSON.stringify(String(folderId))}`,
    4_000,
    `indexed media folder ${folderId} selected`
  );
  return { ok: Boolean(selected), folderId, ancestorIds, steps };
}

async function selectIndexedMediaFolder(client, rootResult, seed) {
  const folderIds = await loadIndexedMediaFolderIds(
    client,
    Number(rootResult.rootId),
    Number(rootResult.rootFolderId)
  );
  if (!folderIds.length) {
    return { ok: false, reason: "Core returned no indexed media folders", folderIds };
  }

  const attempts = [];
  const maxAttempts = Math.min(folderIds.length, 4);
  for (let index = 0; index < maxAttempts; index += 1) {
    const folderId = folderIds[(seed + index) % folderIds.length];
    try {
      const selected = await expandAndSelectTreeFolder(client, folderId);
      const visible = selected.ok
        ? await waitForIndexedFolderVisibleMedia(client)
        : { ok: false, reason: selected.reason };
      attempts.push({ folderId, selected, visible });
      if (visible.ok) return { ok: true, folderId, folderIds, attempts };
    } catch (error) {
      attempts.push({ folderId, error: String(error) });
    }
  }
  return { ok: false, reason: "indexed media folders did not produce visible media", folderIds, attempts };
}

async function waitForIndexedFolderVisibleMedia(client) {
  const attempts = [];
  let visible = await waitForAnyVisibleMedia(client, 2500);
  attempts.push({ stage: "initial", visible });
  if (visible.ok) return { ...visible, attempts };

  if (visible.mediaTiles > 0) {
    const scroll = await scrollGridToRatio(client, 1);
    visible = await waitForAnyVisibleMedia(client, 2500);
    attempts.push({ stage: "existing-media-bottom", scroll, visible });
    if (visible.ok) return { ...visible, attempts };
  }

  if (visible.pendingMediaPageRequests > 0) {
    visible = await waitForAnyVisibleMedia(client, 4500);
    attempts.push({ stage: "media-page-drain", visible });
    if (visible.ok) return { ...visible, attempts };
    if (visible.mediaTiles > 0) {
      const scroll = await scrollGridToRatio(client, 1);
      visible = await waitForAnyVisibleMedia(client, 2500);
      attempts.push({ stage: "loaded-media-bottom", scroll, visible });
      if (visible.ok) return { ...visible, attempts };
    }
  }

  return { ...visible, ok: false, attempts };
}

async function scrollTree(client, amount) {
  const result = await evaluate(
    client,
    `(() => {
      const tree = document.querySelector(".tree-list");
      if (!(tree instanceof HTMLElement)) return { ok: false, reason: "no tree-list" };
      const before = tree.scrollTop;
      tree.scrollTop = Math.max(0, Math.min(tree.scrollHeight - tree.clientHeight, tree.scrollTop + ${amount}));
      tree.dispatchEvent(new Event("scroll", { bubbles: true }));
      return {
        ok: true,
        before,
        after: tree.scrollTop,
        maxTop: Math.max(0, tree.scrollHeight - tree.clientHeight),
        visible: [...document.querySelectorAll(".tree-item")].filter((node) => {
          const rect = node.getBoundingClientRect();
          return rect.bottom > 0 && rect.top < window.innerHeight;
        }).length
      };
    })()`
  );
  await delay(180);
  return result;
}

async function dragOverlayScrollbar(client, scrollableSelector, dragRatio = 0.62) {
  const setup = await evaluate(
    client,
    `(() => {
      const element = document.querySelector(${JSON.stringify(scrollableSelector)});
      if (!(element instanceof HTMLElement)) return { ok: false, reason: "missing scrollable" };
      const rect = element.getBoundingClientRect();
      if (element.scrollHeight <= element.clientHeight + 1) {
        return { ok: false, skipped: true, reason: "not scrollable", before: element.scrollTop };
      }
      const x = Math.max(rect.left + 8, rect.right - 8);
      const y = Math.min(rect.bottom - 18, Math.max(rect.top + 18, rect.top + rect.height * 0.45));
      element.dispatchEvent(new PointerEvent("pointerover", { bubbles: true, clientX: x, clientY: y }));
      document.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, clientX: x, clientY: y }));
      return {
        ok: true,
        x,
        y,
        before: element.scrollTop,
        rectHeight: rect.height,
        maxTop: Math.max(0, element.scrollHeight - element.clientHeight)
      };
    })()`
  );
  if (!setup?.ok) return setup;

  await client.send("Input.dispatchMouseEvent", {
    type: "mouseMoved",
    x: setup.x,
    y: setup.y,
    button: "none"
  });
  await delay(120);

  const thumb = await evaluate(
    client,
    `(() => {
      const thumb = document.querySelector('.megle-overlay-scrollbar-vertical[data-scrollbar-visible="true"] .megle-overlay-scrollbar-thumb');
      if (!(thumb instanceof HTMLElement)) return null;
      const rect = thumb.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, height: rect.height };
    })()`
  );

  if (!thumb) {
    return await evaluate(
      client,
      `(() => {
        const element = document.querySelector(${JSON.stringify(scrollableSelector)});
        if (!(element instanceof HTMLElement)) return { ok: false, reason: "missing fallback scrollable" };
        const before = element.scrollTop;
        const maxTop = Math.max(0, element.scrollHeight - element.clientHeight);
        element.scrollTop = Math.min(maxTop, before + Math.max(320, element.clientHeight * ${dragRatio}));
        element.dispatchEvent(new Event("scroll", { bubbles: true }));
        return { ok: true, fallback: true, before, after: element.scrollTop, maxTop };
      })()`
    );
  }

  const dragDelta = Math.max(80, Math.round(setup.rectHeight * dragRatio));
  await client.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: thumb.x,
    y: thumb.y,
    button: "left",
    clickCount: 1
  });
  for (let step = 1; step <= 6; step += 1) {
    await client.send("Input.dispatchMouseEvent", {
      type: "mouseMoved",
      x: thumb.x,
      y: thumb.y + (dragDelta * step) / 6,
      button: "left",
      buttons: 1
    });
    await delay(35);
  }
  await client.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: thumb.x,
    y: thumb.y + dragDelta,
    button: "left",
    clickCount: 1
  });
  await delay(250);

  return await evaluate(
    client,
    `(() => {
      const element = document.querySelector(${JSON.stringify(scrollableSelector)});
      if (!(element instanceof HTMLElement)) return { ok: false, reason: "missing after scrollable" };
      return {
        ok: true,
        before: ${setup.before},
        after: element.scrollTop,
        maxTop: Math.max(0, element.scrollHeight - element.clientHeight)
      };
    })()`
  );
}

async function clickTreeFolder(client, offset) {
  return await evaluate(
    client,
    `(() => {
      const labels = [...document.querySelectorAll(".tree-item .tree-label")]
        .filter((node) => {
          const text = (node.textContent || "").trim();
          const rect = node.getBoundingClientRect();
          return text && rect.width > 0 && rect.height > 0;
        });
      if (labels.length === 0) return { ok: false, reason: "no tree labels" };
      const node = labels[${offset} % labels.length];
      node.click();
      return { ok: true, text: (node.textContent || "").trim(), count: labels.length };
    })()`
  );
}

async function selectArtistsRoot(client) {
  const rootSelection = await evaluate(
    client,
    `(() => {
      const rootItems = [...document.querySelectorAll('.tree-list > .tree-branch > .tree-item[data-depth="0"][data-root-id]')]
        .filter((node) => node instanceof HTMLElement);
      const rootItem =
        rootItems.find((node) => (node.querySelector(".tree-label span")?.textContent || "").trim() === "Artists") ??
        rootItems.find((node) => (node.querySelector(".tree-label span")?.textContent || "").includes("Artists")) ??
        rootItems[0] ??
        null;
      const rootLabel = rootItem?.querySelector(".tree-label") ?? null;
      if (!(rootItem instanceof HTMLElement) || !(rootLabel instanceof HTMLElement)) {
        return { ok: false, reason: "no top-level root label" };
      }
      rootItem.scrollIntoView({ block: "nearest", inline: "nearest" });
      const rootName = (rootLabel.querySelector("span")?.textContent || rootLabel.textContent || "").trim();
      rootLabel.click();
      const disclosure = rootItem.querySelector(".tree-disclosure");
      if (
        disclosure instanceof HTMLButtonElement &&
        !disclosure.disabled &&
        rootItem.getAttribute("aria-expanded") !== "true"
      ) {
        disclosure.click();
      }
      return {
        ok: true,
        text: (rootLabel.textContent || "").trim(),
        rootId: rootItem.getAttribute("data-root-id"),
        rootFolderId: rootItem.getAttribute("data-folder-id") ?? null,
        rootName,
        rootIndex: rootItems.indexOf(rootItem),
        rootCount: rootItems.length
      };
    })()`
  );
  if (rootSelection?.ok) {
    let lastSelection = null;
    const deadline = Date.now() + 6_500;
    while (Date.now() < deadline) {
      lastSelection = await evaluate(
        client,
        `(() => {
          const rootId = ${JSON.stringify(rootSelection.rootId)};
          const rootFolderId = ${JSON.stringify(rootSelection.rootFolderId)};
          const rootItems = [...document.querySelectorAll('.tree-list > .tree-branch > .tree-item[data-depth="0"][data-root-id]')]
            .filter((node) => node instanceof HTMLElement);
          const rootItem = rootItems.find((node) =>
            node.getAttribute("data-root-id") === rootId &&
            (rootFolderId === null || node.getAttribute("data-folder-id") === rootFolderId)
          ) ?? null;
          const rootLabel = rootItem?.querySelector(".tree-label") ?? null;
          const selectedItem = document.querySelector(".tree-list > .tree-branch > .tree-item.selected");
          const selected =
            selectedItem instanceof HTMLElement &&
            selectedItem.getAttribute("data-depth") === "0" &&
            selectedItem.getAttribute("data-root-id") === rootId &&
            (rootFolderId === null || selectedItem.getAttribute("data-folder-id") === rootFolderId);
          if (!selected && rootLabel instanceof HTMLElement) {
            rootItem?.scrollIntoView({ block: "nearest", inline: "nearest" });
            rootLabel.click();
          }
          const disclosure = rootItem?.querySelector(".tree-disclosure");
          if (
            disclosure instanceof HTMLButtonElement &&
            !disclosure.disabled &&
            rootItem?.getAttribute("aria-expanded") !== "true"
          ) {
            disclosure.click();
          }
          return {
            selected,
            selectedLabel: selectedItem?.querySelector(".tree-label")?.textContent?.trim() ?? null,
            selectedRootId: selectedItem?.getAttribute("data-root-id") ?? null,
            selectedFolderId: selectedItem?.getAttribute("data-folder-id") ?? null,
            selectedDepth: selectedItem?.getAttribute("data-depth") ?? null,
            rootText: rootLabel?.textContent?.trim() ?? null
          };
        })()`
      );
      if (lastSelection?.selected) break;
      await delay(180);
    }
    if (!lastSelection?.selected) {
      throw new Error(
        `Timed out waiting for Artists top-level root selected; last=${JSON.stringify(lastSelection)}`
      );
    }
    await delay(350);
    return { ...rootSelection, lastSelection };
  }
  await delay(350);
  return rootSelection;
}

async function waitForArtistsRootSubfolders(client, timeoutMs = 5_000) {
  return await waitFor(
    client,
    `(() => {
      const selectedRoot = document.querySelector('.tree-list > .tree-branch > .tree-item.selected[data-depth="0"]');
      return Boolean(selectedRoot) && document.querySelectorAll('.subfolder-card').length > 0;
    })()`,
    timeoutMs,
    "Artists root selected with subfolder cards"
  );
}

async function ensureArtistsRootSubfolderContext(client) {
  await ensureLibrary(client);
  const rootResult = await selectArtistsRoot(client);
  const initialTopScroll = await scrollGridToRatio(client, 0);
  let lastContext = null;
  const deadline = Date.now() + 12_000;
  while (Date.now() < deadline) {
    lastContext = await evaluate(
      client,
      `(() => {
        const rootId = ${JSON.stringify(rootResult.rootId)};
        const rootFolderId = ${JSON.stringify(rootResult.rootFolderId)};
        const selectedItem = document.querySelector(".tree-item.selected");
        const rootItems = [...document.querySelectorAll('.tree-list > .tree-branch > .tree-item[data-depth="0"][data-root-id]')]
          .filter((node) => node instanceof HTMLElement);
        const rootItem = rootItems.find((node) =>
          node.getAttribute("data-root-id") === rootId &&
          (rootFolderId === null || node.getAttribute("data-folder-id") === rootFolderId)
        ) ?? null;
        const rootLabel = rootItem?.querySelector(".tree-label") ?? null;
        const selected =
          selectedItem instanceof HTMLElement &&
          selectedItem.getAttribute("data-depth") === "0" &&
          selectedItem.getAttribute("data-root-id") === rootId &&
          (rootFolderId === null || selectedItem.getAttribute("data-folder-id") === rootFolderId);
        if (!selected && rootLabel instanceof HTMLElement) {
          rootItem?.scrollIntoView({ block: "nearest", inline: "nearest" });
          rootLabel.click();
        }
        const grid = document.querySelector(".virtual-grid");
        if (grid instanceof HTMLElement && grid.scrollTop !== 0) {
          grid.scrollTop = 0;
          grid.dispatchEvent(new Event("scroll", { bubbles: true }));
        }
        const strip = document.querySelector(".subfolder-strip");
        return {
          ready: selected && Boolean(strip),
          selected,
          hasStrip: Boolean(strip),
          hasGrid: grid instanceof HTMLElement,
          scrollTop: grid instanceof HTMLElement ? grid.scrollTop : null,
          selectedRootId: selectedItem?.getAttribute("data-root-id") ?? null,
          selectedFolderId: selectedItem?.getAttribute("data-folder-id") ?? null,
          selectedDepth: selectedItem?.getAttribute("data-depth") ?? null
        };
      })()`
    );
    if (lastContext?.ready) {
      break;
    }
    await delay(250);
  }
  if (!lastContext?.ready) {
    throw new Error(`Timed out waiting for Artists root subfolder context; last=${JSON.stringify(lastContext)}`);
  }
  const expanded = await evaluate(
    client,
    `(() => {
      const strip = document.querySelector(".subfolder-strip");
      const button = document.querySelector(".subfolder-strip-heading-button");
      if (strip?.classList.contains("is-collapsed") && button instanceof HTMLElement) {
        button.click();
        return { clicked: true };
      }
      return { clicked: false };
    })()`
  );
  if (expanded?.clicked) {
    await delay(250);
  }
  await waitForArtistsRootSubfolders(client, 12_000);
  return { rootResult, topScroll: initialTopScroll, context: lastContext, expanded };
}

async function expandVisibleTreeBranch(client, offset) {
  const result = await evaluate(
    client,
    `(() => {
      const items = [...document.querySelectorAll(".tree-item")]
        .filter((item) => {
          const rect = item.getBoundingClientRect();
          const button = item.querySelector(".tree-disclosure");
          return button instanceof HTMLButtonElement && !button.disabled && rect.bottom > 0 && rect.top < window.innerHeight;
        });
      if (items.length === 0) return { ok: false, skipped: true, reason: "no expandable visible tree item" };
      const item = items[${offset} % items.length];
      const button = item.querySelector(".tree-disclosure");
      const wasExpanded = item.getAttribute("aria-expanded") === "true";
      if (button instanceof HTMLElement && !wasExpanded) {
        button.click();
      }
      return {
        ok: true,
        wasExpanded,
        text: item.querySelector(".tree-label")?.textContent?.trim() ?? "",
        count: items.length
      };
    })()`
  );
  await delay(350);
  return result;
}

async function clickDistantTreeFolder(client, offset, scrollAmount) {
  const scroll = await scrollTree(client, scrollAmount);
  const expanded = await expandVisibleTreeBranch(client, offset);
  const clicked = await clickTreeFolder(client, offset * 3 + 1);
  await delay(450);
  return { ok: clicked?.ok !== false, scroll, expanded, clicked };
}

async function clickSubfolderCard(client, offset) {
  return await evaluate(
    client,
    `(() => {
      const cards = [...document.querySelectorAll(".subfolder-card")].filter((node) => {
        const rect = node.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
      if (cards.length === 0) return { ok: false, reason: "no subfolder cards" };
      const node = cards[${offset} % cards.length];
      const button = node.querySelector(".subfolder-card-main");
      if (!(button instanceof HTMLElement)) return { ok: false, reason: "no subfolder card main button" };
      const name = node.querySelector(".subfolder-card-name")?.textContent?.trim() ?? "";
      button.click();
      return { ok: true, text: name, count: cards.length };
    })()`
  );
}

async function expandSubfolderHierarchy(client, offset) {
  const attemptExpand = async (fallback = false) => await evaluate(
    client,
    `(async () => {
      const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const grid = document.querySelector(".virtual-grid");
      const cards = () => [...document.querySelectorAll(".subfolder-card")].filter((node) => {
        const rect = node.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
      const visibleCards = () => cards().filter((node) => {
        const rect = node.getBoundingClientRect();
        return rect.bottom > 0 && rect.top < window.innerHeight;
      });
      const expandableCards = () => cards().filter((node) => {
        const button = node.querySelector(".subfolder-card-expand");
        return button instanceof HTMLButtonElement &&
          !button.disabled &&
          button.getAttribute("aria-pressed") !== "true";
      });
      const expandedCards = () => cards().filter((node) => {
        const button = node.querySelector(".subfolder-card-expand");
        return button instanceof HTMLButtonElement &&
          !button.disabled &&
          button.getAttribute("aria-pressed") === "true";
      });
      const maxDepth = (nodes) => nodes.reduce((max, node) => {
        const depth = Number(node.getAttribute("data-depth") ?? 0);
        return Number.isFinite(depth) ? Math.max(max, depth) : max;
      }, 0);
      const scrollRatios = [null, 0, 0.24, 0.5, 0.76];
      let probedCards = 0;
      let probedButtons = 0;
      const reexpandFirstExpanded = async () => {
        const expanded = expandedCards();
        if (expanded.length === 0) return null;
        const node = expanded[${offset} % expanded.length];
        const button = node.querySelector(".subfolder-card-expand");
        if (!(button instanceof HTMLButtonElement) || button.disabled) return null;
        const beforeCards = cards();
        const beforeCount = beforeCards.length;
        const beforeDepth = maxDepth(beforeCards);
        const name = node.querySelector(".subfolder-card-name")?.textContent?.trim() ?? "";
        node.scrollIntoView({ block: "center", inline: "nearest" });
        grid?.dispatchEvent(new Event("scroll", { bubbles: true }));
        await delay(80);
        button.click();
        await delay(180);
        button.click();
        await delay(450);
        const afterCards = cards();
        return {
          ok: true,
          fallback: ${fallback ? "true" : "false"},
          reexpanded: true,
          name,
          beforeCount,
          afterCount: afterCards.length,
          beforeDepth,
          afterDepth: maxDepth(afterCards),
          probedCards,
          probedButtons
        };
      };
      if (expandableCards().length === 0) {
        const reexpanded = await reexpandFirstExpanded();
        if (reexpanded) return reexpanded;
      }
      for (const ratio of scrollRatios) {
        if (ratio !== null && grid instanceof HTMLElement) {
          const maxTop = Math.max(0, grid.scrollHeight - grid.clientHeight);
          grid.scrollTop = Math.round(maxTop * ratio);
          grid.dispatchEvent(new Event("scroll", { bubbles: true }));
          await delay(120);
        }
        const candidateCards = expandableCards();
        probedCards = Math.max(probedCards, candidateCards.length);
        if (candidateCards.length === 0) continue;
        const start = ${offset} % candidateCards.length;
        for (let attempt = 0; attempt < Math.min(candidateCards.length, 8); attempt += 1) {
          const beforeCards = cards();
          const candidates = expandableCards();
          if (candidates.length === 0) break;
          const node = candidates[(start + attempt) % candidates.length];
          node.scrollIntoView({ block: "center", inline: "nearest" });
          grid?.dispatchEvent(new Event("scroll", { bubbles: true }));
          await delay(80);
          const button = node?.querySelector(".subfolder-card-expand");
          if (!(button instanceof HTMLButtonElement) || button.disabled) continue;
          probedButtons += 1;
          const beforeCount = beforeCards.length;
          const beforeDepth = maxDepth(beforeCards);
          const name = node.querySelector(".subfolder-card-name")?.textContent?.trim() ?? "";
          const wasExpanded = button.getAttribute("aria-pressed") === "true";
          if (wasExpanded) {
            continue;
          }
          button.click();
          await delay(450);
          const afterCards = cards();
          const afterCount = afterCards.length;
          const afterDepth = maxDepth(afterCards);
          if (afterCount > beforeCount || afterDepth > beforeDepth) {
            return {
              ok: true,
              fallback: ${fallback ? "true" : "false"},
              name,
              beforeCount,
              afterCount,
              beforeDepth,
              afterDepth,
              probedCards,
              probedButtons
            };
          }
        }
      }
      const reexpanded = await reexpandFirstExpanded();
      if (reexpanded) return reexpanded;
      return {
        ok: false,
        fallback: ${fallback ? "true" : "false"},
        reason: cards().length === 0
          ? "no subfolder cards"
          : "no collapsed or expanded subfolder control available",
        count: cards().length,
        maxDepth: maxDepth(cards()),
        probedCards,
        probedButtons
      };
    })()`,
    8_000
  );

  const currentResult = await attemptExpand(false);
  if (currentResult.ok) return currentResult;

  let rootContext = null;
  try {
    rootContext = await ensureArtistsRootSubfolderContext(client);
  } catch (cause) {
    return {
      ok: false,
      reason: String(cause),
      currentResult,
      rootContext
    };
  }
  const toggleResult = await setRecursiveChildContents(client, true);
  const rootContentReady = await waitForArtistsRootSubfolders(client).then(
    () => ({ ok: true }),
    (cause) => ({ ok: false, reason: String(cause) })
  );
  if (!rootContentReady.ok) {
    return {
      ok: false,
      reason: rootContentReady.reason ?? "Artists root subfolder cards were not ready",
      currentResult,
      rootContext,
      toggleResult,
      rootContentReady
    };
  }
  await scrollGridToRatio(client, 0);
  await waitForArtistsRootSubfolders(client).catch(() => undefined);
  const rootResult = await attemptExpand(true);
  if (rootResult.ok) return rootResult;
  return {
    ok: false,
    reason: rootResult.reason,
    currentResult,
    rootContext,
    toggleResult,
    rootContentReady,
    rootResult
  };
}

async function verifyEmptySubfolderExpandButtons(client) {
  return await evaluate(
    client,
    `(() => {
      const cards = [...document.querySelectorAll(".subfolder-card")].filter((node) => {
        const rect = node.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
      const emptyCards = cards.filter((node) => node.getAttribute("data-child-status") === "empty");
      const emptyCardsWithExpand = emptyCards.filter((node) => node.querySelector(".subfolder-card-expand"));
      return {
        ok: emptyCardsWithExpand.length === 0,
        cardCount: cards.length,
        emptyCount: emptyCards.length,
        emptyWithExpandCount: emptyCardsWithExpand.length,
        skipped: emptyCards.length === 0,
        reason: emptyCards.length === 0 ? "no visible loaded empty subfolder cards" : undefined
      };
    })()`
  );
}

async function setRecursiveChildContents(client, checked) {
  await ensureLibrary(client);
  await evaluate(
    client,
    `(() => {
      const grid = document.querySelector(".virtual-grid");
      if (grid instanceof HTMLElement && grid.scrollTop !== 0) {
        grid.scrollTop = 0;
        grid.dispatchEvent(new Event("scroll", { bubbles: true }));
      }
      return true;
    })()`
  );
  await delay(180);
  const result = await evaluate(
    client,
    `(() => {
      const input = document.querySelector(".library-browser-content-toggle input");
      const label = document.querySelector(".library-browser-content-toggle");
      if (!(input instanceof HTMLInputElement) || !(label instanceof HTMLElement)) {
        return { ok: false, skipped: true, reason: "no recursive toggle" };
      }
      const before = input.checked;
      if (before !== ${checked}) {
        label.click();
      }
      return { ok: true, before, requested: ${checked} };
    })()`
  );
  await delay(700);
  return {
    ...result,
    checked: await evaluate(
      client,
      `document.querySelector(".library-browser-content-toggle input")?.checked ?? null`
    )
  };
}

async function clickVisibleTile(client, offset, timeoutMs = 700) {
  const startedAt = Date.now();
  let last = null;
  while (Date.now() - startedAt < timeoutMs) {
    last = await evaluate(
      client,
      `(() => {
        const tiles = [...document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)})].filter((tile) => {
          const rect = tile.getBoundingClientRect();
          return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
        });
        if (tiles.length === 0) return { ok: false, skipped: true, reason: "no visible media tiles" };
        const tile = tiles[${offset} % tiles.length];
        tile.click();
        return {
          ok: true,
          mediaId: tile.getAttribute("data-media-id"),
          label: tile.getAttribute("aria-label") ?? tile.textContent?.trim() ?? "",
          count: tiles.length
        };
      })()`
    );
    if (last?.ok) {
      return { ...last, elapsedMs: Date.now() - startedAt };
    }
    await delay(80);
  }
  return { ...(last ?? { ok: false, skipped: true, reason: "no visible media tiles" }), elapsedMs: Date.now() - startedAt };
}

async function openVisibleTilePreview(client, offset) {
  const result = await evaluate(
    client,
    `(() => {
      const tiles = [...document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)})].filter((tile) => {
        const rect = tile.getBoundingClientRect();
        return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
      });
      if (tiles.length === 0) return { ok: false, skipped: true, reason: "no visible media tiles" };
      const tile = tiles[${offset} % tiles.length];
      tile.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true, button: 0 }));
      return { ok: true, label: tile.getAttribute("aria-label") ?? tile.textContent?.trim() ?? "", count: tiles.length };
    })()`
  );
  if (result?.ok) {
    await waitFor(client, `Boolean(document.querySelector(".central-preview-stage"))`, 10_000, "preview open");
  }
  return result;
}

async function openPreviewFromAvailableMedia(client, seed) {
  const mediaReady = await ensureVisibleMediaTiles(client, seed);
  if (!mediaReady.ok || mediaReady.skipped) return mediaReady;

  await delay(160);
  const firstOpen = await openVisibleTilePreview(client, seed);
  if (firstOpen?.ok !== false) {
    return { ok: true, mediaReady, opened: firstOpen, attempts: [firstOpen] };
  }

  const visibleRetry = await waitForAnyVisibleMedia(client, 1200);
  const secondOpen = visibleRetry.ok ? await openVisibleTilePreview(client, seed + 1) : null;
  if (secondOpen?.ok !== false && secondOpen !== null) {
    return { ok: true, mediaReady, opened: secondOpen, attempts: [firstOpen, secondOpen] };
  }

  return {
    ok: false,
    skipped: true,
    reason: "unable to open preview from indexed visible media",
    mediaReady,
    visibleRetry,
    attempts: [firstOpen, secondOpen].filter(Boolean)
  };
}

async function ensureLibrary(client) {
  if (await evaluate(client, `Boolean(document.querySelector(".central-preview-stage"))`)) {
    await invokeSelector(client, '[aria-label="Back to library"]');
    await waitFor(client, `!document.querySelector(".central-preview-stage")`, 10_000, "preview close");
  }
  if (!(await evaluate(client, `Boolean(document.querySelector(".virtual-grid"))`))) {
    await invokeSelector(client, '[aria-label="Library"]');
    await waitFor(client, `Boolean(document.querySelector(".virtual-grid"))`, 10_000, "library grid");
  }
}

async function setLayout(client, label) {
  await ensureLibrary(client);
  await invokeSelector(client, ".layout-menu-trigger");
  await waitFor(client, `Boolean(document.querySelector(".layout-menu-popover"))`, 10_000, "layout menu");
  await invokeContains(client, ".layout-menu-item", label);
  await waitFor(
    client,
    `Boolean(document.querySelector(".virtual-grid--${label.toLowerCase()}"))`,
    10_000,
    `layout ${label}`
  );
  return { ok: true, label };
}

async function setSort(client, label) {
  await ensureLibrary(client);
  await invokeSelector(client, '[data-compact-popover-trigger="sort"]');
  await waitFor(
    client,
    `Boolean(document.querySelector('[data-compact-popover-root="sort"]'))`,
    10_000,
    "sort menu"
  );
  await invokeContains(client, ".sort-menu-item", label);
  await waitFor(
    client,
    `!document.querySelector('[data-compact-popover-root="sort"]')`,
    10_000,
    `sort ${label} menu close`
  );
  return { ok: true, label };
}

async function closePreview(client) {
  const wasOpen = await evaluate(client, `Boolean(document.querySelector(".central-preview-stage"))`);
  if (!wasOpen) {
    return { ok: false, skipped: true, reason: "preview was not open" };
  }
  await invokeSelector(client, '[aria-label="Back to library"]');
  await waitFor(client, `!document.querySelector(".central-preview-stage")`, 10_000, "preview close");
  return { ok: true };
}

async function runSweep(client) {
  await waitFor(
    client,
    `Boolean(document.querySelector(".app-shell")) && Boolean(document.querySelector(".virtual-grid"))`,
    180_000,
    "desktop shell and grid"
  );

  await runOperation(client, "startup: shell and library grid visible", async () => {
    await waitFor(client, `document.querySelectorAll(".tree-item").length > 0`, 180_000, "tree items");
    return { treeItems: await evaluate(client, `document.querySelectorAll(".tree-item").length`) };
  });

  await runOperation(client, "root initial display: first folders or media", async () => {
    await waitFor(
      client,
      `document.querySelectorAll(".subfolder-card").length > 0 || document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)}).length > 0`,
      300_000,
      "first browsable content"
    );
    return await evaluate(client, `({
      subfolderCards: document.querySelectorAll(".subfolder-card").length,
      mediaTiles: document.querySelectorAll(${JSON.stringify(MEDIA_TILE_SELECTOR)}).length
    })`);
  }, { waitVisible: true, visibleTimeoutMs: 2500 });

  await runOperation(client, "artists root recursive child contents on: initial total directory", async () => {
    await ensureLibrary(client);
    const rootContext = await ensureArtistsRootSubfolderContext(client);
    const toggleResult = await setRecursiveChildContents(client, true);
    return { ok: toggleResult?.ok !== false, rootContext, toggleResult };
  }, { requireAction: true, waitVisible: true, visibleTimeoutMs: 3500 });

  const operationFactories = buildFocusedOperationFactories(client);

  let loopIndex = 0;
  while (operationRecords.length < targetOperationCount) {
    const factory = operationFactories[loopIndex % operationFactories.length];
    await factory(loopIndex);
    loopIndex += 1;
  }
}

function buildFocusedOperationFactories(client) {
  const groups = [];
  const add = (count, factory) => {
    groups.push({ factory, remaining: count });
  };

  add(55, async (i) => runOperation(client, `adaptive cross-tree folder switch ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Adaptive");
    return await clickDistantTreeFolder(client, i, i % 3 === 0 ? 2600 : -1200);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(55, async (i) => runOperation(client, `waterfall cross-tree folder switch ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Waterfall");
    return await clickDistantTreeFolder(client, i + 5, i % 4 === 0 ? 3200 : -1500);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(35, async (i) => runOperation(client, `grid cross-tree folder switch ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Grid");
    return await clickDistantTreeFolder(client, i + 9, i % 3 === 0 ? 3600 : -1800);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(35, async (i) => runOperation(client, `list cross-tree folder switch ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "List");
    return await clickDistantTreeFolder(client, i + 13, i % 3 === 0 ? 4200 : -2100);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(45, async (i) => runOperation(client, `adaptive deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Adaptive");
    return await scrollGridToRatio(client, ((i % 9) + 1) / 10);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(45, async (i) => runOperation(client, `waterfall deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Waterfall");
    return await scrollGridToRatio(client, ((i % 8) + 2) / 10);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(30, async (i) => runOperation(client, `grid deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Grid");
    return await scrollGridToRatio(client, ((i % 7) + 2) / 10);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(30, async (i) => runOperation(client, `list deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "List");
    return await scrollGridToRatio(client, ((i % 6) + 3) / 10);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(35, async (i) => runOperation(client, `adaptive scrollbar drag deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Adaptive");
    return await dragOverlayScrollbar(client, ".virtual-grid", i % 2 === 0 ? 0.66 : 0.42);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(35, async (i) => runOperation(client, `waterfall scrollbar drag deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    await setLayout(client, "Waterfall");
    return await dragOverlayScrollbar(client, ".virtual-grid", i % 2 === 0 ? 0.7 : 0.45);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(45, async (i) => runOperation(client, `recursive child contents deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    const rootContext = await ensureArtistsRootSubfolderContext(client);
    const toggleResult = await setRecursiveChildContents(client, true);
    await setLayout(client, i % 2 === 0 ? "Adaptive" : "Waterfall");
    const scrollResult = i % 3 === 0
      ? await dragOverlayScrollbar(client, ".virtual-grid", 0.7)
      : await scrollGridToRatio(client, ((i % 8) + 2) / 10);
    const scrollAction = optionalGridScrollActionResult(scrollResult);
    return { ok: scrollAction.ok, rootContext, toggleResult, scrollAction, scrollResult };
  }, { requireAction: true, waitVisible: true, visibleTimeoutMs: 3500 }));

  add(35, async (i) => runOperation(client, `artists root recursive total deep scroll ${i}`, async () => {
    await ensureLibrary(client);
    const rootContext = await ensureArtistsRootSubfolderContext(client);
    const toggleResult = await setRecursiveChildContents(client, true);
    await setLayout(client, i % 2 === 0 ? "Adaptive" : "Waterfall");
    const scrollResult = i % 2 === 0
      ? await scrollGridToRatio(client, ((i % 7) + 3) / 10)
      : await dragOverlayScrollbar(client, ".virtual-grid", 0.72);
    const scrollAction = optionalGridScrollActionResult(scrollResult);
    return { ok: scrollAction.ok, rootContext, toggleResult, scrollAction, scrollResult };
  }, { requireAction: true, waitVisible: true, visibleTimeoutMs: 3500 }));

  add(65, async (i) => runOperation(client, `tree directory deep scroll and switch ${i}`, async () => {
    await ensureLibrary(client);
    const scrollAmount = i % 5 === 0 ? -5200 : 1800 + (i % 7) * 700;
    return await clickDistantTreeFolder(client, i + 11, scrollAmount);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(24, async (i) => runOperation(client, `layout switch ${["Adaptive", "Waterfall", "Grid", "List"][i % 4]} ${i}`, async () => {
    await ensureLibrary(client);
    return await setLayout(client, ["Adaptive", "Waterfall", "Grid", "List"][i % 4]);
  }, { waitVisible: true, visibleTimeoutMs: 3000 }));

  add(24, async (i) => {
    const targets = [
      { label: "Name A-Z", menuLabel: "Name A–Z" },
      { label: "Name Z-A", menuLabel: "Name Z–A" },
      { label: "Newest first", menuLabel: "Newest first" }
    ];
    const target = targets[i % targets.length];
    return runOperation(client, `sort ${target.label} ${i}`, async () => {
      await ensureLibrary(client);
      return await setSort(client, target.menuLabel);
    }, { waitVisible: true, visibleTimeoutMs: 3500 });
  });

  add(20, async (i) => runOperation(client, `tree directory scrollbar drag ${i}`, async () => {
    await ensureLibrary(client);
    const dragResult = await dragOverlayScrollbar(client, ".tree-list", i % 2 === 0 ? 0.72 : 0.5);
    const expanded = await expandVisibleTreeBranch(client, i + 17);
    const clicked = await clickTreeFolder(client, i + 23);
    await delay(450);
    return { dragResult, expanded, clicked };
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(35, async (i) => runOperation(client, `select visible media click ${i}`, async () => {
    await ensureLibrary(client);
    const mediaReady = await ensureVisibleMediaTiles(client, i);
    if (!mediaReady.ok || mediaReady.skipped) return mediaReady;
    const clicked = await clickVisibleTile(client, i);
    return { ok: clicked?.ok !== false, mediaReady, clicked };
  }, { requireAction: true, waitSelected: true, selectedTimeoutMs: 3500 }));

  add(30, async (i) => runOperation(client, `preview open clicked media ${i}`, async () => {
    await ensureLibrary(client);
    return await openPreviewFromAvailableMedia(client, i);
  }, { requireAction: true, waitPreview: true, previewTimeoutMs: 5000 }));

  add(18, async (i) => runOperation(client, `preview close ${i}`, async () => {
    await ensureLibrary(client);
    const opened = await openPreviewFromAvailableMedia(client, i);
    if (opened?.ok === false) return opened;
    const closed = await closePreview(client);
    return { ok: closed?.ok !== false, opened, closed };
  }, { requireAction: true, waitVisible: true, visibleTimeoutMs: 3500 }));

  add(18, async (i) => runOperation(client, `preview next after click ${i}`, async () => {
    if (!(await evaluate(client, `Boolean(document.querySelector(".central-preview-stage"))`))) {
      const opened = await openPreviewFromAvailableMedia(client, i);
      if (opened?.ok === false) return opened;
    }
    return await invokeEnabledSelector(client, '[data-titlebar-control="preview-next"]');
  }, { waitPreview: true, previewTimeoutMs: 5000 }));

  add(18, async (i) => runOperation(client, `preview previous after click ${i}`, async () => {
    if (!(await evaluate(client, `Boolean(document.querySelector(".central-preview-stage"))`))) {
      const opened = await openPreviewFromAvailableMedia(client, i);
      if (opened?.ok === false) return opened;
    }
    return await invokeEnabledSelector(client, '[data-titlebar-control="preview-previous"]');
  }, { waitPreview: true, previewTimeoutMs: 5000 }));

  add(15, async (i) => runOperation(client, `toggle recursive child contents ${i}`, async () => {
    await ensureLibrary(client);
    const rootContext = await ensureArtistsRootSubfolderContext(client);
    const toggleResult = await setRecursiveChildContents(client, i % 2 === 0);
    return { ok: toggleResult?.ok !== false, rootContext, toggleResult };
  }, { requireAction: true, waitVisible: true, visibleTimeoutMs: 3500 }));

  add(25, async (i) => runOperation(client, `subfolder hierarchy expand ${i}`, async () => {
    await ensureLibrary(client);
    await ensureArtistsRootSubfolderContext(client);
    await setRecursiveChildContents(client, true);
    await setLayout(client, i % 2 === 0 ? "Adaptive" : "Waterfall");
    return await expandSubfolderHierarchy(client, i);
  }, { requireAction: true, waitVisible: true, visibleTimeoutMs: 3500 }));

  add(10, async (i) => runOperation(client, `empty subfolder has no expand button ${i}`, async () => {
    await ensureLibrary(client);
    await ensureArtistsRootSubfolderContext(client);
    await setRecursiveChildContents(client, true);
    await delay(1200);
    return await verifyEmptySubfolderExpandButtons(client);
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(15, async (i) => runOperation(client, `open subfolder card ${i}`, async () => {
    await ensureLibrary(client);
    const result = await clickSubfolderCard(client, i);
    await delay(500);
    return result;
  }, { waitVisible: true, visibleTimeoutMs: 3500 }));

  add(8, async (i) => runOperation(client, `folder history back ${i}`, async () => {
    await ensureLibrary(client);
    const enabled = await evaluate(client, `!(document.querySelector('[data-titlebar-control="library-folder-back"]')?.disabled ?? true)`);
    if (enabled) await invokeSelector(client, '[data-titlebar-control="library-folder-back"]');
    return { enabled };
  }, { waitVisible: true }));

  add(8, async (i) => runOperation(client, `folder history forward ${i}`, async () => {
    await ensureLibrary(client);
    const enabled = await evaluate(client, `!(document.querySelector('[data-titlebar-control="library-folder-forward"]')?.disabled ?? true)`);
    if (enabled) await invokeSelector(client, '[data-titlebar-control="library-folder-forward"]');
    return { enabled };
  }, { waitVisible: true }));

  add(8, async (i) => runOperation(client, `filter images toggle ${i}`, async () => {
    await ensureLibrary(client);
    await invokeSelector(client, '[data-compact-popover-trigger="filter"]');
    await waitFor(client, `Boolean(document.querySelector('[data-compact-popover-root="filter"]'))`, 10_000, "filter open");
    await invokeContains(client, ".filter-menu-item", "Images");
    await delay(450);
    if (await evaluate(client, `Boolean(document.querySelector('[data-compact-popover-root="filter"]'))`)) {
      await invokeContains(client, ".filter-menu-item", "Images");
    }
    await pressKey(client, "Escape");
  }, { waitVisible: true }));

  add(8, async (i) => runOperation(client, `search set clear ${i}`, async () => {
    await ensureLibrary(client);
    await setSearch(client, i % 2 === 0 ? "png" : "a");
    await setSearch(client, "");
  }, { waitVisible: true, visibleTimeoutMs: 3000 }));

  return interleaveOperationFactories(groups);
}

function interleaveOperationFactories(groups) {
  const factories = [];
  while (groups.some((group) => group.remaining > 0)) {
    for (const group of groups) {
      if (group.remaining <= 0) continue;
      factories.push(group.factory);
      group.remaining -= 1;
    }
  }
  return factories;
}

function percentile(values, p) {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[index];
}

function statsFor(pattern) {
  const values = operationRecords
    .filter((record) => pattern.test(record.label))
    .map((record) => record.elapsedMs);
  return {
    count: values.length,
    p50: percentile(values, 50),
    p95: percentile(values, 95),
    max: values.length > 0 ? Math.max(...values) : 0
  };
}

function collectNestedValues(value, keys, output = []) {
  if (value === null || value === undefined) return output;
  if (Array.isArray(value)) {
    for (const item of value) collectNestedValues(item, keys, output);
    return output;
  }
  if (typeof value !== "object") return output;
  for (const [key, nestedValue] of Object.entries(value)) {
    if (keys.has(key) && typeof nestedValue === "string" && nestedValue.trim()) {
      output.push(nestedValue.trim());
    }
    collectNestedValues(nestedValue, keys, output);
  }
  return output;
}

function hasNestedFlag(value, key) {
  if (value === null || value === undefined) return false;
  if (Array.isArray(value)) return value.some((item) => hasNestedFlag(item, key));
  if (typeof value !== "object") return false;
  if (value[key] === true) return true;
  return Object.values(value).some((item) => hasNestedFlag(item, key));
}

function uniqueSorted(values) {
  return [...new Set(values.filter((value) => typeof value === "string" && value.trim()))].sort();
}

function countBy(values) {
  return values.reduce((counts, value) => {
    if (!value) return counts;
    counts[value] = (counts[value] ?? 0) + 1;
    return counts;
  }, {});
}

function operationFamily(label) {
  return label.replace(/\s+\d+$/, "");
}

function buildCoverage() {
  const selectedFolderKeys = uniqueSorted(
    operationRecords
      .map((record) => {
        const rootId = record.dom?.selectedRootId;
        const folderId = record.dom?.selectedFolderId ?? "root";
        return rootId ? `${rootId}:${folderId}` : null;
      })
      .filter(Boolean)
  );
  const selectedDepths = operationRecords
    .map((record) => Number(record.dom?.selectedDepth))
    .filter((value) => Number.isFinite(value));
  const clickedMediaLabels = uniqueSorted(
    operationRecords
      .filter((record) => /select visible media click|preview open clicked media|preview close/i.test(record.label))
      .flatMap((record) => collectNestedValues(record.actionResult, new Set(["label"])))
  );
  const clickedFolderLabels = uniqueSorted(
    operationRecords
      .filter((record) => /folder|subfolder|tree|history/i.test(record.label))
      .flatMap((record) =>
        collectNestedValues(record.actionResult, new Set(["text", "rootName", "name", "label"]))
      )
  );
  const layoutClasses = countBy(operationRecords.map((record) => record.dom?.layoutClass));
  const operationFamilyCounts = countBy(operationRecords.map((record) => operationFamily(record.label)));
  const skippedOperations = operationRecords.filter((record) => hasNestedFlag(record.actionResult, "skipped"));
  const failedRequiredActions = operationRecords.filter(
    (record) => record.requireAction && record.actionResult?.skipped === true
  );
  return {
    operationFamilyCounts,
    layoutClasses,
    selectedFolderKeys,
    selectedFolderLabels: uniqueSorted(operationRecords.map((record) => record.dom?.selectedTreeLabel).filter(Boolean)),
    clickedFolderLabels,
    clickedMediaLabels,
    maxSelectedDepth: selectedDepths.length > 0 ? Math.max(...selectedDepths) : 0,
    skippedCount: skippedOperations.length,
    skippedRequiredActionCount: failedRequiredActions.length,
    skippedOperations: skippedOperations.slice(0, 20).map((record) => ({
      index: record.index,
      label: record.label,
      reason: record.actionResult?.reason ?? null
    }))
  };
}

function validateCoverage(coverage) {
  const failures = [];
  if (allowPartialCoverage) {
    return failures;
  }
  if (targetOperationCount === 0) {
    return failures;
  }
  if (operationRecords.length < targetOperationCount) {
    failures.push(`operation count too low: ${operationRecords.length}/${targetOperationCount}`);
    return failures;
  }
  for (const layoutClass of REQUIRED_LAYOUT_CLASSES) {
    if (!coverage.layoutClasses[layoutClass]) {
      failures.push(`layout not covered: ${layoutClass}`);
    }
  }
  for (const family of [
    "adaptive cross-tree folder switch",
    "waterfall cross-tree folder switch",
    "grid cross-tree folder switch",
    "list cross-tree folder switch",
    "adaptive deep scroll",
    "waterfall deep scroll",
    "grid deep scroll",
    "list deep scroll",
    "artists root recursive total deep scroll",
    "tree directory deep scroll and switch",
    "tree directory scrollbar drag",
    "subfolder hierarchy expand",
    "preview open clicked media",
    "preview close",
    "sort Name A-Z",
    "sort Name Z-A",
    "sort Newest first"
  ]) {
    if (!coverage.operationFamilyCounts[family]) {
      failures.push(`operation family not covered: ${family}`);
    }
  }
  const strict = targetOperationCount >= 500;
  const minFolders = strict ? 18 : 6;
  const minFolderLabels = strict ? 12 : 4;
  const minMediaLabels = strict ? 8 : 3;
  if (coverage.selectedFolderKeys.length < minFolders) {
    failures.push(`distinct selected folders too low: ${coverage.selectedFolderKeys.length}/${minFolders}`);
  }
  if (coverage.clickedFolderLabels.length < minFolderLabels) {
    failures.push(`distinct clicked folders too low: ${coverage.clickedFolderLabels.length}/${minFolderLabels}`);
  }
  if (coverage.clickedMediaLabels.length < minMediaLabels) {
    failures.push(`distinct clicked media too low: ${coverage.clickedMediaLabels.length}/${minMediaLabels}`);
  }
  if (strict && coverage.maxSelectedDepth < 2) {
    failures.push(`tree depth coverage too shallow: ${coverage.maxSelectedDepth}/2`);
  }
  if (coverage.skippedRequiredActionCount > 0) {
    failures.push(`required operations skipped: ${coverage.skippedRequiredActionCount}`);
  }
  return failures;
}

function buildSummary(ok) {
  const slowOperations = operationRecords.filter((record) => record.slow);
  const coverage = buildCoverage();
  const coverageFailures = validateCoverage(coverage);
  const runOk = ok && coverageFailures.length === 0;
  const databaseFiles = collectDatabaseFileStats();
  return {
    run: {
      ok: runOk,
      root: autoRoot,
      launchCommand: startDesktopCommand,
      dataDir,
      dbPath,
      debugPort,
      targetOperationCount,
      actualOperationCount: operationRecords.length,
      resetData,
      allowPartialCoverage,
      thumbnailCacheMode,
      thumbnailCacheBatchLimit,
      waitForBulkIdleBeforeSweep,
      waitForThumbnailTaskDrainAfterSweep,
      startedAtUtc: new Date(startedAt).toISOString(),
      completedAtUtc: new Date().toISOString()
    },
    seenMediaIds: [
      ...new Set(
        operationRecords.flatMap((record) => [
          ...(record.dom?.visibleMediaIds ?? []),
          ...(record.dom?.selectedMediaId ? [Number(record.dom.selectedMediaId)] : [])
        ])
      )
    ].filter((value) => Number.isInteger(value) && value > 0),
    thumbnailCache: {
      before: thumbnailCacheStatsBefore,
      after: thumbnailCacheStatsAfter
    },
    databaseFiles,
    stats: {
      folderSwitch: statsFor(/folder|subfolder|tree/i),
      scroll: statsFor(/scroll/i),
      previewOpen: statsFor(/preview open/i),
      previewNavigation: statsFor(/preview next|preview previous/i),
      layoutSwitch: statsFor(/layout/i),
      adaptiveWaterfallFolderSwitch: statsFor(/adaptive cross-tree folder switch|waterfall cross-tree folder switch/i),
      recursiveDeepScroll: statsFor(/recursive child contents deep scroll|artists root recursive total deep scroll/i),
      treeDirectory: statsFor(/tree directory/i),
      scrollbarDrag: statsFor(/scrollbar drag/i),
      visibleThumbnailWait: summarizeActionWait("visibleReady"),
      selectedThumbnailWait: summarizeActionWait("selectedReady"),
      previewReadyWait: summarizeActionWait("previewReady")
    },
    coverage,
    coverageFailures,
    slowestOperations: [...operationRecords]
      .sort((left, right) => right.elapsedMs - left.elapsedMs)
      .slice(0, 10)
      .map((record) => ({
        index: record.index,
        label: record.label,
        elapsedMs: record.elapsedMs,
        longTaskDurationMs: record.longTaskDurationMs,
        api: record.api,
        dom: record.dom,
        actionResult: record.actionResult,
        error: record.error
      })),
    slowOperations,
    failures,
    consoleErrors,
    networkProblems,
    operationRecords,
    stdoutPath,
    stderrPath
  };
}

function summarizeActionWait(key) {
  const waits = operationRecords
    .filter((record) => key !== "visibleReady" || (record.actionResult?.[key]?.total ?? 0) > 0)
    .map((record) => record.actionResult?.[key]?.elapsedMs)
    .filter((value) => typeof value === "number");
  const incomplete = operationRecords.filter(
    (record) =>
      record.actionResult?.[key]?.complete === false &&
      (key !== "visibleReady" || (record.actionResult?.[key]?.total ?? 0) > 0)
  ).length;
  return {
    count: waits.length,
    incomplete,
    p50: percentile(waits, 50),
    p95: percentile(waits, 95),
    max: waits.length > 0 ? Math.max(...waits) : 0
  };
}

const startedAt = Date.now();

try {
  debugPort = await resolveDebugPort(debugPort);
  console.log(`Using Electron debug port ${debugPort}`);
  startDevApp();
  await waitForUrl(webUrl, 90_000);
  const client = new CdpClient(await waitForTarget());
  await client.ready;
  await client.send("Page.enable");
  await client.send("Runtime.enable");
  await client.send("Log.enable");
  await client.send("Network.enable");
  await waitFor(
    client,
    "Boolean(window.megleDesktop?.coreUrl && window.megleDesktop?.sessionToken)",
    90_000,
    "desktop bridge core session"
  );
  await waitFor(
    client,
    `(async () => {
      const bridge = window.megleDesktop;
      if (!bridge?.coreUrl || !bridge?.sessionToken) return false;
      try {
        const url = new URL("health", bridge.coreUrl.endsWith("/") ? bridge.coreUrl : bridge.coreUrl + "/").toString();
        const response = await fetch(url, {
          headers: { "x-megle-session": bridge.sessionToken }
        });
        return response.ok;
      } catch {
        return false;
      }
    })()`,
    90_000,
    "desktop core api health"
  );
  await prepareThumbnailCacheScenario(client);
  await installInstrumentation(client);
  await runSweep(client);
  if (waitForThumbnailTaskDrainAfterSweep) {
    await waitForThumbnailTaskDrain(client);
  }
  thumbnailCacheStatsAfter = await desktopApiJson(client, "/thumbnails/cache/stats").catch(
    () => null
  );
  client.close();
  const slowOperations = operationRecords.filter((record) => record.slow);
  if (failOnSlow && slowOperations.length > 0) {
    failures.push(...slowOperations);
  }
  const unexpectedNetworkProblems = networkProblems.filter(
    (problem) => !isExpectedCanceledNetworkProblem(problem)
  );
  if (consoleErrors.length > 0) {
    failures.push(`console errors: ${consoleErrors.length}`);
  }
  if (unexpectedNetworkProblems.length > 0) {
    failures.push(`unexpected network failures: ${unexpectedNetworkProblems.length}`);
  }
  const summary = buildSummary(failures.length === 0);
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));
  console.log(`Artists million desktop sweep complete. Summary: ${summaryPath}`);
  if (!summary.run.ok) process.exitCode = 1;
} catch (error) {
  failures.push(String(error));
  const summary = buildSummary(false);
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));
  throw error;
} finally {
  await stopDevApp();
}

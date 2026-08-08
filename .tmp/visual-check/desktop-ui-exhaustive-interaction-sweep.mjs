import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";
import { prepareControlledMediaRoot, resolveRepositoryRoot } from "./controlled-media.mjs";
import {
  collectUnexpectedConsoleErrors,
  collectUnexpectedNetworkProblems
} from "./network-diagnostics.mjs";
import { collectStartupDiagnostics as parseStartupDiagnostics } from "./startup-diagnostics.mjs";

const root = await resolveRepositoryRoot(import.meta.url);
const visualRoot = path.join(root, ".tmp", "visual-check");
const logDir = path.join(visualRoot, "logs");
const visualRunId = `megle-exhaustive-${Date.now().toString(36)}-${process.pid}`;
const dataDir = path.join(visualRoot, `data-exhaustive-ui-${visualRunId}`);
const electronUserDataDir = path.join(dataDir, "electron-user-data");
const controlledMedia = await prepareControlledMediaRoot(visualRoot, visualRunId);
const autoRoot = controlledMedia.root;
const webUrl = "http://127.0.0.1:5178";
const debugPort = Number(process.env.MEGLE_EXHAUSTIVE_DEBUG_PORT ?? 9501);

const stdoutPath = path.join(logDir, `desktop-ui-exhaustive-${visualRunId}.stdout.log`);
const stderrPath = path.join(logDir, `desktop-ui-exhaustive-${visualRunId}.stderr.log`);
const summaryPath = path.join(logDir, "desktop-ui-exhaustive-summary.json");

const stdout = createWriteStream(stdoutPath, { flags: "w" });
const stderr = createWriteStream(stderrPath, { flags: "w" });

const results = [];
const failures = [];
const consoleErrors = [];
const networkProblems = [];
const requestUrls = new Map();
const hardFailures = [];
const startupOutput = [];
let invokedTree = null;
let child;
let stoppingChild = false;

await mkdir(logDir, { recursive: true });
await mkdir(dataDir, { recursive: true });
await mkdir(electronUserDataDir, { recursive: true });

function appendOutput(source, chunk) {
  const text = chunk.toString();
  startupOutput.push({ source, text });
  if (source === "stderr") {
    stderr.write(text);
  } else {
    stdout.write(text);
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function collectStartupProblems() {
  return parseStartupDiagnostics(startupOutput).startupProblems;
}

async function waitForControlledRootStartup(timeoutMs = 60_000) {
  const expected = `[megle] auto-added root ${autoRoot}`;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (startupOutput.some((entry) => entry.text.includes(expected))) return;
    const startupProblems = collectStartupProblems();
    const rootFailure = startupProblems.find((problem) =>
      /MEGLE_AUTO_ADD_ROOT|auto-add|auto.?added root/i.test(problem.line)
    );
    if (rootFailure) throw new Error(`Controlled root startup failed: ${rootFailure.line}`);
    await delay(100);
  }
  throw new Error(`Timed out waiting for controlled root startup evidence: ${expected}`);
}

function startDevApp() {
  child = spawn("npm", ["run", "dev"], {
    cwd: root,
    env: {
      ...process.env,
      MEGLE_WEB_URL: webUrl,
      MEGLE_DB_PATH: path.join(dataDir, "megle.sqlite"),
      MEGLE_ELECTRON_USER_DATA_DIR: electronUserDataDir,
      MEGLE_AUTO_ADD_ROOT: autoRoot,
      MEGLE_REMOTE_DEBUG: "1",
      MEGLE_REMOTE_DEBUG_PORT: String(debugPort),
      MEGLE_VISUAL_HARNESS: "1",
      MEGLE_VISUAL_RUN_ID: visualRunId
    },
    shell: process.platform === "win32",
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true
  });

  child.stdout.on("data", (chunk) => appendOutput("stdout", chunk));
  child.stderr.on("data", (chunk) => appendOutput("stderr", chunk));
  child.on("error", (error) => hardFailures.push(`dev process error: ${error}`));
  child.on("exit", (code, signal) => {
    if (!stoppingChild) {
      hardFailures.push(`dev process exited before sweep completion: code=${code} signal=${signal ?? "none"}`);
    }
  });
}

async function stopDevApp() {
  if (!child || child.killed || child.pid === undefined) {
    return;
  }

  stoppingChild = true;
  if (process.platform === "win32") {
    await new Promise((resolve) => {
      const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
        stdio: "ignore",
        windowsHide: true
      });
      killer.on("error", () => resolve());
      killer.on("exit", () => resolve());
    });
    return;
  }

  child.kill("SIGTERM");
  await delay(2000);
  if (!child.killed) {
    child.kill("SIGKILL");
  }
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

async function waitForUrl(url, timeoutMs = 30_000) {
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

async function waitForTarget(timeoutMs = 120_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const targets = await httpJson(`http://127.0.0.1:${debugPort}/json/list`);
      const page = targets.find((target) => target.type === "page" && target.url.startsWith(webUrl));
      if (page?.webSocketDebuggerUrl) {
        return page.webSocketDebuggerUrl;
      }
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }
  throw new Error(`Timed out waiting for Electron CDP target: ${String(lastError)}`);
}

async function pageTargets() {
  const targets = await httpJson(`http://127.0.0.1:${debugPort}/json/list`);
  return targets.filter((target) => target.type === "page" && target.url.startsWith(webUrl));
}

async function waitForPageTarget(urlPart, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastTargets = [];
  while (Date.now() < deadline) {
    lastTargets = await pageTargets().catch(() => []);
    const target = lastTargets.find((candidate) => candidate.url.includes(urlPart));
    if (target?.webSocketDebuggerUrl) return target;
    await delay(250);
  }
  throw new Error(`Timed out waiting for page target ${urlPart}; targets=${JSON.stringify(lastTargets.map((target) => target.url))}`);
}

async function waitForPageTargetCount(urlPart, expectedCount, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  let lastCount = 0;
  while (Date.now() < deadline) {
    lastCount = (await pageTargets().catch(() => [])).filter((target) => target.url.includes(urlPart)).length;
    if (lastCount === expectedCount) return lastCount;
    await delay(250);
  }
  throw new Error(`Timed out waiting for ${urlPart} target count ${expectedCount}; last=${lastCount}`);
}

class CdpClient {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.nextId = 1;
    this.pending = new Map();
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
    if (message.method === "Runtime.exceptionThrown") {
      consoleErrors.push(message.params?.exceptionDetails?.exception?.description ?? JSON.stringify(message.params));
    }
    if (message.method === "Runtime.consoleAPICalled" && message.params?.type === "error") {
      consoleErrors.push(
        message.params.args?.map((argument) => argument.value ?? argument.description ?? "").join(" ") ??
          JSON.stringify(message.params)
      );
    }
    if (message.method === "Log.entryAdded" && message.params?.entry?.level === "error") {
      consoleErrors.push(message.params.entry.text ?? JSON.stringify(message.params.entry));
    }
    if (message.method === "Network.requestWillBeSent") {
      requestUrls.set(message.params?.requestId, message.params?.request?.url ?? null);
    }
    if (message.method === "Network.responseReceived") {
      const response = message.params?.response ?? {};
      if ((response.status ?? 0) >= 400) {
        networkProblems.push({
          requestId: message.params?.requestId,
          url: response.url ?? requestUrls.get(message.params?.requestId) ?? null,
          status: response.status,
          type: message.params?.type ?? null
        });
      }
    }
    if (message.method === "Network.loadingFailed" && message.params?.type !== "WebSocket") {
      networkProblems.push({
        requestId: message.params?.requestId,
        url: requestUrls.get(message.params?.requestId) ?? null,
        status: "loadingFailed",
        type: message.params?.type ?? null,
        errorText: message.params?.errorText ?? null,
        canceled: message.params?.canceled === true,
        blockedReason: message.params?.blockedReason ?? null
      });
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

async function evaluate(client, expression) {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true
  });
  if (result.exceptionDetails) {
    throw new Error(JSON.stringify(result.exceptionDetails));
  }
  return result.result?.value;
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

async function pointFor(client, selector, xRatio = 0.5, yRatio = 0.5) {
  const point = await evaluate(
    client,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return { x: Math.round(rect.left + rect.width * ${xRatio}), y: Math.round(rect.top + rect.height * ${yRatio}) };
    })()`
  );
  if (!point) {
    throw new Error(`Selector not found: ${selector}`);
  }
  return point;
}

async function pointForContains(client, selector, text) {
  const point = await evaluate(
    client,
    `(() => {
      const nodes = [...document.querySelectorAll(${JSON.stringify(selector)})];
      const node = nodes.find((candidate) => (candidate.textContent || "").includes(${JSON.stringify(text)}));
      if (!node) {
        return { options: nodes.map((candidate) => (candidate.textContent || "").trim()) };
      }
      const rect = node.getBoundingClientRect();
      return {
        x: Math.round(rect.left + rect.width / 2),
        y: Math.round(rect.top + rect.height / 2),
        text: (node.textContent || "").trim()
      };
    })()`
  );
  if (!point || !("x" in point)) {
    throw new Error(`Contains target not found: ${text}; got=${JSON.stringify(point)}`);
  }
  return point;
}

async function mouse(client, type, point, extra = {}) {
  await client.send("Input.dispatchMouseEvent", {
    type,
    x: point.x,
    y: point.y,
    button: extra.button ?? "none",
    clickCount: extra.clickCount ?? 0,
    pointerType: "mouse"
  });
}

async function clickSelector(client, selector, xRatio = 0.5, yRatio = 0.5) {
  await client.send("Page.bringToFront").catch(() => undefined);
  await evaluate(client, "window.focus(); true").catch(() => undefined);
  const point = await pointFor(client, selector, xRatio, yRatio);
  await mouse(client, "mouseMoved", point);
  await mouse(client, "mousePressed", point, { button: "left", clickCount: 1 });
  await mouse(client, "mouseReleased", point, { button: "left", clickCount: 1 });
  await delay(250);
}

async function clickContains(client, selector, text) {
  const point = await pointForContains(client, selector, text);
  await mouse(client, "mouseMoved", point);
  await mouse(client, "mousePressed", point, { button: "left", clickCount: 1 });
  await mouse(client, "mouseReleased", point, { button: "left", clickCount: 1 });
  await delay(300);
}

async function invokeContains(client, selector, text) {
  const outcome = await evaluate(
    client,
    `(() => {
      const nodes = [...document.querySelectorAll(${JSON.stringify(selector)})];
      const node = nodes.find((candidate) => (candidate.textContent || "").includes(${JSON.stringify(text)}));
      if (!(node instanceof HTMLElement)) {
        return { ok: false, options: nodes.map((candidate) => (candidate.textContent || "").trim()) };
      }
      node.click();
      return { ok: true, text: (node.textContent || "").trim() };
    })()`
  );
  if (!outcome?.ok) {
    throw new Error(`Invoke target not found: ${text}; got=${JSON.stringify(outcome)}`);
  }
  await delay(300);
}

async function invokeSelector(client, selector) {
  const outcome = await evaluate(
    client,
    `(() => {
      const node = document.querySelector(${JSON.stringify(selector)});
      if (!(node instanceof HTMLElement)) {
        return false;
      }
      node.click();
      return true;
    })()`
  );
  if (!outcome) {
    throw new Error(`Invoke selector not found: ${selector}`);
  }
  await delay(300);
}

async function dispatchContextMenuContains(client, selector, text) {
  const result = await evaluate(
    client,
    `(() => {
      const nodes = [...document.querySelectorAll(${JSON.stringify(selector)})];
      const node = nodes.find((candidate) => (candidate.textContent || "").includes(${JSON.stringify(text)}));
      if (!(node instanceof HTMLElement)) {
        return { ok: false, options: nodes.map((candidate) => (candidate.textContent || "").trim()) };
      }
      const rect = node.getBoundingClientRect();
      const clientX = Math.round(rect.left + rect.width / 2);
      const clientY = Math.round(rect.top + rect.height / 2);
      node.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX,
        clientY,
        button: 2
      }));
      return { ok: true };
    })()`
  );
  if (!result?.ok) {
    throw new Error(`Context-menu target not found: ${text}; got=${JSON.stringify(result)}`);
  }
  await delay(350);
}

async function dispatchContextMenuFirstMediaTile(client) {
  const result = await evaluate(
    client,
    `(() => {
      const tile = document.querySelector(".media-tile");
      if (!(tile instanceof HTMLElement)) {
        return { ok: false };
      }
      const rect = tile.getBoundingClientRect();
      const clientX = Math.round(rect.left + rect.width / 2);
      const clientY = Math.round(rect.top + Math.min(rect.height * 0.4, 80));
      tile.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX,
        clientY,
        button: 2
      }));
      return { ok: true };
    })()`
  );
  if (!result?.ok) {
    throw new Error("No media tile available for context menu");
  }
  await delay(350);
}

async function doubleClickFirstTile(client) {
  const outcome = await evaluate(
    client,
    `(() => {
      const tile = document.querySelector(".media-tile");
      if (!(tile instanceof HTMLElement)) {
        return false;
      }
      tile.dispatchEvent(new MouseEvent("dblclick", {
        bubbles: true,
        cancelable: true,
        button: 0
      }));
      return true;
    })()`
  );
  if (!outcome) {
    throw new Error("No media tile for preview");
  }
  await delay(400);
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
      const input = document.querySelector(".search-bar .overlay-field-input");
      if (!(input instanceof HTMLInputElement)) return false;
      input.focus();
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(input, ${JSON.stringify(value)});
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    })()`
  );
  await delay(500);
}

async function installShellSpy(client) {
  await evaluate(
    client,
    `(() => {
      const shell = window.megleDesktop?.shell;
      if (!shell) return false;
      window.__megleShellSpy = { calls: [] };
      shell.openPath = async (value) => {
        window.__megleShellSpy.calls.push({ method: "openPath", value });
        return true;
      };
      shell.revealPath = async (value) => {
        window.__megleShellSpy.calls.push({ method: "revealPath", value });
        return true;
      };
      shell.copyText = async (value) => {
        window.__megleShellSpy.calls.push({ method: "copyText", value });
        return true;
      };
      return true;
    })()`
  );
}

async function consumeShellCalls(client) {
  return (
    (await evaluate(
      client,
      `(() => {
        const spy = window.__megleShellSpy;
        if (!spy) return [];
        const calls = [...spy.calls];
        spy.calls.length = 0;
        return calls;
      })()`
    )) ?? []
  );
}

async function contextMenuItems(client) {
  return await evaluate(
    client,
    `(() => [...document.querySelectorAll('#context-menu-root [data-overlay-menu-item="true"]')].map((node) => ({
      label: (node.textContent || '').trim(),
      disabled: node instanceof HTMLButtonElement ? node.disabled : node.getAttribute('aria-disabled') === 'true'
    })))()`
  );
}

async function ensureCompactPopoverClosed(client, name) {
  const rootSelector = `#${name}-root`;
  const open = await evaluate(
    client,
    `Boolean(document.querySelector(${JSON.stringify(rootSelector)}))`
  );
  if (open) {
    await pressKey(client, "Escape");
    await waitFor(
      client,
      `!document.querySelector(${JSON.stringify(rootSelector)})`,
      10_000,
      `${name} popover close`
    );
  }
}

async function clickContextMenuContains(client, text) {
  await clickContains(client, '#context-menu-root [data-overlay-menu-item="true"]', text);
}

async function expectDialog(client, titleText) {
  await waitFor(
    client,
    `Boolean([...document.querySelectorAll('.overlay-modal-surface h2')].find((node) => (node.textContent || '').includes(${JSON.stringify(titleText)})))`,
    10_000,
    `dialog ${titleText}`
  );
}

async function closeDialog(client) {
  await clickContains(client, ".overlay-dialog-button", "Cancel");
  await waitFor(
    client,
    `!document.querySelector('.overlay-modal-backdrop:not([aria-hidden="true"])')`,
    10_000,
    "dialog close"
  );
}

async function openContextMenuForMedia(client) {
  await dispatchContextMenuFirstMediaTile(client);
  await waitFor(client, `Boolean(document.querySelector('.context-menu'))`, 10_000, "media context menu");
}

async function openContextMenuForTreeNode(client, text) {
  await dispatchContextMenuContains(client, ".tree-item .tree-label", text);
  await waitFor(client, `Boolean(document.querySelector('.context-menu'))`, 10_000, `context menu ${text}`);
}

function assertMenuLabels(actualItems, expectedLabels, label) {
  const labels = actualItems.map((item) => item.label);
  for (const expected of expectedLabels) {
    if (!labels.some((labelText) => labelText.includes(expected))) {
      throw new Error(`${label} missing menu item: ${expected}; actual=${JSON.stringify(labels)}`);
    }
  }
}

function assertMenuItemEnabled(actualItems, expectedLabel, label) {
  const item = actualItems.find((entry) => entry.label.includes(expectedLabel));
  if (!item) {
    throw new Error(`${label} missing enabled-state item: ${expectedLabel}`);
  }
  if (item.disabled) {
    throw new Error(`${label} item unexpectedly disabled: ${expectedLabel}`);
  }
}

async function runExhaustiveSweep(client) {
  await waitFor(
    client,
    `document.querySelectorAll('.tree-item').length > 0 && document.querySelectorAll('.media-tile').length > 0`,
    180_000,
    "library loaded"
  );
  results.push("startup/library load");

  await invokeSelector(client, '[data-titlebar-control="open-settings"]');
  const settingsTarget = await waitForPageTarget("#/settings");
  const settingsClient = new CdpClient(settingsTarget.webSocketDebuggerUrl);
  await settingsClient.ready;
  await Promise.all([
    settingsClient.send("Page.enable"),
    settingsClient.send("Runtime.enable")
  ]);
  await waitFor(settingsClient, `Boolean(document.querySelector('.settings-window'))`, 10_000, "Settings secondary window");
  await evaluate(
    settingsClient,
    `(() => {
      const tab = [...document.querySelectorAll('.settings-dialog-nav-item')]
        .find((node) => node.textContent?.trim() === '外观');
      tab?.click();
      return Boolean(tab);
    })()`
  );
  await waitFor(settingsClient, `Boolean(document.querySelector('#library-grid-gap'))`, 10_000, "Settings library grid controls");
  results.push("primary nav -> settings secondary window");
  await evaluate(settingsClient, `void window.megleDesktop?.closeSettingsWindow?.()`);
  await waitForPageTargetCount("#/settings", 0);
  settingsClient.close();

  await invokeSelector(client, '[data-titlebar-control="open-plugins"]');
  const pluginsTarget = await waitForPageTarget("#/plugins");
  const pluginsClient = new CdpClient(pluginsTarget.webSocketDebuggerUrl);
  await pluginsClient.ready;
  await Promise.all([
    pluginsClient.send("Page.enable"),
    pluginsClient.send("Runtime.enable")
  ]);
  await waitFor(pluginsClient, `Boolean(document.querySelector('.plugins-body'))`, 10_000, "Plugins secondary window");
  await waitFor(
    pluginsClient,
    `document.querySelector('.secondary-window')?.getAttribute('data-secondary-content-visible') === 'true'`,
    10_000,
    "Plugins native-window reveal"
  );
  results.push("primary nav -> plugins secondary window");
  await evaluate(pluginsClient, `void window.megleDesktop?.closePluginsWindow?.()`);
  await waitForPageTargetCount("#/plugins", 0);
  pluginsClient.close();

  await invokeSelector(client, '[data-titlebar-control="library-switcher"]');
  await waitFor(client, `Boolean(document.querySelector('#library-root'))`, 10_000, "library switcher");
  await pressKey(client, "Escape");
  await waitFor(client, `!document.querySelector('#library-root')`, 10_000, "library switcher close");
  await waitFor(client, `Boolean(document.querySelector('.virtual-grid'))`, 10_000, "back to library");
  results.push("primary nav -> library switcher");

  await invokeSelector(client, '[data-titlebar-control="notifications"]');
  await waitFor(client, `Boolean(document.querySelector('#notifications-root'))`, 10_000, "notifications popover");
  await pressKey(client, "Escape");
  await waitFor(client, `!document.querySelector('#notifications-root')`, 10_000, "notifications popover close");
  results.push("notifications popover");

  for (const label of ["Images", "Videos", "Other", "1+", "2+", "3+", "4+", "5+", "Favorites"]) {
    await ensureCompactPopoverClosed(client, "filter");
    await invokeSelector(client, '[data-titlebar-control="library-filter"]');
    await waitFor(client, `Boolean(document.querySelector('#filter-root'))`, 10_000, `filter popover ${label}`);
    await invokeContains(client, ".filter-menu-item", label);
  }
  await ensureCompactPopoverClosed(client, "filter");
  await invokeSelector(client, '[data-titlebar-control="library-filter"]');
  await waitFor(client, `Boolean(document.querySelector('#filter-root'))`, 10_000, "filter popover clear");
  await invokeContains(client, ".filter-menu-clear", "Clear filters");
  if (await evaluate(client, `Boolean(document.querySelector('#filter-root'))`)) {
    await pressKey(client, "Escape");
  }
  await waitFor(client, `!document.querySelector('#filter-root')`, 10_000, "filter clear settle");
  results.push("filter menu sweep");

  for (const label of [
    "Newest first",
    "Oldest first",
    "Name A",
    "Name Z",
    "Highest rated",
    "Lowest rated"
  ]) {
    await invokeSelector(client, '[data-titlebar-control="library-sort"]');
    await waitFor(client, `Boolean(document.querySelector('#sort-root'))`, 10_000, `sort popover ${label}`);
    await invokeContains(client, "#sort-root [data-overlay-menu-item=\"true\"]", label);
  }
  results.push("sort menu sweep");

  for (const label of ["Adaptive", "Waterfall", "Grid", "List", "Grid"]) {
    await invokeSelector(client, ".layout-menu-trigger");
    await waitFor(client, `Boolean(document.querySelector('#layout-root'))`, 10_000, `layout popover ${label}`);
    await invokeContains(client, "#layout-root [data-overlay-menu-item=\"true\"]", label);
  }
  await waitFor(client, `Boolean(document.querySelector('.virtual-grid--grid'))`, 10_000, "grid restore");
  results.push("layout menu sweep");

  const childToggleExists = await evaluate(client, `Boolean(document.querySelector('.library-browser-content-toggle input[type="checkbox"]'))`);
  if (childToggleExists) {
    const before = await evaluate(client, `document.querySelector('.library-browser-content-toggle input')?.checked ?? null`);
    await clickSelector(client, ".library-browser-content-toggle");
    await waitFor(client, `(document.querySelector('.library-browser-content-toggle input')?.checked ?? null) !== ${before}`, 10_000, "child contents toggle change");
    await clickSelector(client, ".library-browser-content-toggle");
    await waitFor(client, `(document.querySelector('.library-browser-content-toggle input')?.checked ?? null) === ${before}`, 10_000, "child contents toggle restore");
    results.push("child folder contents toggle");
  }

  const subfolderLabels = (await evaluate(
    client,
    `[...document.querySelectorAll('.subfolder-card-name')].map((node) => (node.textContent || '').trim())`
  )) ?? [];
  if (subfolderLabels[0]) {
    await clickContains(client, ".subfolder-card-name", subfolderLabels[0]);
    await delay(500);
    results.push("subfolder card sweep");
  }

  await setSearch(client, "png");
  await waitFor(client, `document.querySelector('.search-bar .overlay-field-input')?.value === 'png'`, 10_000, "search set");
  await setSearch(client, "");
  await waitFor(client, `document.querySelector('.search-bar .overlay-field-input')?.value === ''`, 10_000, "search clear");
  results.push("search set/clear");

  await clickContains(client, ".tree-item .tree-label", "img2img-images");
  await delay(600);
  await clickContains(client, ".tree-item .tree-label", "txt2img-images");
  await delay(600);
  results.push("tree folder switch");

  await clickContains(client, ".tree-item .tree-label", "img2img-grids");
  await waitFor(client, `document.querySelectorAll('.media-gridcell').length > 0`, 30_000, "img2img-grids media");
  await clickSelector(client, '[data-titlebar-control="library-filter"]');
  await waitFor(client, `Boolean(document.querySelector('#filter-root'))`, 10_000, "filter open for zero-result regression");
  await invokeContains(client, ".filter-menu-item", "Videos");
  await waitFor(
    client,
    `Boolean(document.querySelector('.grid-empty')) || document.querySelectorAll('.media-gridcell').length === 0`,
    10_000,
    "empty after videos filter"
  );
  await invokeContains(client, ".filter-menu-item", "Videos");
  await waitFor(client, `document.querySelectorAll('.media-gridcell').length > 0`, 20_000, "restored cells after videos filter");
  const restoredRowLefts = await evaluate(
    client,
    `[...document.querySelectorAll('.media-gridcell')].slice(0, 6).map((el) => Math.round(el.getBoundingClientRect().left))`
  );
  const distinctRestoredRowLefts = [...new Set(restoredRowLefts)];
  if (distinctRestoredRowLefts.length <= 1) {
    throw new Error(
      `filter toggle layout regression: expected multiple columns after restoring from empty state, got ${JSON.stringify(
        restoredRowLefts
      )}`
    );
  }
  results.push("filter zero-result restore");

  await waitFor(client, `document.querySelectorAll('.media-tile').length > 0`, 30_000, "media tiles before preview sweep");

  await doubleClickFirstTile(client);
  await waitFor(client, `Boolean(document.querySelector('.central-preview-stage'))`, 10_000, "preview open");
  await clickSelector(client, '[aria-label="Next media"]');
  await clickSelector(client, '[aria-label="Actual size"]');
  await clickSelector(client, '[aria-label="Back to library"]');
  await waitFor(client, `!document.querySelector('.central-preview-stage')`, 10_000, "preview close");
  results.push("preview toolbar sweep");

  await invokeSelector(client, '[data-titlebar-control="open-settings"]');
  const gridSettingsTarget = await waitForPageTarget("#/settings");
  const gridSettingsClient = new CdpClient(gridSettingsTarget.webSocketDebuggerUrl);
  await gridSettingsClient.ready;
  await gridSettingsClient.send("Runtime.enable");
  await evaluate(
    gridSettingsClient,
    `(() => {
      const tab = [...document.querySelectorAll('.settings-dialog-nav-item')]
        .find((node) => node.textContent?.trim() === '外观');
      tab?.click();
      return Boolean(tab);
    })()`
  );
  await waitFor(gridSettingsClient, `Boolean(document.querySelector('#library-grid-gap'))`, 10_000, "settings grid controls");
  await evaluate(
    gridSettingsClient,
    `(() => {
      const gapInput = document.querySelector('#library-grid-gap');
      const labelInput = document.querySelector('#library-grid-label-height');
      for (const input of [gapInput, labelInput]) {
        if (!(input instanceof HTMLInputElement)) continue;
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
        const next = input.value === input.max ? input.min : input.max;
        setter?.call(input, next);
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.dispatchEvent(new Event('change', { bubbles: true }));
      }
      return true;
    })()`
  );
  await evaluate(gridSettingsClient, `void window.megleDesktop?.closeSettingsWindow?.()`);
  await waitForPageTargetCount("#/settings", 0);
  gridSettingsClient.close();
  await waitFor(client, `Boolean(document.querySelector('.virtual-grid'))`, 10_000, "return to library");
  results.push("settings grid controls");

  await installShellSpy(client);

  await openContextMenuForMedia(client);
  assertMenuLabels(await contextMenuItems(client), [
    "Preview",
    "Open",
    "Rename",
    "Move to",
    "Move to recycle bin",
    "Delete permanently",
    "Copy path",
    "Reveal in Explorer"
  ], "media");
  const mediaItems = await contextMenuItems(client);
  assertMenuItemEnabled(mediaItems, "Open", "media");
  assertMenuItemEnabled(mediaItems, "Copy path", "media");
  assertMenuItemEnabled(mediaItems, "Reveal in Explorer", "media");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Preview");
  await waitFor(client, `Boolean(document.querySelector('.central-preview-stage'))`, 10_000, "media context preview");
  await clickSelector(client, '[aria-label="Back to library"]');
  await waitFor(client, `!document.querySelector('.central-preview-stage')`, 10_000, "close preview after media context");
  await openContextMenuForMedia(client);
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Rename");
  await expectDialog(client, "Rename file");
  await closeDialog(client);
  await openContextMenuForMedia(client);
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Move to");
  await expectDialog(client, "Move");
  await closeDialog(client);
  await openContextMenuForMedia(client);
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Move to recycle bin");
  await expectDialog(client, "Move to recycle bin");
  await closeDialog(client);
  await openContextMenuForMedia(client);
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Delete permanently");
  await expectDialog(client, "Delete permanently");
  await closeDialog(client);
  results.push("media context menu sweep");

  await openContextMenuForTreeNode(client, "img2img-images");
  const folderItems = await contextMenuItems(client);
  assertMenuLabels(folderItems, [
    "Open folder",
    "Refresh folder",
    "Rescan root",
    "Rename",
    "Move to",
    "Move to recycle bin",
    "Delete permanently",
    "Copy path",
    "Reveal in Explorer"
  ], "folder");
  assertMenuItemEnabled(folderItems, "Open folder", "folder");
  assertMenuItemEnabled(folderItems, "Refresh folder", "folder");
  assertMenuItemEnabled(folderItems, "Rescan root", "folder");
  assertMenuItemEnabled(folderItems, "Copy path", "folder");
  assertMenuItemEnabled(folderItems, "Reveal in Explorer", "folder");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Open folder");
  await delay(500);
  await openContextMenuForTreeNode(client, "img2img-images");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Refresh folder");
  await delay(500);
  await openContextMenuForTreeNode(client, "img2img-images");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Rescan root");
  await delay(500);
  await openContextMenuForTreeNode(client, "img2img-images");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Rename");
  await expectDialog(client, "Rename folder");
  await closeDialog(client);
  await openContextMenuForTreeNode(client, "img2img-images");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Move to");
  await expectDialog(client, "Move");
  await closeDialog(client);
  await openContextMenuForTreeNode(client, "img2img-images");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Move to recycle bin");
  await expectDialog(client, "Move to recycle bin");
  await closeDialog(client);
  await openContextMenuForTreeNode(client, "img2img-images");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Delete permanently");
  await expectDialog(client, "Delete permanently");
  await closeDialog(client);
  results.push("folder context menu sweep");

  await openContextMenuForTreeNode(client, "outputs");
  const rootItems = await contextMenuItems(client);
  assertMenuLabels(rootItems, [
    "Open root",
    "Refresh folder",
    "Rescan root",
    "Rename root",
    "Move root",
    "Remove root",
    "Copy path",
    "Reveal in Explorer"
  ], "root");
  const disabledLabels = rootItems.filter((item) => item.disabled).map((item) => item.label);
  for (const label of ["Rename root", "Move root", "Remove root"]) {
    if (!disabledLabels.some((value) => value.includes(label))) {
      throw new Error(`root disabled item mismatch: missing ${label}; disabled=${JSON.stringify(disabledLabels)}`);
    }
  }
  assertMenuItemEnabled(rootItems, "Open root", "root");
  assertMenuItemEnabled(rootItems, "Refresh folder", "root");
  assertMenuItemEnabled(rootItems, "Rescan root", "root");
  assertMenuItemEnabled(rootItems, "Copy path", "root");
  assertMenuItemEnabled(rootItems, "Reveal in Explorer", "root");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Open root");
  await delay(500);
  await openContextMenuForTreeNode(client, "outputs");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Refresh folder");
  await delay(500);
  await openContextMenuForTreeNode(client, "outputs");
  await invokeContains(client, '#context-menu-root [data-overlay-menu-item="true"]', "Rescan root");
  await delay(500);
  await openContextMenuForTreeNode(client, "outputs");
  results.push("root context menu sweep");
}

function buildSummary() {
  const startupDiagnostics = parseStartupDiagnostics(startupOutput);
  const startupProblems = startupDiagnostics.startupProblems;
  const consoleFailures = collectUnexpectedConsoleErrors(consoleErrors);
  const networkFailures = collectUnexpectedNetworkProblems(networkProblems);
  const finalHardFailures = [...hardFailures];
  if (consoleFailures.length) finalHardFailures.push("console errors");
  if (networkFailures.length) finalHardFailures.push("network failures");
  const ok =
    startupProblems.length === 0 &&
    consoleFailures.length === 0 &&
    networkFailures.length === 0 &&
    finalHardFailures.length === 0 &&
    failures.length === 0;
  return {
    run: {
      ok,
      id: visualRunId,
      startedAtUtc: new Date(startedAt).toISOString(),
      completedAtUtc: new Date().toISOString(),
      repositoryRoot: root
    },
    controlledMedia,
    invokedTree,
    results,
    failures,
    scenarioFailures: failures,
    startupFailures: startupProblems,
    consoleFailures,
    consoleErrors,
    networkProblems,
    networkFailures,
    cargoBuildFinished: startupDiagnostics.cargoBuildFinished,
    cargoDiagnostics: startupDiagnostics.cargoDiagnostics,
    runtimeProblems: startupDiagnostics.runtimeProblems,
    startupProblems,
    hardFailures: [...new Set(finalHardFailures)],
    stdoutPath,
    stderrPath
  };
}

const startedAt = Date.now();
let client;

try {
  startDevApp();
  await waitForUrl(webUrl, 60_000);
  await waitForControlledRootStartup();
  client = new CdpClient(await waitForTarget());
  await client.ready;
  await client.send("Page.enable");
  await client.send("Runtime.enable");
  await client.send("Network.enable");
  await client.send("Log.enable");
  await waitFor(
    client,
    `location.href.startsWith(${JSON.stringify(webUrl)})`,
    30_000,
    "invoked tree URL"
  );
  invokedTree = {
    harnessPath: fileURLToPath(import.meta.url),
    repositoryRoot: root,
    childCwd: root,
    controlledMediaRoot: autoRoot,
    pageUrl: await evaluate(client, "location.href")
  };
  const controlledRelativePath = path.relative(root, autoRoot);
  if (controlledRelativePath.startsWith("..") || path.isAbsolute(controlledRelativePath)) {
    hardFailures.push(`controlled media root is outside the invoked repository: ${autoRoot}`);
  }
  if (!invokedTree.pageUrl.startsWith(webUrl)) {
    hardFailures.push(`CDP target does not belong to the invoked tree URL: ${invokedTree.pageUrl}`);
  }
  await runExhaustiveSweep(client);
} catch (error) {
  failures.push(String(error));
} finally {
  client?.close();
  await stopDevApp();
  await delay(250);
  const summary = buildSummary();
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));
  stdout.end();
  stderr.end();
  if (summary.run.ok) {
    console.log(`Desktop exhaustive interaction sweep complete. Summary: ${summaryPath}`);
  } else {
    console.error(`Desktop exhaustive interaction sweep failed. Summary: ${summaryPath}`);
    process.exitCode = 1;
  }
}

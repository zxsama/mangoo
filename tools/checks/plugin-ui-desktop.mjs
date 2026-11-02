// Real Host + packaged plugin document acceptance. Only isolated local test data;
// never configures an account, opens authorization, or requests remote writes.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { connectCdp } from "../dev/cdp-client.mjs";
import { createAndActivateManagedLibrary, addExistingMediaRoot, managedLibraryHostEnv, resolveCoreBinary } from "../dev/managed-library-harness.mjs";

const root = process.cwd();
const args = process.argv.slice(2);
const value = (flag, fallback) => args.includes(flag) ? args[args.indexOf(flag) + 1] : fallback;
const mode = value("--mode", "local");
assert.ok(["local", "plugin", "notification"].includes(mode));
const output = path.resolve(value("--output", ".tmp/plugin-ui-extraction-20260923/desktop"));
await mkdir(output, { recursive: true });
const directory = await mkdtemp(path.join(output, `${mode}-`));
const binary = resolveCoreBinary({ root, coreBinary: value("--core", undefined) });
const plugins = path.resolve(value("--plugins", process.env.MANGOO_PLUGINS_DIR ?? path.join(path.dirname(binary), "bundled-plugins")));
const desktopExecutable = value("--desktop", null);
const webDirectory = path.resolve(value("--web", "apps/web/dist"));
const freePort = () => new Promise((resolve, reject) => { const server = net.createServer(); server.on("error", reject); server.listen(0, "127.0.0.1", () => { const port = server.address().port; server.close(() => resolve(port)); }); });
const port = await freePort();
const debugPort = Number(value("--debug-port", "9580"));
const origin = `http://127.0.0.1:${port}`;
const token = crypto.randomBytes(32).toString("base64url");
const env = { ...process.env, ...managedLibraryHostEnv(directory), MANGOO_CORE_ADDR: `127.0.0.1:${port}`, MANGOO_SESSION_TOKEN: token, MANGOO_ALLOWED_ORIGIN: origin, MANGOO_WEB_DIR: webDirectory, MANGOO_SERVE_WEB: "1", MANGOO_PLUGINS_DIR: plugins, MANGOO_PLUGIN_STATE_DIR: path.join(directory, "plugin-state"), MANGOO_PARENT_PID: String(process.pid) };
for (const key of Object.keys(env)) if (key.startsWith("MANGOO_ONEDRIVE_")) delete env[key];
const core = spawn(binary, [], { cwd: root, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
let coreLog = "";
core.stdout.on("data", (v) => { coreLog += v; }); core.stderr.on("data", (v) => { coreLog += v; });
let electron;
const connections = [];
const api = async (route, body, method = body ? "POST" : "GET") => {
  const response = await fetch(`${origin}/api${route}`, { method, headers: { "x-mangoo-session": token, ...(body ? { "content-type": "application/json" } : {}) }, body: body ? JSON.stringify(body) : undefined });
  assert.ok(response.ok, `${method} ${route}: ${response.status}`);
  return response.status === 204 ? null : response.json();
};
const wait = async (check, label, timeout = 30000) => { const until = Date.now() + timeout; while (Date.now() < until) { try { const result = await check(); if (result) return result; } catch {} await delay(100); } throw new Error(`Timed out: ${label}`); };
const connect = async (hash) => {
  const target = await wait(async () => { const targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json(); return targets.find((target) => target.type === "page" && (hash ? target.url.includes(hash) : !target.url.includes("#/"))); }, `window ${hash || "main"}`);
  const cdp = connectCdp(target.webSocketDebuggerUrl); await cdp.ready; await cdp.send("Runtime.enable"); await cdp.send("Page.enable"); connections.push(cdp); return cdp;
};
const evaluate = async (cdp, expression, contextId) => { const result = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, ...(contextId ? { contextId } : {}) }); if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text); return result.result.value; };
const clickText = (cdp, label, contextId) => evaluate(cdp, `(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(label)}); if (!b || b.disabled) throw Error('Button unavailable'); b.click(); return true; })()`, contextId);
const view = async (cdp, expectedText) => {
  // Electron isolates this opaque-origin document into its own renderer target.
  const parentId = (await cdp.send("Page.getFrameTree")).frameTree.frame.id;
  const child = await wait(async () => {
    const { targetInfos } = await cdp.send("Target.getTargets");
    const owned = targetInfos.find((target) => target.type === "iframe" && target.url === "about:srcdoc" && target.parentId === parentId);
    if (!owned) return false;
    const targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
    return targets.find((target) => target.id === owned.targetId);
  }, "plugin frame");
  const frame = connectCdp(child.webSocketDebuggerUrl); await frame.ready; await frame.send("Runtime.enable"); connections.push(frame);
  await wait(() => evaluate(frame, `document.body.innerText.includes(${JSON.stringify(expectedText)})`), expectedText);
  assert.equal(await evaluate(frame, "Boolean(window.mangooDesktop)"), false);
  assert.equal(await evaluate(frame, "(() => {try {return parent.document === document} catch {return false}})()"), false);
  return frame;
};
try {
  await wait(() => api("/libraries"), "Core ready");
  const libraryBase = path.join(directory, "managed-library");
  const mediaRoot = path.join(directory, "media");
  await mkdir(mediaRoot);
  await writeFile(path.join(mediaRoot, "fixture.png"), Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64"));
  const created = await createAndActivateManagedLibrary({ baseUrl: `${origin}/api`, token, name: "Fixture Library", databasePath: path.join(libraryBase, "Database"), contentRoots: [{ path: path.join(libraryBase, "Files"), createIfMissing: true }] });
  await addExistingMediaRoot({ baseUrl: `${origin}/api`, token, contextRevision: created.contextRevision, mediaRoot });
  const discovery = mode === "notification" ? await api("/plugins/discover", {}) : null;
  const installation = await api("/plugins");
  if (mode === "notification") assert.ok(installation.items.some((plugin) => plugin.pluginId === "org.example.notification-status"), JSON.stringify(discovery));
  assert.equal(installation.items.some((p) => p.pluginId === "com.mangoo.onedrive"), mode === "plugin");
  electron = spawn(desktopExecutable ? path.resolve(desktopExecutable) : path.join(root, "node_modules/electron/dist/electron.exe"), [`--remote-debugging-port=${debugPort}`, `--user-data-dir=${path.join(directory, "electron-user-data")}`, ...(desktopExecutable ? [] : ["apps/desktop/dist/main.js"]), "--mangoo-visual-harness=1", `--mangoo-visual-run-id=plugin-ui-${mode}`], { cwd: root, env: { ...env, MANGOO_CORE_EXTERNAL: "1", MANGOO_CORE_URL: `${origin}/api`, MANGOO_WEB_URL: origin, MANGOO_VISUAL_HARNESS: "1", MANGOO_VISUAL_OS_BACKDROP: "1", MANGOO_ELECTRON_USER_DATA_DIR: path.join(directory, "electron-user-data") }, windowsHide: true, stdio: "ignore" });
  const main = await connect();
  await wait(() => main.evalJson("Boolean(document.querySelector('.shell-library-switcher-button'))"), "main ready");
  await main.run("document.querySelector('.notification-toggle').click()");
  await wait(() => main.evalJson("Boolean(document.querySelector('#notifications-root'))"), "notification popover");
  let lastNotificationHealth = null;
  const notificationHealth = await wait(async () => {
    const snapshot = await evaluate(main, "window.mangooDesktop.notifications.getCompactSnapshot()");
    lastNotificationHealth = snapshot.syncHealth;
    return snapshot.syncHealth.state === "healthy" ? snapshot.syncHealth : false;
  }, "ordinary Core notifications reach the local notification store").catch((error) => {
    throw new Error(`${error.message}: ${JSON.stringify(lastNotificationHealth)}`, { cause: error });
  });
  assert.equal(await main.evalJson("document.querySelector('#notifications-root').textContent.includes('同步受限')"), false);
  await main.screenshot(path.join(directory, "local-notification-popover.png"));
  await main.run("window.mangooDesktop.openNotificationsWindow()");
  const notifications = await connect("#/notifications");
  await wait(() => notifications.evalJson("Boolean(document.querySelector('#notification-panel-status'))"), "notification center");
  assert.equal(await notifications.evalJson("document.body.textContent.includes('通知同步受限')"), false);
  assert.equal(await notifications.evalJson("Boolean(document.querySelector('.notification-status-health'))"), false);
  if (!installation.items.some((plugin) => plugin.enabledIntent && !plugin.permissionRequired && plugin.contributions?.ui?.views?.some((view) => view.slot === "notification-center"))) {
    assert.equal(await notifications.evalJson("Boolean(document.querySelector('.notification-status-plugins'))"), false);
  }
  await delay(700);
  await notifications.screenshot(path.join(directory, "local-notification-center.png"));
  await notifications.run("window.mangooDesktop.closeNotificationsWindow()");
  connections.splice(connections.indexOf(notifications), 1);
  notifications.close();
  await main.send("Page.bringToFront");
  await main.run("document.querySelector('.notification-toggle').click()");
  await main.run("document.querySelector('.shell-library-switcher-button').click()");
  await wait(() => main.evalJson("Boolean(document.querySelector('.library-switcher'))"), "library menu");
  assert.equal(await main.evalJson("document.querySelector('.library-switcher').textContent.includes('从远端恢复')"), false);
  await main.screenshot(path.join(directory, "local-library-menu.png"));
  await main.run("window.mangooDesktop.openSettingsWindow()");
  const settings = await connect("#/settings");
  await settings.send("Page.bringToFront");
  await wait(() => settings.evalJson("[...document.querySelectorAll('button')].some(b => b.textContent.trim() === '资源库')"), "settings tabs");
  await clickText(settings, "资源库");
  await wait(() => settings.evalJson("document.body.textContent.includes('资源库列表')"), "library settings");
  for (const text of ["从远端恢复", "同步与恢复", "同步文件夹"]) assert.equal(await settings.evalJson(`document.body.textContent.includes(${JSON.stringify(text)})`), false);
  // Secondary windows reveal through a compositor animation after DOM mounting.
  await delay(700);
  await settings.screenshot(path.join(directory, "local-library-settings.png"));
  let pluginSummary = null;
  if (mode === "plugin") {
    await main.run("window.mangooDesktop.openPluginsWindow()");
    const manager = await connect("#/plugins");
    await wait(() => manager.evalJson("Boolean(document.getElementById('plugin-tab-com.mangoo.onedrive'))"), "plugin card");
    await manager.run("document.getElementById('plugin-tab-com.mangoo.onedrive').click()");
    const initial = await api("/plugins/com.mangoo.onedrive");
    if (!initial.enabledIntent) {
      await wait(() => manager.evalJson("Boolean(document.querySelector('[aria-label=\"启用插件\"]:not(:disabled)'))"), "enable toggle");
      await manager.run("document.querySelector('[aria-label=\"启用插件\"]').click()");
    }
    const enabled = await wait(async () => { const p = await api("/plugins/com.mangoo.onedrive"); return p.enabledIntent ? p : false; }, "enabled intent");
    if (enabled.permissionRequired) {
      await wait(() => manager.evalJson("[...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Approve permissions' && !b.disabled)"), "permission approval");
      await clickText(manager, "Approve permissions");
    }
    const current = await wait(async () => { const p = await api("/plugins/com.mangoo.onedrive"); return p.enabledIntent && !p.permissionRequired ? p : false; }, "plugin enabled");
    const entry = current.contributions.ui.views.find((v) => v.slot === "plugin-detail");
    await wait(() => manager.evalJson(`[...document.querySelectorAll('button')].some(b => b.textContent.trim() === ${JSON.stringify(entry.title)})`), "plugin contribution");
    await clickText(manager, entry.title);
    const frame = await view(manager, "OneDrive Sync");
    await wait(() => evaluate(frame, "document.body.innerText.includes('启用此资源库的同步后，可管理同步文件夹。')"), "unbound local plugin view");
    assert.equal(await evaluate(frame, "document.body.innerText.includes('Plugin view operation is not permitted')"), false);
    await manager.screenshot(path.join(directory, "plugin-owned-settings.png"));
    await clickText(frame, "从远端恢复");
    await wait(() => evaluate(frame, "Boolean(document.getElementById('managed-library-wizard'))"), "plugin restore dialog");
    await manager.screenshot(path.join(directory, "plugin-owned-restore.png"));
    await evaluate(frame, "document.querySelector('#managed-library-wizard button').focus()");
    await manager.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await manager.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    assert.ok(await manager.evalJson("Boolean(document.querySelector('.plugin-view-frame'))"), "inner dialog Escape preserves Host view");
    const captured = await evaluate(manager, "window.mangooDesktop.visual.capturePage()");
    assert.ok(Buffer.from(captured, "base64").length > 1000, "native compositor capture");
    // Safe isolated lifecycle test: there is no configured identity/account.
    await api("/plugins/com.mangoo.onedrive/disable", {});
    await clickText(frame, "刷新");
    await wait(() => manager.evalJson("!document.querySelector('.plugin-view-frame')"), "disabled view revocation");
    pluginSummary = { viewId: entry.id, version: current.selectedVersion, lifecycleEpoch: current.lifecycleEpoch, uiSha256: current.contributions.ui.sha256, nativeCapture: true, disabledViewRevoked: true };
  } else if (mode === "notification") {
    const pluginId = "org.example.notification-status";
    const initial = await api(`/plugins/${pluginId}`);
    if (!initial.enabledIntent) await api(`/plugins/${pluginId}/enable`, {});
    const enabled = await api(`/plugins/${pluginId}`);
    if (enabled.permissionRequired) await api(`/plugins/${pluginId}/grants`, { permissions: enabled.requestedPermissions }, "PUT");
    await main.run("window.mangooDesktop.openNotificationsWindow()");
    const center = await connect("#/notifications");
    await wait(() => center.evalJson("Boolean(document.querySelector('.notification-status-plugins'))"), "plugin notification entries");
    assert.equal(await center.evalJson("'sessionToken' in window.mangooDesktop || 'coreUrl' in window.mangooDesktop || 'shell' in window.mangooDesktop"), false);
    await center.send("Page.bringToFront");
    await delay(700);
    await center.screenshot(path.join(directory, "plugin-notification-entry.png"));
    await clickText(center, "Fixture service status");
    const manager = await connect("#/plugins");
    const frame = await view(manager, "Plugin-owned notification status");
    assert.equal(await evaluate(frame, "document.getElementById('library').textContent"), created.library.libraryUuid);
    await manager.screenshot(path.join(directory, "plugin-notification-status.png"));
    connections.splice(connections.indexOf(frame), 1);
    frame.close();
    await api(`/plugins/${pluginId}/disable`, {});
    await manager.run("(() => { const channel = new BroadcastChannel('mangoo-plugin-state'); channel.postMessage({ kind: 'changed', source: 'desktop-acceptance' }); channel.close(); })()");
    await wait(() => manager.evalJson("!document.querySelector('.plugin-view-frame')"), "notification view revoked on disable");
    await center.send("Page.bringToFront");
    await wait(() => center.evalJson("!document.querySelector('.notification-status-plugins')"), "notification entry removed on disable");
    assert.equal(await manager.evalJson("Boolean(document.querySelector('.plugin-view-frame'))"), false);
    pluginSummary = { pluginId, restrictedNotificationBridge: true, pluginOwnedStatus: true, libraryContext: true, disabledViewRevoked: true };
  } else {
    const captured = await evaluate(main, "window.mangooDesktop.visual.capturePage()");
    assert.ok(Buffer.from(captured, "base64").length > 1000);
  }
  const result = { passed: true, mode, directory, packagedDesktop: Boolean(desktopExecutable), libraryUuid: created.library.libraryUuid, notificationHealth, plugin: pluginSummary, noAccountConfigured: true, noRemoteWrites: true };
  await writeFile(path.join(directory, "result.json"), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} catch (error) {
  for (const [index, connection] of connections.entries()) {
    await connection.screenshot(path.join(directory, `failure-${index}.png`)).catch(() => {});
    await writeFile(path.join(directory, `failure-${index}.txt`), await connection.evalJson("document.body.innerText").catch(() => "window unavailable"));
  }
  console.error(`Acceptance evidence: ${directory}`);
  throw error;
} finally {
  for (const connection of connections) connection.close();
  if (electron?.exitCode === null) spawnSync("taskkill", ["/PID", String(electron.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
  core.kill();
  await Promise.race([new Promise((resolve) => core.once("exit", resolve)), delay(5000)]);
  if (core.exitCode === null) spawnSync("taskkill", ["/PID", String(core.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
  await writeFile(path.join(directory, "core.log"), coreLog.replaceAll(token, "[session]"));
}

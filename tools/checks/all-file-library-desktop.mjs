// Actual desktop/Host acceptance using only a fresh local fixture Library.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import sharp from "sharp";
import { connectCdp } from "../dev/cdp-client.mjs";
import { createAndActivateManagedLibrary, addExistingMediaRoot, managedLibraryHostEnv, resolveCoreBinary } from "../dev/managed-library-harness.mjs";

const root = process.cwd();
const output = path.join(root, ".tmp", "all-file-library-validation");
await mkdir(output, { recursive: true });
const directory = await mkdtemp(path.join(output, "desktop-"));
const mediaRoot = path.join(directory, "media");
await mkdir(path.join(mediaRoot, "nested"), { recursive: true });
const raster = { create: { width: 320, height: 180, channels: 3, background: "#458fa8" } };
await sharp(raster).png().toFile(path.join(mediaRoot, "01-image.png"));
await sharp(raster).jpeg().toFile(path.join(mediaRoot, "02-image.JPG"));
await sharp(raster).webp().toFile(path.join(mediaRoot, "03-image.webp"));
const ppm = Buffer.concat([Buffer.from("P6\n1600 900\n255\n"), Buffer.alloc(1600 * 900 * 3)]);
for (let i = ppm.length - 1600 * 900 * 3; i < ppm.length; i += 3) { ppm[i] = 225; ppm[i + 1] = 95; ppm[i + 2] = 60; }
await writeFile(path.join(mediaRoot, "04-decoder.ppm"), ppm);
for (const name of ["05-notes.txt", "06-code.ts", "07-archive.zip", "08-document.pdf", "09-unknown.exampletype", "10-no-extension", ".ordinary-hidden", "nested/nested-note.txt"]) {
  await writeFile(path.join(mediaRoot, name), "Local all-file acceptance fixture\n");
}
await writeFile(path.join(mediaRoot, "11-empty.bin"), "");
const expectedCount = 13;
const freePort = () => new Promise((resolve, reject) => {
  const server = net.createServer(); server.on("error", reject);
  server.listen(0, "127.0.0.1", () => { const port = server.address().port; server.close(() => resolve(port)); });
});
const port = await freePort(), debugPort = await freePort();
const origin = `http://127.0.0.1:${port}`, token = randomUUID();
const env = { ...process.env, ...managedLibraryHostEnv(directory), MANGOO_CORE_ADDR: `127.0.0.1:${port}`,
  MANGOO_SESSION_TOKEN: token, MANGOO_ALLOWED_ORIGIN: origin, MANGOO_SERVE_WEB: "1", MANGOO_WEB_DIR: path.join(root, "apps/web/dist"),
  MANGOO_PLUGINS_DIR: process.env.MANGOO_PLUGINS_DIR ?? path.join(path.dirname(resolveCoreBinary({ root })), "bundled-plugins"), MANGOO_PLUGIN_STATE_DIR: path.join(directory, "plugin-state"), MANGOO_PARENT_PID: String(process.pid) };
delete env.ELECTRON_RUN_AS_NODE;
const core = spawn(resolveCoreBinary({ root }), [], { cwd: root, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
let log = "", desktop;
core.stdout.on("data", (data) => log += data); core.stderr.on("data", (data) => log += data);
const connections = [];
const api = async (route, body, method = body === undefined ? "GET" : "POST") => {
  const response = await fetch(`${origin}/api${route}`, { method, headers: { "x-mangoo-session": token, ...(body === undefined ? {} : { "content-type": "application/json" }) }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await response.text(); assert.ok(response.ok, `${method} ${route}: ${response.status} ${text}`);
  return text ? JSON.parse(text) : null;
};
const wait = async (check, label, timeout = 45000) => {
  const deadline = Date.now() + timeout; let lastError;
  while (Date.now() < deadline) { try { const value = await check(); if (value) return value; } catch (error) { lastError = error; } await delay(100); }
  throw new Error(`Timed out: ${label}${lastError ? `: ${lastError.message}` : ""}`);
};
const evaluate = async (cdp, expression) => {
  const result = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result.value;
};
const connect = async (hash = "") => {
  const target = await wait(async () => {
    const rows = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
    return rows.find((row) => row.type === "page" && (hash ? row.url.includes(hash) : !row.url.includes("#/")));
  }, `desktop ${hash || "main"}`);
  const cdp = connectCdp(target.webSocketDebuggerUrl); await cdp.ready; connections.push(cdp);
  await cdp.send("Runtime.enable"); await cdp.send("Page.enable"); await cdp.send("Log.enable");
  // Observe product requests without introducing requests or changing timing.
  await evaluate(cdp, `(() => {
    window.__allFileRequests = [];
    const original = window.fetch;
    window.fetch = function(input, init) {
      const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, location.href);
      const tracked = /\\/api\\/(media|plugins|roots)(\\/|$)/.test(url.pathname);
      const entry = { at: Date.now(), path: url.pathname + url.search, method: init?.method ?? 'GET' };
      if (tracked) window.__allFileRequests.push(entry);
      return original.call(this, input, init).then(response => {
        if (tracked) {
          entry.status = response.status;
          if (response.headers.get('content-type')?.includes('application/json')) response.clone().json().then(body => {
            entry.rows = (body.items ?? [body]).map(item => ({ id: item.id ?? item.fileId ?? item.pluginId, name: item.name, fileCount: item.fileCount, presentation: item.presentation, thumbnailState: item.thumbnailState, state: item.state, runtimeState: item.runtimeState }));
          }).catch(() => {});
        }
        return response;
      }, error => { if (tracked) entry.error = error.name; throw error; });
    };
  })()`);
  return cdp;
};
const click = (cdp, selector) => evaluate(cdp, `(() => {const e=document.querySelector(${JSON.stringify(selector)}); if(!e || e.disabled) throw Error('Unavailable: '+${JSON.stringify(selector)}); e.click(); return true;})()`);
const tile = (id) => `.media-tile[data-media-id="${id}"]`;
const openPreview = async (cdp, id) => {
  await evaluate(cdp, `document.querySelector(${JSON.stringify(tile(id))}).dispatchEvent(new MouseEvent('dblclick',{bubbles:true}))`);
  await wait(() => evaluate(cdp, "Boolean(document.querySelector('.central-preview-overlay .preview-placeholder'))"), "central preview");
};
const escape = async (cdp) => {
  await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
};
const report = { passed: false, directory, layouts: [], noAccountConfigured: true };
try {
  await wait(() => api("/libraries"), "Core ready");
  const created = await createAndActivateManagedLibrary({ baseUrl: `${origin}/api`, token, name: "All-file acceptance", databasePath: path.join(directory, "Library", "Database"), contentRoots: [{ path: path.join(directory, "Library", "Files"), createIfMissing: true }] });
  const added = await addExistingMediaRoot({ baseUrl: `${origin}/api`, token, contextRevision: created.contextRevision, mediaRoot });
  const media = await wait(async () => { const page = await api("/media?sort=name_asc&limit=100"); return page.items.length === expectedCount ? page.items : false; }, "all ordinary files indexed");
  const unknown = media.find((item) => item.name === "09-unknown.exampletype"), decoded = media.find((item) => item.name === "04-decoder.ppm");
  assert.ok(unknown && decoded); assert.equal(decoded.presentation.thumbnail, "unavailable");
  const search = await api("/search?q=exampletype"); assert.ok(search.items.some((item) => item.id === unknown.id));
  const roots = await api("/roots"); assert.ok(roots.items.some((item) => item.fileCount === expectedCount));
  const before = await api(`/media/${decoded.id}`);
  desktop = spawn(path.join(root, "node_modules/electron/dist/electron.exe"), [`--remote-debugging-port=${debugPort}`, `--user-data-dir=${path.join(directory, "electron-user-data")}`, "apps/desktop/dist/main.js", "--mangoo-visual-harness=1", `--mangoo-visual-run-id=all-file-${Date.now()}`], {
    cwd: root, env: { ...env, MANGOO_CORE_EXTERNAL: "1", MANGOO_CORE_URL: `${origin}/api`, MANGOO_WEB_URL: origin,
      MANGOO_VISUAL_HARNESS: "1", MANGOO_VISUAL_OS_BACKDROP: "1", MANGOO_ELECTRON_USER_DATA_DIR: path.join(directory, "electron-user-data") }, windowsHide: true, stdio: "ignore",
  });
  const main = await connect();
  const fixtureRoot = roots.items.find((item) => item.fileCount === expectedCount);
  const fixtureRootSelector = `.tree-item[data-root-id="${fixtureRoot.id}"] .tree-label`;
  await wait(() => evaluate(main, `Boolean(document.querySelector(${JSON.stringify(fixtureRootSelector)}))`), "fixture root in sidebar");
  await click(main, fixtureRootSelector);
  await wait(() => evaluate(main, `Boolean(document.querySelector(${JSON.stringify(tile(unknown.id))}))`), "unknown file tile");
  for (const [mode, label] of [["adaptive", "Adaptive"], ["waterfall", "Waterfall"], ["grid", "Grid"], ["list", "List"]]) {
    await click(main, '.layout-menu-trigger');
    await wait(() => evaluate(main, "Boolean(document.querySelector('#layout-root [role=option]'))"), "layout menu");
    await evaluate(main, `([...document.querySelectorAll('#layout-root [role=option]')].find(e=>e.textContent.trim()===${JSON.stringify(label)})).click()`);
    await wait(() => evaluate(main, `Boolean(document.querySelector('.media-tile--${mode}'))`), mode);
    const badges = await evaluate(main, `([...document.querySelectorAll('.media-tile')].map(e=>({name:e.getAttribute('aria-label'),badge:e.querySelector('.file-format-badge')?.textContent??null})))`);
    for (const entry of badges) {
      if (/0[12]-image\./.test(entry.name)) assert.equal(entry.badge, null);
      if (entry.name.includes("09-unknown.exampletype")) assert.equal(entry.badge, "EXAMPLETYPE");
    }
    await openPreview(main, unknown.id);
    assert.ok(await evaluate(main, "Boolean(document.querySelector('.central-preview-overlay .preview-placeholder.file-fallback .file-format-artwork'))"));
    await main.screenshot(path.join(directory, `${mode}-unknown-preview.png`));
    await escape(main); await wait(() => evaluate(main, `!document.querySelector('.central-preview-overlay') && Boolean(document.querySelector(${JSON.stringify(tile(unknown.id))}))`), "preview return");
    await main.screenshot(path.join(directory, `${mode}-library.png`)); report.layouts.push(mode);
  }
  await click(main, `.tree-item[data-root-id="${fixtureRoot.id}"] .tree-label`);
  await wait(() => evaluate(main, "Boolean(document.querySelector('.subfolder-card-main[aria-label^=\"Select nested;\"]'))"), "nested folder strip");
  await wait(() => evaluate(main, "Boolean(document.querySelector('.subfolder-card .file-format-artwork'))"), "generic folder cover");
  await evaluate(main, "document.querySelector('.subfolder-card-main[aria-label^=\"Select nested;\"]').dispatchEvent(new MouseEvent('dblclick',{bubbles:true}))");
  const nested = media.find((item) => item.name === "nested-note.txt");
  await wait(() => evaluate(main, `Boolean(document.querySelector(${JSON.stringify(tile(nested.id))}))`), "nested folder contents");
  await main.screenshot(path.join(directory, "nested-folder.png"));
  await click(main, `.tree-item[data-root-id="${fixtureRoot.id}"] .tree-label`);
  await wait(() => evaluate(main, `Boolean(document.querySelector(${JSON.stringify(tile(decoded.id))}))`), "root return");
  report.folderCoverAndNavigation = true;
  // Enable through the actual plugin manager, exercising cross-window invalidation.
  await evaluate(main, "window.mangooDesktop.openPluginsWindow()");
  const manager = await connect("#/plugins");
  await wait(() => evaluate(manager, "Boolean(document.getElementById('plugin-tab-com.mangoo.ppm-decoder'))"), "decoder card");
  await click(manager, '[id="plugin-tab-com.mangoo.ppm-decoder"]');
  await click(manager, '[aria-label="启用插件"]');
  await wait(async () => (await api("/plugins/com.mangoo.ppm-decoder")).enabledIntent, "decoder enabled intent");
  if ((await api("/plugins/com.mangoo.ppm-decoder")).permissionRequired) {
    await wait(() => evaluate(manager, "[...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Approve permissions'&&!e.disabled)"), "decoder permissions");
    await evaluate(manager, "[...document.querySelectorAll('button')].find(e=>e.textContent.trim()==='Approve permissions').click()");
  }
  await main.send("Page.bringToFront");
  await wait(() => evaluate(main, `document.querySelector(${JSON.stringify(tile(decoded.id))})?.getAttribute('data-thumb-state') === 'ready'`), "plugin content appears in existing grid", 60000);
  await openPreview(main, decoded.id);
  const previewDimensions = await wait(() => evaluate(main, "(() => { const image = document.querySelector('.central-preview-overlay .preview-placeholder.ready img'); return image?.complete && image.naturalWidth > 1024 && image.alt === '04-decoder.ppm' ? { width: image.naturalWidth, height: image.naturalHeight } : false; })()"), "decoded larger image on the central preview surface");
  await main.screenshot(path.join(directory, "decoder-preview.png"));
  const after = await api(`/media/${decoded.id}`); assert.equal(after.id, before.id);
  await escape(main);
  await manager.send("Page.bringToFront"); await click(manager, '[aria-label="停用插件"]');
  await wait(async () => !(await api("/plugins/com.mangoo.ppm-decoder")).enabledIntent, "decoder disabled");
  await main.send("Page.bringToFront");
  await wait(() => evaluate(main, `document.querySelector(${JSON.stringify(tile(decoded.id))})?.getAttribute('data-thumb-state') === 'ready'`), "validated image survives disable");
  await writeFile(path.join(mediaRoot, "12-after-disable.ppm"), ppm);
  const newFile = await wait(async () => (await api("/media?limit=100")).items.find((item) => item.name === "12-after-disable.ppm"), "new file collected with decoder disabled");
  assert.equal(newFile.presentation.thumbnail, "unavailable");
  await wait(() => evaluate(main, `(() => {
    const grid = document.querySelector('.virtual-grid');
    grid.scrollTop = grid.scrollHeight;
    const item = document.querySelector(${JSON.stringify(tile(newFile.id))});
    return item?.getAttribute('data-thumb-state') === 'unavailable' && Boolean(item.querySelector('.file-format-artwork')) && !item.querySelector('img, video');
  })()`), "new disabled-decoder file uses artwork in the actual grid");
  await main.screenshot(path.join(directory, "disabled-new-file-artwork.png"));
  const countStartedAt = Date.now();
  await wait(() => evaluate(main, `(() => {
    const sidebar = document.querySelector('.tree-item[data-root-id="${fixtureRoot.id}"] .tree-media-count');
    return sidebar?.getAttribute('aria-label') === '${expectedCount + 1} files';
  })()`), "sidebar count converges after watched file collection", 15000);
  report.countConvergence = { sidebar: expectedCount + 1, delayMs: Date.now() - countStartedAt };
  await main.screenshot(path.join(directory, "counts-converged.png"));
  const capture = await evaluate(main, "window.mangooDesktop.visual.capturePage()");
  const captureBytes = Buffer.from(capture, "base64"); assert.ok(captureBytes.length > 1000);
  await writeFile(path.join(directory, "native-capture.png"), captureBytes);
  report.fileCount = expectedCount; report.root = added; report.searchVisible = true; report.decoder = { enabledFromUi: true, preview: true, previewDimensions, cachedImageSurvivesDisable: true, newFileFallsBack: true, newFileGridArtwork: true, identityPreserved: true };
  report.nativeCaptureBytes = captureBytes.length; report.passed = true;
  await writeFile(path.join(directory, "result.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
} catch (error) {
  report.error = error.stack;
  report.apiState = { plugins: await api("/plugins").catch(() => null), roots: await api("/roots").catch(() => null), media: await api("/media?limit=100").catch(() => null) };
  for (const [index, cdp] of connections.entries()) await writeFile(path.join(directory, `requests-${index}.json`), JSON.stringify(await evaluate(cdp, "(async () => ({ hidden: document.hidden, phase: await window.mangooDesktop.getLibraryPhase(), requests: window.__allFileRequests, tiles: [...document.querySelectorAll('.media-tile')].map(e => ({ id: e.dataset.mediaId, state: e.dataset.thumbState, rect: e.getBoundingClientRect().toJSON() })) }))()").catch(error => ({ error: String(error) })), null, 2));
  for (const [index, cdp] of connections.entries()) { await cdp.screenshot(path.join(directory, `failure-${index}.png`)).catch(() => {}); await writeFile(path.join(directory, `failure-${index}.txt`), await evaluate(cdp, "document.body.innerText").catch(() => "unavailable")); }
  await writeFile(path.join(directory, "result.json"), JSON.stringify(report, null, 2)); console.error(`Evidence: ${directory}`); throw error;
} finally {
  for (const cdp of connections) cdp.close();
  if (desktop?.exitCode === null) spawnSync("taskkill", ["/PID", String(desktop.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
  if (core.exitCode === null) core.kill();
  await Promise.race([new Promise((resolve) => core.once("exit", resolve)), delay(5000)]);
  await writeFile(path.join(directory, "core.log"), log.replaceAll(token, "[session]"));
}

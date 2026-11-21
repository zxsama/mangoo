// Exercise the packaged web bundle, Electron shell and real Core against an isolated host.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFile, cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { once } from "node:events";
import { zipSync, strToU8 } from "fflate";
import sharp from "sharp";
import { connectCdp } from "../dev/cdp-client.mjs";
import { availableLoopbackPort } from "../dev/web-dev-url.mjs";
import {
  managedLibraryHostEnv,
  createAndActivateManagedLibrary,
  resolveCoreBinary,
} from "../dev/managed-library-harness.mjs";

const repo = process.cwd();
const output = path.join(repo, ".tmp", "notebook-desktop");
await mkdir(output, { recursive: true });
const dir = await mkdtemp(path.join(output, "run-"));
const port = await availableLoopbackPort(0);
const debugPort = await availableLoopbackPort(0);
const origin = "http://127.0.0.1:" + port;
const token = randomBytes(32).toString("hex");
const packaged = process.argv.includes("--packaged");
const appDirectory = path.join(repo, ".data", "desktop-release", "win-unpacked");
const binary = packaged ? path.join(appDirectory, "resources", "core", "mangoo-core.exe") : resolveCoreBinary({});
const env = {
  ...process.env,
  ...managedLibraryHostEnv(dir),
  MANGOO_CORE_ADDR: "127.0.0.1:" + port,
  MANGOO_SESSION_TOKEN: token,
  MANGOO_ALLOWED_ORIGIN: origin,
  MANGOO_WEB_DIR: packaged ? path.join(appDirectory, "resources", "web") : path.join(repo, "apps/web/dist"),
  MANGOO_SERVE_WEB: "1",
  MANGOO_PLUGINS_DIR: path.join(path.dirname(binary), "bundled-plugins"),
  MANGOO_PLUGIN_STATE_DIR: path.join(dir, "plugin-state"),
  MANGOO_PARENT_PID: String(process.pid),
};
const isolatedBinary = path.join(dir, path.basename(binary));
await copyFile(binary, isolatedBinary);
const isolatedPlugins = path.join(dir, "bundled-plugins");
await cp(env.MANGOO_PLUGINS_DIR, isolatedPlugins, { recursive: true });
env.MANGOO_PLUGINS_DIR = isolatedPlugins;
const core = spawn(isolatedBinary, [], { env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
const stopped = once(core, "close");
let coreLog = "";
core.stdout.on("data", (b) => (coreLog += b));
core.stderr.on("data", (b) => (coreLog += b));
const api = async (route, method = "GET", body) => {
  const r = await fetch(origin + "/api" + route, {
    method,
    headers: { "x-mangoo-session": token, "content-type": "application/json" },
    body: body && JSON.stringify(body),
  });
  const text = await r.text();
  assert.ok(r.ok, route + " " + r.status + " " + text);
  return text ? JSON.parse(text) : null;
};
const wait = async (fn, label, timeout = 30000) => {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      const ok = await fn();
      if (ok) {
        console.log(`${label} · ${Date.now() - (end - timeout)} ms`);
        return ok;
      }
    } catch (e) {
      last = e;
    }
    await delay(100);
  }
  throw Error("Timed out: " + label + (last ? " · " + last.message : ""));
};
const archives = (obj) =>
  Buffer.from(
    zipSync(Object.fromEntries(Object.entries(obj).map(([key, value]) => [key, strToU8(value)]))),
  ).toString("base64");
// A visible offline reference strip exercises image collection and reading width.
const swatches = await sharp(Buffer.from([41, 57, 64, 75, 100, 99, 124, 151, 137,
  171, 172, 144, 195, 157, 112, 166, 119, 95]), { raw: { width: 6, height: 1, channels: 3 } })
  .resize(510, 36, { kernel: "nearest", fit: "fill" }).png().toBuffer();
const files = [
  [
    "材质评审手册.md",
    Buffer.from([
      "在相同的观察条件下比较陶瓷与织物，记录不同光照下的高光边缘和漫反射过渡。",
      "## 参考色组",
      "![材质参考色](sample.png)",
      "## 评审清单",
      "- [x] 固定相机和参考光源\n- [ ] 补充侧光下的材质对比",
      "> 同一组对比图使用同一套相机和色彩空间设置。",
      "## 取样记录",
      "| 材质 | 观察项 |\n| --- | --- |\n| 陶瓷 | 高光边缘 |\n| 棉布 | 漫反射过渡 |",
    ].join("\n\n")).toString("base64"),
    "text/markdown",
  ],
  [
    "sample.png",
    swatches.toString("base64"),
    "image/png",
  ],
  [
    "网页.html",
    Buffer.from("<!doctype html><h1>网页示例</h1><script>document.title='demo'</script>").toString(
      "base64",
    ),
    "text/html",
  ],
  [
    "电子书.epub",
    archives({
      mimetype: "application/epub+zip",
      "META-INF/container.xml": '<container><rootfile full-path="OEBPS/content.opf"/></container>',
      "OEBPS/content.opf":
        '<package><manifest><item id="c1" href="chapter.xhtml"/></manifest><spine><itemref idref="c1"/></spine></package>',
      "OEBPS/chapter.xhtml": "<html><body><h1>第一章</h1><p>离线阅读</p></body></html>",
    }),
    "application/epub+zip",
  ],
  [
    "计划.docx",
    archives({
      "[Content_Types].xml":
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
      "_rels/.rels":
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>',
      "word/document.xml":
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>目录审阅</w:t></w:r></w:p></w:body></w:document>',
    }),
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  ],
];
let electron, cdp, manager, settingsCdp;
const browser = async (expression) => {
  const r = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails)
    throw Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
};
const click = (label) =>
  browser(
    `(()=>{const el=[...document.querySelectorAll('button')].find(x=>(x.getAttribute('aria-label')===${JSON.stringify(label)} || x.textContent.trim()===${JSON.stringify(label)}) && x.offsetWidth && !x.disabled);if(!el)throw Error('Button unavailable: '+${JSON.stringify(label)});el.click()})()`,
  );
const fill = (selector, text) =>
  browser(
    `(()=>{const el=document.querySelector(${JSON.stringify(selector)});Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,"value").set.call(el,${JSON.stringify(text)});el.dispatchEvent(new Event("input",{bubbles:true}))})()`,
  );
const has = (selector) => browser(`Boolean(document.querySelector(${JSON.stringify(selector)}))`);
const switcher = () => wait(async () => {
  if (await has(".library-switcher-list[aria-busy=false] .library-switcher-main")) return true;
  if (await has(".shell-library-switcher-button[aria-expanded=false]:not(:disabled)")) {
    await titlebarClick("library-switcher");
  }
  return false;
}, "open the ready library switcher");
const leftControls = ["library-switcher", "open-plugins", "open-settings", "locate-folder", "toggle-sidebars"];
const shellControls = [...leftControls, "utility-separator", "notifications", "window-minimize", "window-maximize", "window-close"];
async function verifyButtonIcons(client, selector) {
  const evidence = await client.evalJson(`(() => {
    const svgs = [...document.querySelectorAll(${JSON.stringify(selector)})].filter(svg => svg.checkVisibility());
    const mismatches = [];
    for (const svg of svgs) {
      const path = svg.querySelector('path:last-child');
      const originalStyle = path.getAttribute('style');
      const style = getComputedStyle(path);
      const before = [style.d, style.transform];
      const bold = style.getPropertyValue('--button-icon-path').trim();
      const fit = style.getPropertyValue('--button-icon-transform').trim();
      path.style.d = bold;
      path.style.transform = fit;
      if (!bold || !fit || before[0] !== style.d || before[1] !== style.transform) mismatches.push(svg.dataset.buttonIcon);
      if (originalStyle === null) path.removeAttribute('style'); else path.setAttribute('style', originalStyle);
    }
    return { count: svgs.length, mismatches };
  })()`);
  assert.ok(evidence.count > 0, "expected button icons in " + selector);
  assert.deepEqual(evidence.mismatches, [], "bold artwork and calibrated fit in " + selector);
}
const titlebarGeometry = () => browser(`(() => {
  const selectors = ${JSON.stringify(shellControls)};
  return Object.fromEntries(selectors.map(id => {
    const selector = id === 'utility-separator' ? '.shell-titlebar-utility-separator' : '[data-titlebar-control="' + id + '"]';
    const element = [...document.querySelectorAll(selector)].find(el => el.checkVisibility() && el.offsetWidth);
    const rect = element.getBoundingClientRect();
    return [id, { x: rect.x, y: rect.y, w: rect.width, h: rect.height, region: getComputedStyle(element).webkitAppRegion,
      inLeftHeader: !!element.closest('.workbench-column-left .shell-titlebar-left') }];
  }));
})()`);
async function settledChrome(libraryName) {
  await wait(() => browser(`document.querySelector('.shell-library-switcher-name')?.textContent === ${JSON.stringify(libraryName)}`), "active titlebar library " + libraryName);
  let previous;
  return wait(async () => {
    const current = await titlebarGeometry();
    const isStable = previous && JSON.stringify(previous) === JSON.stringify(current);
    previous = current;
    return isStable && current;
  }, "settled titlebar controls");
}
const titlebarClick = async id => {
  const { x, y } = await browser(`(() => {
    const selector = '[data-titlebar-control="' + ${JSON.stringify(id)} + '"]';
    for (const element of document.querySelectorAll(selector)) {
      const { left, top, width, height } = element.getBoundingClientRect();
      const x = left + width / 2, y = top + height / 2;
      if (width && height && element.contains(document.elementFromPoint(x, y))) return { x, y };
    }
    throw Error('No clickable control ' + selector);
  })()`);
  for (const type of ["mousePressed", "mouseReleased"]) {
    await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  }
};
async function verifyRightChrome() {
  const controls = await browser(`[...document.querySelectorAll('.shell-right-actions [data-titlebar-control]')]
    .filter(button => button.checkVisibility() && button.offsetWidth)
    .map(button => {
      const svg = button.querySelector('svg');
      const icon = svg.getBoundingClientRect();
      const path = svg.querySelector('path:last-child');
      const artwork = path.getBoundingClientRect();
      const hit = button.getBoundingClientRect();
      return {
        id: button.dataset.titlebarControl, path: path.getAttribute('d'), label: button.ariaLabel,
        size: [icon.width, icon.height], hit: [hit.width, hit.height], artworkWidth: artwork.width,
        dx: icon.x + icon.width / 2 - hit.x - hit.width / 2,
        dy: icon.y + icon.height / 2 - hit.y - hit.height / 2
      };
    })`);
  const byId = Object.fromEntries(controls.map(control => [control.id, control]));
  const maximize = byId["window-maximize"];
  assert.ok(["最大化", "还原"].includes(maximize?.label), "expected a visible maximize or restore control");
  for (const [id, size, hitWidth] of [
    ["notifications", 15, 28],
    ["window-minimize", 14, 32],
    ["window-maximize", 13, 32],
    ["window-close", 14, 32],
  ]) {
    const control = byId[id];
    assert.deepEqual(control.size, [size, size], id + " renders at native size");
    assert.deepEqual(control.hit, [hitWidth, 30], id + " retains its hit target");
    assert.ok(Math.abs(control.dx) <= 0.5 && Math.abs(control.dy) <= 0.5, id + " is centered in its button");
    assert.ok(Math.round(control.artworkWidth) >= 9, id + " artwork width was " + control.artworkWidth);
  }
  return maximize;
}
const treeItem = ".workbench-column-body-left .notebook-index .notebook-item";
const dividerGeometry = () => browser(`(() => {
  const shell = document.querySelector('.app-shell').getBoundingClientRect();
  const column = document.querySelector('.workbench-column-left');
  const header = column.querySelector('.shell-titlebar-left').getBoundingClientRect();
  const tree = column.querySelector('.workbench-column-body-left .notebook-index').getBoundingClientRect();
  const center = document.querySelector('.workbench-column-center');
  const rail = document.querySelector('.shell-column-resizer-left');
  const divider = rail.getBoundingClientRect();
  const seam = getComputedStyle(rail, '::after');
  const sample = document.createElement('canvas').getContext('2d');
  sample.fillStyle = seam.backgroundColor;
  sample.fillRect(0, 0, 1, 1);
  const rect = column.getBoundingClientRect();
  const paintedEdge = (element, side) => {
    const css = getComputedStyle(element);
    const color = css.getPropertyValue('border-' + side + '-color');
    return parseFloat(css.getPropertyValue('border-' + side + '-width')) > 0
      && !['none', 'hidden'].includes(css.getPropertyValue('border-' + side + '-style'))
      && color !== 'transparent' && !color.endsWith(', 0)');
  };
  return { x: divider.x + divider.width / 2, y: divider.y + divider.height / 2,
    top: divider.top, bottom: divider.bottom, shellTop: shell.top, shellBottom: shell.bottom,
    columnTop: rect.top, columnBottom: rect.bottom, edge: rect.right, headerEdge: header.right, treeEdge: tree.right,
    headerBottom: header.bottom, treeTop: tree.top, treeWidth: tree.width,
    adjacentBorders: paintedEdge(column, 'right') || paintedEdge(center, 'left'),
    seamWidth: parseFloat(seam.width), hitWidth: divider.width, seamAlpha: sample.getImageData(0, 0, 1, 1).data[3] / 255 };
})()`);
async function dragDivider(offset) {
  const { x, y } = await dividerGeometry();
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x + offset, y, button: "left", buttons: 1 });
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x + offset, y, button: "left", clickCount: 1 });
}
const targetFor = async route => (await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json())
  .find(target => target.type === "page" && target.url.startsWith(origin) && target.url.includes(route));
try {
  await wait(() => api("/libraries"), "Core");
  const media = await createAndActivateManagedLibrary({
    baseUrl: origin + "/api",
    token,
    name: "媒体",
    databasePath: path.join(dir, "MediaDB"),
    contentRoots: [{ path: path.join(dir, "MediaFiles"), createIfMissing: true }],
  });
  await api("/plugins/discover", "POST", {});
  const plugin = "/plugins/com.mangoo.notebook";
  const installed = await api(plugin);
  await api(plugin + "/grants", "PUT", { permissions: installed.requestedPermissions });
  await api(plugin + "/enable", "POST");
  const userData = path.join(dir, "electron");
  electron = spawn(
    packaged ? path.join(appDirectory, "Mangoo.exe") : path.join(repo, "node_modules/electron/dist/electron.exe"),
    [
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${userData}`,
      ...(packaged ? [] : ["apps/desktop/dist/main.js"]),
      "--mangoo-visual-harness=1",
      "--mangoo-visual-run-id=notes-acceptance",
    ],
    {
      env: {
        ...env,
        MANGOO_CORE_EXTERNAL: "1",
        MANGOO_CORE_URL: origin + "/api",
        MANGOO_WEB_URL: origin,
        MANGOO_VISUAL_HARNESS: "1",
        MANGOO_ELECTRON_USER_DATA_DIR: userData,
      },
      windowsHide: true,
      stdio: "ignore",
    },
  );
  const page = await wait(
    async () =>
      (await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json()).find(
        (x) => x.type === "page" && x.url.startsWith(origin),
      ),
    "desktop window",
  );
  cdp = connectCdp(page.webSocketDebuggerUrl);
  await cdp.ready;
  await cdp.send("Runtime.enable");
  await cdp.send("Log.enable");
  await cdp.send("Page.enable");
  await wait(() => has(".shell-library-switcher-button"), "library selector");
  const mediaChrome = await settledChrome("媒体");
  const iconSizes = {
    "open-plugins": 16, "open-settings": 16, "locate-folder": 16, "toggle-sidebars": 16,
    "library-filter": 15, "library-sort": 14, "library-layout": 14
  };
  const renderedIcons = await browser(`(() => {
    const sizes = ${JSON.stringify(iconSizes)};
    return Object.fromEntries(Object.entries(sizes).map(([id, originalSize]) => {
      const selector = '[data-titlebar-control="' + id + '"] svg';
      const svg = [...document.querySelectorAll(selector)].find(el => el.getBoundingClientRect().width);
      const { width, height } = svg.getBoundingClientRect();
      const original = svg.cloneNode(false);
      original.setAttribute('width', originalSize);
      original.setAttribute('height', originalSize);
      original.setAttribute('viewBox', '0 0 256 256');
      original.removeAttribute('preserveAspectRatio');
      svg.replaceWith(original);
      const baseline = original.getBoundingClientRect();
      original.replaceWith(svg);
      return [id, {
        width, height, nominal: [Number(svg.getAttribute('width')), Number(svg.getAttribute('height'))],
        baseline: [baseline.width, baseline.height],
        fit: svg.getAttribute('preserveAspectRatio') ?? 'xMidYMid meet'
      }];
    }));
  })()`);
  for (const [id, size] of Object.entries(iconSizes)) {
    const icon = renderedIcons[id];
    assert.deepEqual(icon.nominal, [size, size], id + " keeps its original SVG dimensions");
    assert.deepEqual([icon.width, icon.height], icon.baseline, id + " keeps its original layout dimensions");
    if (icon.width < size) assert.equal(icon.fit, "xMidYMid meet", id + " keeps the drawing centered and proportional");
  }
  const switcherIcons = await browser(`[...document.querySelectorAll('.shell-library-switcher-button svg')].map(svg => svg.getBoundingClientRect().width).filter(Boolean)`);
  assert.deepEqual(switcherIcons, [16, 14], "Library identity and switcher chevrons retain their sizes");
  const initialCaption = await verifyRightChrome();
  for (const label of [initialCaption.label === "还原" ? "最大化" : "还原", initialCaption.label]) {
    await titlebarClick("window-maximize");
    await wait(() => has('[data-titlebar-control="window-maximize"][aria-label="' + label + '"]'), "window action " + label);
    await settledChrome("媒体");
    const caption = await verifyRightChrome();
    assert.equal(caption.label, label);
    assert.equal(caption.path === initialCaption.path, label === initialCaption.label, "maximize and restore use distinct symbols");
  }
  await cdp.screenshot(path.join(dir, "library-titlebar.png"));
  await cdp.screenshot(path.join(dir, "media.png"));
  await titlebarClick("library-switcher");
  await wait(async () => { await click("创建资源库…"); return true; }, "new media library action");
  await wait(() => browser("document.querySelector('#managed-library-wizard')?.dataset.overlayState === 'open'"), "media creation dialog");
  const disclosure = await browser(`(() => {
    const control = document.querySelector('.managed-library-database-toggle');
    const icon = control.querySelector('.section-disclosure-chevron').getBoundingClientRect();
    const label = control.querySelector('.section-disclosure-label').getBoundingClientRect();
    return { gap: label.left - icon.right, stroke: getComputedStyle(control).borderTopWidth, expanded: control.ariaExpanded };
  })()`);
  assert.ok(disclosure.gap >= 0 && disclosure.gap <= 8 && disclosure.stroke === "0px" && disclosure.expanded === "false");
  await verifyButtonIcons(cdp, "#managed-library-wizard button svg[data-button-icon]");
  await cdp.screenshot(path.join(dir, "create-library.png"));
  await click("取消");
  await wait(async () => !(await has("#managed-library-wizard")), "dismiss media creation");
  if (!(await has(".library-switcher-footer-action"))) await titlebarClick("library-switcher");
  await wait(async () => {
    await click("创建笔记库…");
    return true;
  }, "new notebook action");
  await wait(() => has('#managed-library-wizard input[aria-label="资源库名称"]'), "creation wizard");
  await fill('input[aria-label="资源库名称"]', "工作笔记");
  await browser("document.querySelector('.managed-library-database-toggle').click()");
  await fill("#managed-library-database-editor input", path.join(dir, "NotebookDB"));
  await wait(async () => {
    await click("创建并打开");
    return true;
  }, "submit creation");
  await wait(() => has(".notebook-workspace"), "notebook workspace", 12000);
  const id = (await api("/libraries/active")).library.libraryUuid;
  const notes = "/notebooks/" + id + "/notes";
  const createFolder = async (name) => {
    await wait(async () => { await click("新建文件夹"); return true; }, "start folder " + name);
    await wait(() => has('input[aria-label="文件夹名称"]'), "folder name");
    await fill('input[aria-label="文件夹名称"]', name);
    await click("确认新建文件夹");
    await wait(() => browser(`[...document.querySelectorAll('[role=treeitem][data-folder-id]')].some(node => node.textContent.includes(${JSON.stringify(name)}))`), "folder created " + name);
  };
  await createFolder("项目资料");
  await createFolder("视觉开发");
  await wait(() => has("[data-folder-id][aria-level='2'][aria-label='视觉开发']"), "nested folder position");
  await click("重命名文件夹");
  await fill('input[aria-label="文件夹名称"]', "材质研究");
  await click("确认重命名");
  await wait(() => has("[data-folder-id][aria-label='材质研究']"), "renamed nested folder");
  await createFolder("临时");
  await click("删除空文件夹");
  await click("确认删除空文件夹");
  await wait(async () => !(await has("[data-folder-id][aria-label='临时']")), "removed empty folder");
  await click("新建 Markdown 笔记");
  await wait(() => has(".cm-content"), "live editor");
  await fill('input[aria-label="笔记标题"]', "渲染调研");
  await browser("document.querySelector('.cm-content').focus()");
  await cdp.send("Input.insertText", { text: "# 光照笔记\n\n记录材质与曝光。" });
  await wait(async () => {
    const list = await api(notes);
    return (
      list[0]?.title === "渲染调研" && (await api(notes + "/" + list[0].id)).markdown.includes("曝光")
    );
  }, "autosaved note");
  await browser(
    `(()=>{const data=new DataTransfer(); for(const [name,b64,type] of ${JSON.stringify(files)})data.items.add(new File([Uint8Array.from(atob(b64),v=>v.charCodeAt(0))],name,{type}));const input=document.querySelector('input[type=file][accept]');input.files=data.files;input.dispatchEvent(new Event('change',{bubbles:true}))})()`,
  );
  await wait(
    async () =>
      (await api(notes)).length === 5 &&
      !(await browser("document.querySelector('.notebook-document')?.getAttribute('aria-busy') === 'true'")),
    "completed import",
  );
  const selectNote = async (name) => {
    const id = (await api(notes)).find(note => note.title === name)?.id;
    assert.ok(id, "No stored note " + name);
    await wait(
      async () =>
        browser(
          `(()=>{const el=document.querySelector('[data-note-id="'+${JSON.stringify(id)}+'"][role="treeitem"]:not(:disabled)');if(el && el.offsetWidth)el.click();return !!el?.offsetWidth})()`,
        ),
      "select " + name,
    );
    await wait(
      () => browser(`document.querySelector('.notebook-title')?.value===${JSON.stringify(name)}`),
      "open " + name,
    );
  };
  await fill(".notebook-search input", "曝光");
  await browser("document.querySelector('.notebook-search').requestSubmit()");
  await wait(
    () =>
      browser(
        "document.querySelectorAll('[role=treeitem][data-note-id]').length === 1 && document.querySelector('[role=treeitem][data-note-id]')?.textContent.includes('渲染调研')",
      ),
    "library full-text search",
  );
  await fill(".notebook-search input", "");
  await selectNote("网页");
  await selectNote("材质评审手册");
  await wait(
    () => browser("document.querySelector('.nb-embed img')?.naturalWidth===510"),
    "managed image visible",
  );
  await wait(
    () => browser("document.querySelector('.nb-writing table')?.textContent.includes('陶瓷') && document.querySelectorAll('.nb-writing input[type=checkbox]').length === 2"),
    "live task list and table",
  );
  await wait(() => browser("document.querySelectorAll('.shell-titlebar .notebook-titlebar-tabs [role=tab]').length >= 3 && !document.querySelector('.notebook-document [role=tablist]')"), "multiple tabs inside native titlebar");
  const noteChrome = await settledChrome("工作笔记");
  await verifyRightChrome();
  await writeFile(path.join(dir, "titlebar.json"), JSON.stringify({ media: mediaChrome, notebook: noteChrome }, null, 2));
  for (const id of shellControls) {
    const before = mediaChrome[id], after = noteChrome[id];
    if (leftControls.includes(id) || id === "utility-separator") {
      assert.ok(before.inLeftHeader && after.inLeftHeader, id + " is in the host's left navigation header in both libraries");
    }
    if (id !== "utility-separator") assert.equal(after.region, "no-drag", id + " stays clickable");
    assert.ok(after.w > 0 && after.h > 0, id + " is visible");
    for (const key of ["x", "y", "h", ...(id === "library-switcher" ? [] : ["w"])]) {
      assert.ok(Math.abs(before[key] - after[key]) <= 1, id + " " + key + " moved " + JSON.stringify({ before, after }));
    }
  }
  const tabRow = await browser(`(() => {
    const tab = document.querySelector('.notebook-tablist [aria-selected=true]');
    const bounds = tab.getBoundingClientRect(), y = bounds.y + bounds.height/2;
    const right = document.querySelector('[data-titlebar-control="notifications"]').getBoundingClientRect().left;
    const start = document.querySelector('.notebook-tab-add').getBoundingClientRect().right;
    let dragWidth = 0, run = 0;
    for (let x = Math.ceil(start); x < right; x++) {
      let el = document.elementFromPoint(x, y), region;
      while (el && !(region = ['no-drag','drag'].find(value => value === getComputedStyle(el).webkitAppRegion))) el = el.parentElement;
      run = region === 'drag' ? run + 1 : 0; dragWidth = Math.max(dragWidth, run);
    }
    return { y, dragWidth, region: getComputedStyle(tab).webkitAppRegion };
  })()`);
  assert.equal(tabRow.region, "no-drag");
  assert.ok(Math.abs(tabRow.y - (noteChrome["open-settings"].y + noteChrome["open-settings"].h/2)) <= 1, "Titlebar vertical alignment");
  assert.ok(tabRow.dragWidth >= 16, "Native caption hit area between tabs and window controls: " + JSON.stringify(tabRow));
  await click("文档大纲");
  await wait(() => has(".nb-outline nav button[title='评审清单']"), "outline chapters");
  const geometry = await browser(`(() => {
    const bounds = selector => document.querySelector(selector).getBoundingClientRect();
    return { title: bounds('.notebook-title').left, toolbar: bounds('.nb-editor-tools').left,
      body: bounds('.nb-writing .cm-content').left, right: bounds('.notebook-document').right,
      sidebar: bounds('.notebook-index').width, width: innerWidth };
  })()`);
  assert.ok(Math.abs(geometry.title - geometry.toolbar) <= 2 && Math.abs(geometry.title - geometry.body) <= 2, "Document column alignment " + JSON.stringify(geometry));
  const divider = await dividerGeometry();
  assert.ok(!divider.adjacentBorders && divider.seamWidth === 1 && divider.hitWidth >= 10 &&
    divider.seamAlpha > 0 && divider.seamAlpha < 0.12, "Single low-contrast divider with a full resize target " + JSON.stringify(divider));
  for (const [a, b] of [["columnTop", "shellTop"], ["columnBottom", "shellBottom"], ["top", "shellTop"],
    ["bottom", "shellBottom"], ["edge", "x"], ["edge", "headerEdge"], ["edge", "treeEdge"], ["headerBottom", "treeTop"]]) {
    assert.ok(Math.abs(divider[a] - divider[b]) <= 2, "Continuous host seam: " + a + "/" + b + " " + JSON.stringify(divider));
  }
  const tabInset = () => browser(`(() => {
    const tablist = document.querySelector('.shell-titlebar-center .notebook-tablist').getBoundingClientRect();
    return tablist.left - document.querySelector('.workbench-column-left').getBoundingClientRect().right;
  })()`);
  const insetBefore = await tabInset();
  assert.ok(insetBefore >= -1 && insetBefore <= 20, "Tabs start at the center pane's leading edge: " + insetBefore);
  await dragDivider(36);
  await wait(async () => Math.abs((await dividerGeometry()).treeWidth - divider.treeWidth - 36) <= 2, "drag host divider widens notebook directory");
  await wait(async () => Math.abs(await tabInset() - insetBefore) <= 1, "tabs follow resized host column");
  await cdp.screenshot(path.join(dir, "notebook-resized.png"));
  await dragDivider(-36);
  await wait(async () => Math.abs((await dividerGeometry()).treeWidth - divider.treeWidth) <= 2, "restore host width");
  await titlebarClick("toggle-sidebars");
  await wait(() => browser(`(() => {
    const left = document.querySelector('.workbench-column-left');
    const button = document.querySelector('.shell-titlebar-center .shell-sidebar-toggle[aria-pressed=true]');
    return !!button?.offsetWidth && (!left.checkVisibility({ visibilityProperty: true }) || left.offsetWidth < 2);
  })()`), "collapse directory and expose shared restore control");
  assert.equal(await browser("document.querySelector('.notebook-title').value"), "材质评审手册");
  await cdp.screenshot(path.join(dir, "notebook-collapsed.png"));
  await titlebarClick("toggle-sidebars");
  await wait(() => browser(`document.querySelector('.workbench-column-body-left .notebook-index')?.getBoundingClientRect().width >= ${divider.treeWidth - 2}`), "restore directory and its previous width");
  await cdp.screenshot(path.join(dir, "notebook.png"));
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 760, height: 720, deviceScaleFactor: 1, mobile: false });
  await wait(() => browser("innerWidth===760 && document.querySelector('.nb-doc-editor').clientWidth < 590"), "compact editor");
  await wait(() => browser(`(() => {
    const list = document.querySelector('.notebook-tablist').getBoundingClientRect();
    const tab = document.querySelector('.notebook-tab[data-active=true]').getBoundingClientRect();
    return tab.left >= list.left - 1 && tab.right <= list.right + 1;
  })()`), "current tab and close button remain visible after shrinking");
  assert.ok(await browser("document.documentElement.scrollWidth <= innerWidth + 1 && document.querySelector('.nb-editor-tools').scrollWidth <= document.querySelector('.nb-editor-tools').clientWidth + 1"), "Compact workspace fits");
  assert.ok(await browser(`(() => {
    const bounds = sel => document.querySelector(sel).getBoundingClientRect();
    const tabs = bounds('.notebook-tablist'), plus = bounds('.notebook-tab-add');
    return tabs.width > 32 && tabs.left >= bounds('.shell-titlebar-left [data-titlebar-control=toggle-sidebars]').right
      && tabs.right <= plus.left + 1 && plus.right <= bounds('[data-titlebar-control=notifications]').left
      && bounds('[data-titlebar-control=window-close]').right <= innerWidth;
  })()`), "Compact tabs and shell controls do not overlap");
  await cdp.screenshot(path.join(dir, "compact.png"));
  const overflowScroll = await browser(`(() => {
    const list = document.querySelector('.notebook-tablist');
    const saved = list.scrollLeft;
    list.scrollLeft = 0;
    list.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: 100 }));
    const moved = list.scrollLeft;
    list.scrollLeft = saved;
    return { capacity: list.scrollWidth - list.clientWidth, moved };
  })()`);
  assert.ok(overflowScroll.capacity > 0 && overflowScroll.moved > 0, "Overflowing titlebar tabs scroll by wheel " + JSON.stringify(overflowScroll));
  assert.ok(await browser(`(() => {
    const thumb = document.querySelector('.mangoo-overlay-scrollbar-horizontal[data-scrollbar-visible=true] .mangoo-overlay-scrollbar-thumb');
    return !thumb || thumb.getBoundingClientRect().top >= document.querySelector('.notebook-tab[data-active=true]').getBoundingClientRect().bottom;
  })()`), "Titlebar scrollbar clears tab text and close buttons");
  await cdp.send("Emulation.clearDeviceMetricsOverride");
  await browser("document.querySelector('.notebook-breadcrumb button[title=\"在目录中定位：项目资料\"]').click()");
  await wait(() => has("[data-folder-id][aria-label='项目资料'][aria-selected='true']"), "breadcrumb reveals folder");
  await titlebarClick("locate-folder");
  await wait(() => browser(`(() => {
    const note = document.querySelector('.workbench-column-body-left [role=treeitem][data-note-id][aria-selected=true]');
    if (note?.getAttribute('aria-label') !== '材质评审手册') return false;
    const rect = note.getBoundingClientRect();
    return note.contains(document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2));
  })()`), "titlebar locator reveals current note in the host directory");
  const metadata = await api("/notebooks/" + id + "/tree");
  const parentId = metadata.folders.find(folder => folder.name === "项目资料").id;
  const introId = (await api(notes)).find(note => note.title === "渲染调研").id;
  await browser(`(() => {
    const src = document.querySelector('[data-note-id="${introId}"]');
    const dest = document.querySelector('[data-folder-id="${parentId}"]');
    const data = new DataTransfer();
    for (const [element, type] of [[src,'dragstart'],[dest,'dragover'],[dest,'drop'],[src,'dragend']]) {
      element.dispatchEvent(new DragEvent(type,{ dataTransfer: data, bubbles: true, cancelable: true }));
    }
  })()`);
  await wait(async () => (await api("/notebooks/" + id + "/tree")).noteParents[introId] === parentId, "move note to parent by drag");
  const originalParent = metadata.noteParents[introId];
  await selectNote("渲染调研");
  await click("移动到文件夹");
  await browser(`(() => {
    const field = document.querySelector('select[aria-label="目标文件夹"]');
    field.value = ${JSON.stringify(originalParent)};
    field.dispatchEvent(new Event("change", { bubbles: true }));
  })()`);
  await click("确认移动");
  await wait(async () => (await api("/notebooks/" + id + "/tree")).noteParents[introId] === originalParent, "restore nested note via move toolbar");
  await selectNote("材质评审手册");
  const downloads = path.join(dir, "downloads");
  await mkdir(downloads);
  await cdp.send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: downloads });
  await browser("document.querySelector('[aria-label=导出笔记]').click()");
  const archive = await wait(() => readFile(path.join(downloads, "材质评审手册.zip")), "portable export");
  await browser(`(()=>{const data=new DataTransfer();data.items.add(new File([Uint8Array.from(atob(${JSON.stringify(archive.toString("base64"))}),v=>v.charCodeAt(0))],"手册.zip",{type:"application/zip"}));const input=document.querySelector('input[type=file][accept]');input.files=data.files;input.dispatchEvent(new Event('change',{bubbles:true}))})()`);
  await wait(async()=> {
    const copies = (await api(notes)).filter(item=>item.title==="材质评审手册");
    if(copies.length !== 2) return false;
    const documents = await Promise.all(copies.map(item=>api(notes+"/"+item.id)));
    return documents.every(item=>item.attachments.length===1 && item.markdown.includes(item.attachments[0].id));
  }, "archive round trip");
  for (const [title, expected] of [
    ["电子书.epub", "第一章"],
    ["计划.docx", "目录审阅"],
  ]) {
    await selectNote(title);
    await wait(
      () =>
        browser(
          `document.querySelector('.notebook-reader iframe')?.srcdoc.includes(${JSON.stringify(expected)})`,
        ),
      "inline " + title,
      15000,
    );
  }
  await cdp.screenshot(path.join(dir, "office.png"));
  const activeTab = await browser("document.querySelector('.notebook-tablist [aria-selected=true]').id");
  const previousOrder = await browser("Array.from(document.querySelectorAll('.notebook-tablist [role=tab]'), t=>t.id)");
  const oldIndex = previousOrder.indexOf(activeTab);
  const key = oldIndex > 0 ? "PageUp" : "PageDown";
  await browser("document.querySelector('.notebook-tablist [aria-selected=true]').focus()");
  for (const type of ["keyDown", "keyUp"]) {
    await cdp.send("Input.dispatchKeyEvent", { type, key, code: key, modifiers: 10, windowsVirtualKeyCode: key === "PageUp" ? 33 : 34 });
  }
  await wait(() => browser(`document.querySelector('.notebook-tablist [aria-selected=true]').id === ${JSON.stringify(activeTab)}
    && Array.from(document.querySelectorAll('.notebook-tablist [role=tab]'), t=>t.id).indexOf(${JSON.stringify(activeTab)}) === ${oldIndex + (key === "PageUp" ? -1 : 1)}`), "reorder tabs with document retained");
  const retainedTabs = await browser("Array.from(document.querySelectorAll('.notebook-tablist [role=tab]'), t => t.id)");
  assert.ok(retainedTabs.length >= 3);
  await click("关闭标签页 网页");
  await wait(() => browser(`document.querySelectorAll('.notebook-tablist [role=tab]').length === ${retainedTabs.length - 1} && document.querySelector('.notebook-title').value === '计划.docx'`), "background tab closed");
  const currentTabs = await browser("Array.from(document.querySelectorAll('.notebook-tablist [role=tab]'), t => t.id)");
  const sendCtrl = async (key, code, virtualKey) => {
    const event = { key, code, modifiers: 2, windowsVirtualKeyCode: virtualKey };
    await cdp.send("Input.dispatchKeyEvent", { ...event, type: "keyDown" });
    await cdp.send("Input.dispatchKeyEvent", { ...event, type: "keyUp" });
  };
  await sendCtrl("Tab", "Tab", 9);
  await wait(() => browser("document.querySelector('.notebook-title')?.value !== '计划.docx'"), "cycle document tabs");
  await selectNote("计划.docx");
  const previousPage = await browser("performance.timeOrigin");
  await cdp.send("Page.reload");
  await wait(() => browser(`performance.timeOrigin !== ${previousPage}`), "new desktop document");
  await wait(() => has(treeItem), "restored selection");
  await wait(() => browser("document.querySelector('.notebook-title')?.value === '计划.docx'"), "active tab restored");
  assert.deepEqual(await browser("Array.from(document.querySelectorAll('.notebook-tablist [role=tab]'), t=>t.id)"), currentTabs);
  await browser("document.querySelector('.shell-titlebar [role=tab][aria-selected=true]').focus()");
  await sendCtrl("w", "KeyW", 87);
  await wait(() => browser(`document.querySelectorAll('.notebook-tablist [role=tab]').length === ${currentTabs.length - 1} && !!document.querySelector('.notebook-title') && document.querySelector('.notebook-document')?.getAttribute('aria-busy') === 'false'`), "close active tab without closing window");
  await wait(() => browser("document.activeElement.matches('.notebook-tab-select[aria-selected=true]')"), "keyboard focus follows the next tab");
  for (let count = currentTabs.length - 1; count > 0; count--) {
    await sendCtrl("w", "KeyW", 87);
    await wait(() => browser(`document.querySelectorAll('.notebook-tablist [role=tab]').length === ${count - 1} && document.querySelector('.notebook-document')?.getAttribute('aria-busy') === 'false'`), "close remaining tab");
  }
  await wait(() => has(".notebook-empty-actions"), "empty workspace stays usable");
  await wait(() => browser("document.activeElement.matches('.notebook-tab-add')"), "closing last focused tab returns to new tab button");
  const emptyPage = await browser("performance.timeOrigin");
  await cdp.send("Page.reload");
  await wait(() => browser(`performance.timeOrigin !== ${emptyPage} && !!document.querySelector('.notebook-empty-actions') && !document.querySelector('.notebook-tablist [role=tab]')`), "empty tab state persists after reload");
  if (process.argv.includes("--large")) {
    await browser(`(() => {
      const section = "Reference samples are recorded at the same angle and light intensity.\\n\\n";
      const contents = "# Large notebook\\n\\n" + section.repeat(Math.ceil(5 * 1024 ** 2 / section.length));
      const files = new DataTransfer();
      files.items.add(new File([contents], "长篇编辑验收.md", { type: "text/markdown" }));
      const picker = document.querySelector('input[type=file][accept]');
      picker.files = files.files; picker.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await wait(() => browser("document.querySelector('.notebook-title')?.value === '长篇编辑验收' && document.querySelector('.notebook-document').getAttribute('aria-busy') === 'false' && !!document.querySelector('.cm-content')"), "import and display five MiB Markdown", 90000);
    await browser("document.querySelector('.cm-content').focus()");
    await sendCtrl("End", "End", 35);
    await cdp.send("Input.insertText", { text: "终稿校验" });
    const largeId = (await api(notes)).find(note => note.title === "长篇编辑验收").id;
    await wait(async () => {
      const result = await api(notes + "/" + largeId);
      return Buffer.byteLength(result.markdown) > 5 * 1024 ** 2 && result.markdown.endsWith("终稿校验");
    }, "large source last-line edit persisted", 90000);
  }
  await switcher();
  await wait(
    async () =>
      browser(
        `(()=>{const b=[...document.querySelectorAll('.library-switcher-list[aria-busy=false] .library-switcher-main:not(:disabled)')].find(b=>b.offsetWidth && b.getAttribute('aria-current') !== 'true' && b.textContent.includes('媒体'));if(b)b.click();return !!b})()`,
      ),
    "switch to media",
  );
  await wait(
    async () =>
      (await api("/libraries/active")).library?.libraryUuid === media.library.libraryUuid &&
      !(await has(".notebook-workspace")) && (await browser("document.querySelector('.shell-library-switcher-name')?.textContent === '媒体'")),
    "media restored", 12000,
  );
  await switcher();
  await wait(
    async () =>
      browser(
        "(()=>{const el=[...document.querySelectorAll('.library-switcher-list[aria-busy=false] .library-switcher-main:not(:disabled)')].find(el=>el.offsetWidth && el.getAttribute('aria-current') !== 'true' && el.textContent.includes('工作笔记')); if(el)el.click();return !!el})()",
      ),
    "reopen notebook",
  );
  await wait(() => has(treeItem), "notebook survives switching", 12000);
  await selectNote("渲染调研");
  const storedBeforePrefs = await api(notes + "/" + introId);
  await titlebarClick("open-settings");
  const openSettings = async () => {
    const target = await wait(() => targetFor("#/settings"), "settings opened from notebook titlebar");
    settingsCdp = connectCdp(target.webSocketDebuggerUrl);
    await settingsCdp.ready;
    await wait(() => settingsCdp.evalJson("!!document.querySelector('.settings-window[data-secondary-content-visible=true] .settings-dialog-nav-item')"), "settings navigation visible after native window reveal");
    return target;
  };
  const settings = await openSettings();
  await settingsCdp.send("Page.bringToFront");
  await settingsCdp.send("Runtime.evaluate", { awaitPromise: true, expression: `(async () => {
    await document.fonts.ready;
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  })()` });
  await settingsCdp.screenshot(path.join(dir, "settings-general.png"));
  await verifyButtonIcons(settingsCdp, ".settings-dialog-nav-item svg[data-button-icon]");
  const buttonGeometry = () => settingsCdp.evalJson(`(() => {
    const button = document.querySelector('.host-settings-jumps button');
    const box = button.getBoundingClientRect();
    const icon = button.querySelector('svg');
    const label = button.querySelector('span');
    const text = document.createRange();
    text.selectNodeContents(label);
    return {
      frame: [box.x, box.y, box.width, box.height],
      iconY: icon.getBoundingClientRect().y,
      labelY: text.getBoundingClientRect().y,
      hover: button.matches(':hover'),
      pressed: button.matches(':active'),
      transitionsOff: [icon, label].every(element => getComputedStyle(element).transitionProperty === 'none'),
      moving: button.getAnimations({subtree: true}).some(animation => animation.transitionProperty === 'translate'),
      reduced: matchMedia('(prefers-reduced-motion: reduce)').matches
    };
  })()`);
  const movePointer = (x, y) => settingsCdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await movePointer(4, 4);
  const rest = await wait(async () => {
    const box = await buttonGeometry();
    return !box.hover && !box.moving && box;
  }, "stationary shortcut foreground");
  const [left, top, width, height] = rest.frame;
  const center = { x: left + width / 2, y: top + height / 2 };
  const verifyShortcutMotion = async reduced => {
    const expectPosition = (offset, hover, pressed, message) => wait(async () => {
      const current = await buttonGeometry();
      return current.hover === hover && current.pressed === pressed && current.reduced === reduced &&
        (!reduced || current.transitionsOff) && !current.moving &&
        Math.abs(current.iconY - rest.iconY - offset) < 0.05 &&
        Math.abs(current.labelY - rest.labelY - offset) < 0.05 &&
        current.frame.every((value, index) => value === rest.frame[index]);
    }, message);
    await movePointer(center.x, center.y);
    await expectPosition(-1, true, false, "hover lifts shortcut icon and label by one pixel");
    await settingsCdp.send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", buttons: 1, clickCount: 1, ...center });
    await expectPosition(0, true, true, "press restores the shortcut foreground inside its frame");
    // Release outside the shortcut so this geometry check does not navigate.
    await settingsCdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", buttons: 1, x: 4, y: 4 });
    await settingsCdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", buttons: 0, clickCount: 1, x: 4, y: 4 });
    await expectPosition(0, false, false, "leaving the shortcut restores its foreground");
  };
  await verifyShortcutMotion(rest.reduced);
  try {
    await settingsCdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: rest.reduced ? "no-preference" : "reduce" }] });
    await verifyShortcutMotion(!rest.reduced);
  } finally {
    await settingsCdp.send("Emulation.setEmulatedMedia", { features: [] });
  }
  assert.deepEqual(await settingsCdp.evalJson("[...document.querySelectorAll('.settings-dialog-nav-item')].map(el => el.textContent.trim()).sort()"),
    ["常用","资源库","缩略图","外观","快捷键","诊断","笔记"].sort());
  const sectionOrder = await settingsCdp.evalJson(`(() => {
    const scroll = document.querySelector('.settings-layout-scroll');
    const links = [...document.querySelectorAll('.settings-dialog-nav-item')];
    return {
      links: links.map(link => link.getAttribute('aria-controls')),
      panels: [...scroll.querySelectorAll('.host-settings-category[data-section]')].map(panel => panel.id),
      scrollable: scroll.scrollHeight > scroll.clientHeight,
    };
  })()`);
  assert.deepEqual(sectionOrder.panels, sectionOrder.links, "all available settings share one page in navigation order");
  assert.ok(sectionOrder.scrollable);
  const scrollState = () => settingsCdp.evalJson(`(() => {
    const pane = document.querySelector('.host-settings');
    return { top: pane.scrollTop, end: pane.scrollHeight - pane.clientHeight };
  })()`);
  const wheelAt = (point, deltaY) => settingsCdp.send("Input.dispatchMouseEvent", { type: "mouseWheel", ...point, deltaX: 0, deltaY });
  const sidebarPoint = await settingsCdp.evalJson(`(() => {
    const nav = document.querySelector('.settings-layout-nav').getBoundingClientRect();
    return {x: nav.left + nav.width / 2, y: nav.top + nav.height / 2};
  })()`);
  await wheelAt(sidebarPoint, 200);
  await wait(async () => (await scrollState()).top > 0, "wheel over fixed categories scrolls the form");
  const railPoint = await settingsCdp.evalJson(`(() => {
    const rect = document.querySelector('.mangoo-overlay-scrollbar-vertical').getBoundingClientRect();
    return {x: rect.left + rect.width / 2, y: rect.top + rect.height / 2};
  })()`);
  for (const direction of [1, -1]) {
    let state = await scrollState();
    const reached = () => direction > 0 ? state.end - state.top <= 1 : state.top <= 1;
    for (let step = 0; step < Math.ceil(state.end / 500) + 3 && !reached(); step++) {
      const previous = state.top;
      await wheelAt(railPoint, direction * 500);
      await wait(async () => Math.abs((await scrollState()).top - previous) > 1, "wheel over the scrollbar advances the page");
      state = await scrollState();
    }
    assert.ok(reached(), direction > 0 ? "wheel reaches the final settings row" : "wheel returns to the first row");
  }
  const currentCategory = () => settingsCdp.evalJson("document.querySelector('.settings-dialog-nav-item[aria-current=location]')?.getAttribute('aria-controls')");
  const wheel = await settingsCdp.evalJson(`(() => {
    const view = document.querySelector('.settings-layout-scroll').getBoundingClientRect();
    const target = document.querySelector('#settings-thumbnails').getBoundingClientRect();
    return { x: view.left + view.width / 2, y: view.top + view.height / 2, deltaY: Math.ceil(target.top - view.top) };
  })()`);
  await settingsCdp.send("Input.dispatchMouseEvent", { type: "mouseWheel", deltaX: 0, ...wheel });
  await wait(async () => await currentCategory() === "settings-thumbnails", "wheel across sections updates the sidebar");
  const showCategory = async (id) => {
    const panel = "settings-" + id;
    await settingsCdp.evalJson(`document.querySelector('[aria-controls="${panel}"]').click()`);
    await wait(() => settingsCdp.evalJson(`(() => {
      const target = document.getElementById('${panel}');
      const scroller = target.closest('.settings-layout-scroll');
      const offset = target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      const atBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
      return document.querySelector('[aria-controls="${panel}"]').getAttribute('aria-current') === 'location' &&
        offset >= -1 && (offset <= 24 || (atBottom && offset < scroller.clientHeight));
    })()`), id + " settings scrolled into view");
  };
  await showCategory("thumbnails");
  assert.equal(await settingsCdp.evalJson("document.querySelectorAll('.settings-stat-grid dt').length"), 5);
  assert.equal(await settingsCdp.evalJson("document.querySelectorAll('#settings-thumbnails [role=radiogroup] [role=radio]').length"), 3);
  await wait(() => settingsCdp.evalJson("document.querySelector('.settings-progress-caption')?.textContent.includes('当前资源库没有') && document.querySelector('[role=progressbar]').getAttribute('aria-valuenow') === '0'"), "empty library shows idle thumbnail progress");
  assert.ok(await settingsCdp.evalJson("[...document.querySelectorAll('.host-settings-actions button')].length > 0 && [...document.querySelectorAll('.host-settings-actions button')].every(b => parseFloat(getComputedStyle(b).fontSize) <= 13)"));
  await settingsCdp.screenshot(path.join(dir, "settings-thumbnails.png"));
  await settingsCdp.send("Emulation.setDeviceMetricsOverride", { width: 640, height: 600, deviceScaleFactor: 1, mobile: false });
  assert.ok(await settingsCdp.evalJson("document.querySelector('.settings-layout-scroll').scrollWidth <= document.querySelector('.settings-layout-scroll').clientWidth + 1"));
  await settingsCdp.screenshot(path.join(dir, "thumbnails-compact.png"));
  await settingsCdp.send("Emulation.clearDeviceMetricsOverride");
  await showCategory("appearance");
  assert.ok(await settingsCdp.evalJson("!!document.querySelector('[data-section=appearance] #panel-control-corner-radius')"), "existing appearance settings remain accessible");
  await settingsCdp.screenshot(path.join(dir, "settings-appearance.png"));
  await settingsCdp.evalJson("document.querySelector('#library-grid-gap').closest('.host-settings-group').scrollIntoView({ block: 'start', behavior: 'instant' })");
  const checkParameterLayout = async () => {
    const misaligned = await settingsCdp.evalJson(`(() => {
      const centerY = rect => rect.top + rect.height / 2;
      return [...document.querySelectorAll('.host-settings-parameter')].filter(row => {
        if (!row.getBoundingClientRect().height) return false;
        const input = row.querySelector('input[type=range]');
        const capsule = input.closest('.settings-style-slider-control');
        const style = getComputedStyle(capsule);
        const box = capsule.getBoundingClientRect();
        const labelText = document.createRange();
        labelText.selectNodeContents(input.labels[0]);
        const label = input.labels[0].getBoundingClientRect();
        const field = row.querySelector('.host-settings-parameter-value');
        const number = field.querySelector('input[type=number]');
        const value = field.getBoundingClientRect();
        const gap = value.left - box.right;
        const rowWidth = row.getBoundingClientRect().width;
        return labelText.getClientRects().length !== 1 ||
          (input.id === 'library-grid-gap' && box.width <= rowWidth / 2) ||
          number.id !== input.id + '-value' || gap < 0 || gap > 5 ||
          number.getBoundingClientRect().left - box.right > 12 || label.right > box.left ||
          Math.abs(centerY(label) - centerY(box)) > 1 || Math.abs(centerY(value) - centerY(box)) > 1 ||
          Math.abs(value.right - row.getBoundingClientRect().right) > 1 || box.width < 100 ||
          box.height !== 28 || style.paddingLeft !== '10px' || parseFloat(style.borderTopLeftRadius) < box.height / 2;
      }).map(row => row.querySelector('label').textContent);
    })()`);
    assert.deepEqual(misaligned, [], "each visible setting aligns its label, original capsule and value on one line");
  };
  await checkParameterLayout();
  await settingsCdp.screenshot(path.join(dir, "settings-parameters.png"));
  await settingsCdp.send("Emulation.setDeviceMetricsOverride", { width: 540, height: 600, deviceScaleFactor: 1, mobile: false });
  await settingsCdp.evalJson("document.querySelector('#library-grid-gap').closest('.host-settings-group').scrollIntoView({ block: 'start', behavior: 'instant' })");
  await checkParameterLayout();
  const compactSliders = await settingsCdp.evalJson(`(() => {
    const group = document.querySelector('#library-grid-gap').closest('.host-settings-group');
    const sliders = [...group.querySelectorAll('input[type=range]')].map(el => el.getBoundingClientRect());
    return sliders.every((rect, index) => index === 0 || (rect.left === sliders[0].left && rect.top > sliders[index - 1].bottom));
  })()`);
  assert.ok(compactSliders, "narrow settings place parameters in a single aligned column");
  await settingsCdp.screenshot(path.join(dir, "settings-parameters-compact.png"));
  await settingsCdp.send("Emulation.clearDeviceMetricsOverride");
  await showCategory("appearance");
  const accent = "#d7a6ff";
  const colorPopup = '.color-picker-popup:not([aria-hidden="true"])';
  const originalAccent = await settingsCdp.evalJson("JSON.parse(localStorage.getItem('mangoo.interfaceStyle')).accentColor");
  const storedAccent = () => settingsCdp.evalJson("JSON.parse(localStorage.getItem('mangoo.interfaceStyle')).accentColor");
  const displayedAccent = () => settingsCdp.evalJson("document.querySelector('#theme-accent-color .color-picker-trigger-value').textContent.toLowerCase()");
  const startColorEdit = async (id, value) => {
    const point = await settingsCdp.evalJson(`(() => {
      const trigger = document.getElementById(${JSON.stringify(id)});
      trigger.scrollIntoView({ block: 'nearest', behavior: 'instant' });
      const box = trigger.getBoundingClientRect();
      return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    })()`);
    for (const type of ["mousePressed", "mouseReleased"]) {
      await settingsCdp.send("Input.dispatchMouseEvent", { type, ...point, button: "left", clickCount: 1 });
    }
    const hex = JSON.stringify(colorPopup + ' [aria-label="HEX 颜色"]');
    await wait(() => settingsCdp.evalJson(`!!document.querySelector(${hex}) && document.activeElement === document.querySelector(${hex})`), "color editor receives focus");
    await settingsCdp.evalJson(`(() => {
    const field = document.querySelector(${hex});
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(field, ${JSON.stringify(value)});
    field.dispatchEvent(new Event('input', { bubbles: true }));
    return field.value;
    })()`);
  };
  const liveTabColor = () => browser("getComputedStyle(document.querySelector('.notebook-tab[data-active=true] .notebook-tab-select > svg')).color");
  const originalTabColor = await liveTabColor();
  const finishColor = async (action) => {
    await settingsCdp.evalJson(`document.querySelector(${JSON.stringify(colorPopup + " " + action)}).click()`);
  };
  for (const dismiss of ["close", "escape", "outside", "focus", "blur"]) {
    await startColorEdit("theme-accent-color", accent);
    await wait(async () => await liveTabColor() === "rgb(215, 166, 255)", "preview reaches notebook");
    assert.equal(await displayedAccent(), accent);
    assert.equal(await storedAccent(), originalAccent);
    if (dismiss === "close") {
      await settingsCdp.screenshot(path.join(dir, "color-picker.png"));
      await finishColor(".color-picker-close");
    } else if (dismiss === "escape") {
      for (const type of ["keyDown", "keyUp"]) await settingsCdp.send("Input.dispatchKeyEvent", { type, key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    } else if (dismiss === "outside") {
      const point = await settingsCdp.evalJson(`(() => {
        const r = document.querySelector('.settings-dialog-nav-item[aria-current]').getBoundingClientRect();
        return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
      })()`);
      for (const type of ["mousePressed", "mouseReleased"]) await settingsCdp.send("Input.dispatchMouseEvent", { type, ...point, button: "left", clickCount: 1 });
    } else if (dismiss === "focus") {
      await settingsCdp.evalJson("document.querySelector('.settings-dialog-nav-item[aria-current]').focus()");
    } else {
      await cdp.send("Page.bringToFront");
    }
    await wait(async () => (await liveTabColor()) === originalTabColor && await settingsCdp.evalJson("document.querySelector('#theme-accent-color').getAttribute('aria-expanded') === 'false'"), dismiss + " restores confirmed color across windows");
    assert.equal(await storedAccent(), originalAccent);
    if (dismiss === "blur") await settingsCdp.send("Page.bringToFront");
  }
  await startColorEdit("theme-accent-color", accent);
  await finishColor(".color-picker-confirm");
  await wait(async () => await storedAccent() === accent, "confirm persists accent");
  await wait(() => browser("getComputedStyle(document.querySelector('.notebook-tab[data-active=true] .notebook-tab-select > svg')).color === 'rgb(215, 166, 255)'"), "custom theme recolors an active notebook tab in the main window");
  assert.equal(await settingsCdp.evalJson("getComputedStyle(document.querySelector('.settings-layout-nav-item[data-active=true] svg')).color"), "rgb(215, 166, 255)");
  assert.equal(await settingsCdp.evalJson("document.querySelector('.host-settings-role-colors').open"), false);
  await settingsCdp.evalJson("document.querySelector('.host-settings-role-colors summary').click()");
  await startColorEdit("theme-role-success", "#89c5fd");
  await finishColor(".color-picker-confirm");
  await wait(() => browser("getComputedStyle(document.querySelector('.notebook-save-state[data-status=saved] svg')).color === 'rgb(137, 197, 253)'"), "status color changes independently of the theme in a plugin");
  await verifyButtonIcons(cdp, ".notebook-workspace button svg[data-button-icon]");
  await cdp.screenshot(path.join(dir, "notebook-accent.png"));
  await settingsCdp.evalJson("document.querySelector('.host-settings-role-colors summary').click()");
  await settingsCdp.screenshot(path.join(dir, "appearance-accent.png"));
  const slider = await settingsCdp.evalJson(`(() => {
    const el = document.querySelector('#library-grid-gap');
    el.scrollIntoView({ block: 'center', behavior: 'instant' });
    const r = el.getBoundingClientRect();
    return { x: r.x + r.width * 0.8, y: r.y + r.height / 2, value: el.value };
  })()`);
  async function clickSlider(clickCount) {
    for (const type of ["mousePressed", "mouseReleased"]) {
      await settingsCdp.send("Input.dispatchMouseEvent", { type, x: slider.x, y: slider.y, button: "left", clickCount });
    }
  }
  const gapValue = () => settingsCdp.evalJson("document.querySelector('#library-grid-gap').value");
  await settingsCdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: slider.x, y: slider.y });
  await wait(() => settingsCdp.evalJson(`(() => {
    const input = document.getElementById('library-grid-gap');
    const capsule = input.closest('.settings-style-slider-control');
    const rect = capsule.getBoundingClientRect();
    const style = getComputedStyle(capsule);
    return !input.hasAttribute('data-interactive-pointer-target') &&
      capsule.getAttribute('data-interactive-pointer-target') === 'true' &&
      Number(style.getPropertyValue('--interactive-pointer-opacity')) > 0 &&
      Math.abs(parseFloat(style.getPropertyValue('--interactive-pointer-x')) - (${slider.x} - rect.left)) < 1;
  })()`), "original capsule carries a single cursor-following highlight");
  await clickSlider(1);
  await wait(async () => (await gapValue()) !== slider.value, "single click adjusts grid spacing");
  assert.equal(await settingsCdp.evalJson("document.getElementById('library-grid-gap-value').value"), await gapValue(),
    "moving the slider updates the numeric editor");
  await clickSlider(2);
  await wait(async () => (await gapValue()) === slider.value, "double-click restores only the slider default");
  const pressSettingKey = async (key, code) => {
    for (const type of ["keyDown", "keyUp"]) {
      await settingsCdp.send("Input.dispatchKeyEvent", { type, key, code: key, windowsVirtualKeyCode: code });
    }
  };
  await pressSettingKey("ArrowRight", 39);
  await wait(async () => (await gapValue()) !== slider.value, "arrow key adjusts the focused slider");
  await pressSettingKey("Enter", 13);
  await wait(async () => (await gapValue()) === slider.value, "Enter restores the focused slider default");
  const typeNumber = async (id, text) => {
    await settingsCdp.evalJson(`(() => {
      const field = document.getElementById(${JSON.stringify(id + "-value")});
      field.focus();
      field.select();
    })()`);
    if (text) await settingsCdp.send("Input.insertText", { text });
    else await pressSettingKey("Backspace", 8);
  };
  const expectParameter = async (id, value) => {
    await wait(() => settingsCdp.evalJson(`(() => {
      const range = document.getElementById(${JSON.stringify(id)});
      return range.valueAsNumber === ${value} && document.getElementById(range.id + '-value').valueAsNumber === ${value};
    })()`), id + " slider and numeric field show " + value);
  };
  await typeNumber("library-grid-gap", "12");
  await pressSettingKey("Enter", 13);
  await expectParameter("library-grid-gap", 12);
  await typeNumber("library-grid-gap", "8");
  await pressSettingKey("Escape", 27);
  await expectParameter("library-grid-gap", 12);
  await typeNumber("library-grid-gap", "");
  await pressSettingKey("Tab", 9);
  await expectParameter("library-grid-gap", 12);
  await typeNumber("library-grid-gap", "999");
  await pressSettingKey("Tab", 9);
  await expectParameter("library-grid-gap", Number(await settingsCdp.evalJson("document.getElementById('library-grid-gap').max")));
  await typeNumber("library-grid-gap", slider.value);
  await pressSettingKey("Enter", 13);
  await expectParameter("library-grid-gap", Number(slider.value));
  const blurControl = await settingsCdp.evalJson(`(() => {
    const input = document.getElementById('side-blur');
    const group = input.closest('details');
    const open = group.open;
    group.open = true;
    return { value: input.value, open };
  })()`);
  await typeNumber("side-blur", "0.73");
  await pressSettingKey("Enter", 13);
  await expectParameter("side-blur", 0.75);
  await typeNumber("side-blur", blurControl.value);
  await pressSettingKey("Enter", 13);
  await expectParameter("side-blur", Number(blurControl.value));
  if (!blurControl.open) await settingsCdp.evalJson("document.getElementById('side-blur').closest('details').open = false");
  const caption = await settingsCdp.evalJson(`(() => {
    const titlebar = document.querySelector('.secondary-window-titlebar');
    const scrollChain = [];
    for (let el = titlebar.parentElement; el; el = el.parentElement) {
      if (el.scrollTop) scrollChain.push([el.id || el.className || el.tagName, el.scrollTop]);
    }
    return { top: titlebar.getBoundingClientRect().top, scrollChain };
  })()`);
  assert.ok(caption.top >= 0 && caption.top < 3, "settings titlebar stays anchored when a slider receives focus: " + JSON.stringify(caption));
  assert.equal(await displayedAccent(), accent);
  const showNotebookSettings = async () => {
    await showCategory("notebook");
    await wait(() => settingsCdp.evalJson("!!document.querySelector('[data-notebook-settings] [role=radiogroup]')"), "notebook preference controls");
  };
  await showNotebookSettings();
  const chooseMarkdownMode = (mode) => settingsCdp.evalJson(`(() => {
    const group = document.querySelector('[aria-label="Markdown 默认视图"]');
    const radio = [...group.querySelectorAll('[role=radio]')].find(el => el.textContent.trim() === ${JSON.stringify(mode)});
    radio.click(); return true;
  })()`);
  await chooseMarkdownMode("源码");
  await wait(() => has(".nb-source .cm-lineNumbers"), "changing default mode updates open note across windows");
  const wrapSelector = '[data-notebook-settings] input[aria-label="源码自动换行"]';
  await settingsCdp.evalJson(`document.querySelector(${JSON.stringify(wrapSelector)}).click()`);
  await wait(async () => !(await has(".nb-source .cm-lineWrapping")), "source wrap changed live");
  await chooseMarkdownMode("实时写作");
  await wait(() => has(".nb-writing .cm-lineWrapping"), "live Markdown retains writing wrap");
  await settingsCdp.evalJson("document.querySelector('[role=switch][aria-label=默认展开文档大纲]').click()");
  await wait(() => has(".nb-outline"), "default outline updates an unmodified tab");
  await settingsCdp.evalJson("document.querySelector('[role=switch][aria-label=默认展开文档大纲]').click()");
  await settingsCdp.evalJson("[...document.querySelector('[aria-label=\"HTML 默认视图\"]').querySelectorAll('[role=radio]')].find(el => el.textContent.trim() === '预览').click()");
  await selectNote("网页");
  await wait(() => has(".nb-html-output iframe") , "HTML preference opens the rendered webpage");
  assert.ok(await has(".nb-source[hidden]"), "HTML code pane starts collapsed");
  await selectNote("渲染调研");
  const weightSelect = root => `[...document.querySelector(${JSON.stringify(root)}).querySelectorAll('label')].find(label => label.firstChild?.textContent?.trim() === '字重')?.control`;
  const setWeight = (root, weight) => `(() => {
    const select = ${weightSelect(root)};
    select.value = ${JSON.stringify(String(weight))};
    select.dispatchEvent(new Event('change', {bubbles:true})); return true;
  })()`;
  const matchesWeight = weight => browser(`getComputedStyle(document.querySelector('.nb-writing .cm-content')).fontWeight === ${JSON.stringify(String(weight))}`);
  await settingsCdp.evalJson(setWeight("[data-notebook-settings]", 700));
  await wait(() => matchesWeight(700), "Markdown inherits device default weight");
  assert.equal((await api(notes + "/" + introId)).revision, storedBeforePrefs.revision, "device defaults do not rewrite a note");
  await settingsCdp.screenshot(path.join(dir, "settings-notebook.png"));
  await settingsCdp.send("Emulation.setDeviceMetricsOverride", { width: 640, height: 600, deviceScaleFactor: 1, mobile: false });
  await wait(() => settingsCdp.evalJson(`(() => {
    const scroll = document.querySelector('.settings-layout-scroll');
    const section = document.querySelector('[data-notebook-settings]');
    const top = section.closest('.host-settings-category').getBoundingClientRect().top - scroll.getBoundingClientRect().top;
    return innerWidth === 640 && scroll.scrollWidth <= scroll.clientWidth + 1 && section.getBoundingClientRect().right <= innerWidth &&
      document.querySelector('[aria-controls="settings-notebook"]').getAttribute('aria-current') === 'location' && Math.abs(top) < 30;
  })()`), "resizing preserves the notebook heading, navigation and reading column");
  await settingsCdp.screenshot(path.join(dir, "settings-compact.png"));
  await settingsCdp.send("Emulation.clearDeviceMetricsOverride");
  await click("元素样式");
  await wait(() => has("#notebook-element-styles select"), "note-specific style controls");
  await browser(setWeight("#notebook-element-styles", 400));
  await settingsCdp.evalJson(setWeight("[data-notebook-settings]", 900));
  await wait(async () => (await api(notes + "/" + introId)).styles?.body?.fontWeight === 400 && (await matchesWeight(400)), "saved per-note weight takes precedence");
  await cdp.screenshot(path.join(dir, "note-style.png"));
  await browser("document.querySelector('#notebook-element-styles .nb-reset').click()");
  await wait(() => matchesWeight(900), "resetting a note element reveals device preference");
  await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  settingsCdp.close();
  await cdp.send("Target.closeTarget", { targetId: settings.id });
  await wait(async () => !(await targetFor("#/settings")), "closed settings window");
  await cdp.send("Page.bringToFront");
  await titlebarClick("open-settings");
  await openSettings();
  await showCategory("appearance");
  assert.equal(await displayedAccent(), accent);
  await showNotebookSettings();
  assert.equal(await settingsCdp.evalJson(`document.querySelector(${JSON.stringify(wrapSelector)}).checked`), false);
  assert.equal(await settingsCdp.evalJson(`(${weightSelect("[data-notebook-settings]")}).value`), "900");
  await settingsCdp.evalJson("[...document.querySelector('[data-notebook-settings]').querySelectorAll('button')].find(el=>el.textContent.trim()==='恢复笔记默认设置').click()");
  await wait(() => matchesWeight(400), "reset restores the built-in notebook defaults in the editor");
  await cdp.send("Page.bringToFront");
  await titlebarClick("open-plugins");
  const managerTarget = await wait(() => targetFor("#/plugins"), "plugins opened from notebook titlebar");
  manager = connectCdp(managerTarget.webSocketDebuggerUrl);
  await manager.ready;
  await wait(
    () => manager.evalJson("Boolean(document.getElementById('plugin-tab-com.mangoo.notebook'))"),
    "notebook plugin card",
  );
  await manager.run("document.getElementById('plugin-tab-com.mangoo.notebook').click()");
  await wait(
    () => manager.evalJson("Boolean(document.querySelector('.plugin-toggle input:checked'))"),
    "enabled toggle",
  );
  await manager.run("document.querySelector('.plugin-toggle input:checked').click()");
  await wait(
    async () => !(await api(plugin)).enabledIntent && !(await has(".notebook-workspace")),
    "live disable", 12000,
  );
  assert.equal((await api("/libraries/active")).library, null);
  await wait(() => settingsCdp.evalJson("!document.querySelector('[data-notebook-settings]') && ![...document.querySelectorAll('.settings-dialog-nav-item')].some(el=>el.textContent.trim()==='笔记')"), "disabling notebook removes its settings from the open window", 15000);
  const fallbackSection = await currentCategory();
  assert.ok(fallbackSection && fallbackSection !== "settings-notebook", "a visible section takes over after notebook is disabled");
  await cdp.send("Page.bringToFront");
  await switcher();
  await wait(() => has(".library-switcher-main"), "disabled selector");
  assert.equal(
    await browser("document.querySelector('.library-switcher').innerText.includes('工作笔记')"),
    false,
  );
  assert.equal(
    await browser("document.querySelector('.library-switcher').innerText.includes('创建笔记库')"),
    false,
  );
  await manager.evalJson("document.querySelector('.plugin-toggle input:not(:checked)').click()");
  await wait(() => settingsCdp.evalJson("!!document.querySelector('.settings-dialog-nav-item[aria-controls=settings-notebook]') && !!document.querySelector('#settings-notebook')"), "re-enabling restores notebook navigation and content", 15000);
  assert.equal(await currentCategory(), fallbackSection, "restoring notebook leaves the reader in the same section");
  await showNotebookSettings();
  console.log("PASS notebook desktop", dir);
} catch (cause) {
  if (settingsCdp) {
    await settingsCdp.screenshot(path.join(dir, "settings-failure.png")).catch(() => {});
    console.error(await settingsCdp.evalJson(`(() => {
      const scroll = document.querySelector('.settings-layout-scroll');
      return { width: innerWidth, windowFocused: document.hasFocus(), activeElement: document.activeElement?.outerHTML?.slice(0, 300),
        activeSection: document.querySelector('.settings-dialog-nav-item[aria-current=location]')?.textContent,
        openPicker: document.querySelector('.color-picker-popup:not([aria-hidden=true])')?.getAttribute('aria-label'),
        scroll: scroll && [scroll.clientWidth, scroll.scrollWidth], overflow:
        [...document.querySelectorAll('.settings-layout-scroll *')]
          .filter(el => el.getBoundingClientRect().width && el.getBoundingClientRect().right > innerWidth + 1)
          .slice(0, 20).map(el => [el.tagName, el.className, el.getBoundingClientRect().right]) };
    })()`).catch(() => null));
  }
  if (cdp) {
    await cdp.screenshot(path.join(dir, "failure.png")).catch(() => {});
    console.error(await browser("document.body.innerText").catch(() => ""), cdp.consoleErrors);
    console.error(await browser("performance.getEntriesByType('resource').filter(e=>e.name.includes('/libraries') || e.duration>3000).slice(-30).map(e=>[new URL(e.name).pathname,e.startTime,e.duration,e.responseStatus])").catch(()=>[]));
  }
  console.error(dir);
  throw cause;
} finally {
  settingsCdp?.close();
  manager?.close();
  cdp?.close();
  electron?.kill();
  core.kill();
  await stopped;
  await rm(isolatedBinary, { force: true });
  assert.equal(path.dirname(path.resolve(isolatedPlugins)), dir);
  await rm(isolatedPlugins, { recursive: true, force: true });
  await writeFile(path.join(dir, "core.log"), coreLog);
}

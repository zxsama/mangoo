import { app, BrowserWindow } from "electron";
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

async function main() {
  const url = process.argv[2];
  const reportDir = process.env.NOTEBOOK_READER_REPORT;
  app.setPath("userData", path.join(reportDir, "profile"));
  const outcomes = {};
  const warnings = [];
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 900, height: 720,
    webPreferences: { sandbox: true, contextIsolation: true, backgroundThrottling: false } });
  const contents = window.webContents;
  contents.debugger.attach("1.3");
  const send = (method,params = {}) => contents.debugger.sendCommand(method,params);
  async function frameAccessibility() {
    // In production Chromium can assign the opaque iframe its own renderer target.
    const { targetInfos } = await send("Target.getTargets");
    const target = targetInfos.find(item => item.type === "iframe" && item.url === "about:srcdoc");
    if (target) {
      const { sessionId } = await send("Target.attachToTarget",{targetId:target.targetId,flatten:true});
      try { return (await contents.debugger.sendCommand("Accessibility.getFullAXTree",{},sessionId)).nodes; }
      finally { await send("Target.detachFromTarget",{sessionId}); }
    }
    const { frameTree } = await send("Page.getFrameTree");
    const frame = frameTree.childFrames?.find(item => item.frame.name === "mangoo-notebook-preview")?.frame;
    return frame ? (await send("Accessibility.getFullAXTree",{frameId:frame.id})).nodes : [];
  }
  contents.on("console-message", ({ level, message }) => { if (level === "warning" || level === "error" || message?.includes("Warning:")) warnings.push(message); });
  const read = expression => contents.executeJavaScript(expression);
  async function wait(probe, label) {
    const end = Date.now() + 9000;
    let last;
    while (Date.now() < end) {
      try { const result = await probe(); if (result) return result; } catch (e) { last = e; }
      const error = await read("document.querySelector('[role=alert]')?.textContent");
      if (error) throw Error(error);
      await delay(40);
    }
    throw Error(label + " " + String(last || "") + " / " + await read("document.body.innerText.slice(-180)"));
  }
  try {
    await window.loadURL(url);
    await wait(() => read("typeof window.showReader === 'function'"), "reader fixture");
    const resources = await read("window.checkReaderResources()");
    const cases = {
      "writer.docx": ["Notebook Word fixture"], "writer.doc": ["Notebook Word fixture"],
      "ledger.xlsx": ["Northern stock", "Southern stock"], "ledger.xls": ["Northern stock", "Southern stock"],
      "slides.pptx": ["First plate", "Second plate"], "slides.ppt": ["First plate", "Second plate"],
      "book.epub": ["第一章样本", "第二章样本"], "preview-budget.epub": [], "cjk.pdf": []
    };
    for (const [filename, pages] of Object.entries(cases)) {
      try {
        await read(`window.showReader(${JSON.stringify(filename)})`);
        await wait(() => read(`document.querySelector('.notebook-reader')?.ariaLabel === ${JSON.stringify(filename)}`), filename);
        if (filename === "preview-budget.epub") {
          await wait(() => read("document.querySelector('[role=alert]')?.innerText.includes('可下载原文件')"), "oversized EPUB fallback");
          const destination = path.join(reportDir, filename);
          const download = new Promise((resolve, reject) => {
            const deadline = setTimeout(() => reject(Error("Original file download timed out")), 9000);
            contents.session.once("will-download", (_event, item) => {
              item.setSavePath(destination);
              item.once("done", (_done, state) => {
                clearTimeout(deadline);
                if (state === "completed") resolve(); else reject(Error(state));
              });
            });
          });
          await Promise.all([download, read("document.querySelector('a[download]').click()")]);
          const source = await fetch(new URL("/reader-fixtures/" + filename, url));
          assert.deepEqual(await readFile(destination), Buffer.from(await source.arrayBuffer()), "unmodified EPUB download");
          outcomes[filename] = { ok: true, previewFallback: true, originalVerified: true };
        } else if (filename === "cjk.pdf") {
          const ink = `(() => {
            const c = document.querySelector('canvas');
            if (!c || c.width < 400) return false;
            const pixels = c.getContext('2d').getImageData(0,0,c.width,c.height).data;
            let dark=0; for(let i=0;i<pixels.length;i+=4) if(pixels[i]<80 && pixels[i+3]>200) dark++;
            return dark>40 ? dark : false;
          })()`;
          const first = await wait(() => read(ink), "CJK PDF ink");
          await writeFile(path.join(reportDir,"cjk-page.png"), (await contents.capturePage()).toPNG());
          await read("document.querySelector('.notebook-pdf button:last-child').click()");
          const second = await wait(async () => {
            if (!await read("document.querySelector('canvas')?.ariaLabel === 'PDF 第 2 页'")) return false;
            const count = await read(ink); return count && count !== first ? count : false;
          }, "second PDF page");
          outcomes[filename] = { ok: true, ink: [first, second] };
        } else {
          for (const [index, expected] of pages.entries()) {
            if (index) await read(`(()=>{const s=document.querySelector('select');s.value='${index}';s.dispatchEvent(new Event('change',{bubbles:true}))})()`);
            await wait(async () => {
              // Inspect the accessibility tree of the painted page without executing script in its sandbox.
              const nodes = await frameAccessibility();
              if (!nodes.some(n => n.role?.value === "StaticText" && n.name?.value.includes(expected))) return false;
              return filename !== "book.epub" || index === 0 || nodes.some(n => n.role?.value === "image" && n.name?.value === "点");
            }, expected);
          }
          outcomes[filename] = { ok: true, pages };
        }
      } catch (cause) { outcomes[filename] = { ok: false, error: String(cause) }; }
      console.log(filename, JSON.stringify(outcomes[filename]));
    }
    await writeFile(path.join(reportDir, "report.json"), JSON.stringify({ outcomes, warnings, resources }, null, 2));
    if (path.basename(reportDir) === "production") assert.ok(resources.every(item => item.cache.includes("immutable")));
    assert.ok(Object.values(outcomes).every(item => item.ok));
  } catch (cause) { console.error(cause); process.exitCode = 1; }
  finally { window.destroy(); app.exit(process.exitCode || 0); }
}
main().catch(error => { console.error(error); app.exit(1); });

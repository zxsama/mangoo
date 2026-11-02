import { app, BrowserWindow } from "electron";
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let window;
const evidence = [];
const outputDir = process.env.MANGOO_SINGLE_OUTLINE_CAPTURE_DIR;
async function main() {
try {
  await app.whenReady();
  window = new BrowserWindow({ show: true, opacity: 0, skipTaskbar: true, width: 800, height: 800, webPreferences: { backgroundThrottling: false } });
  await window.loadURL(process.argv[2]);
  const run = (source) => window.webContents.executeJavaScript(source, true);
  await delay(500);
  window.webContents.debugger.attach("1.3");
  // This fixture asserts renderer focus styling, not native window activation.
  // Keep unrelated desktop focus changes from resetting :focus-visible midway;
  // native activation and acrylic are verified by the separate desktop pass.
  await window.webContents.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: true });
  const key = async (key) => {
    window.focus();
    await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { type: "keyDown", key, code: key, windowsVirtualKeyCode: key === "Tab" ? 9 : 27 });
    await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { type: "keyUp", key, code: key });
    await delay(60);
  };
  const sample = (selector) => run(`(() => {
    const el=document.querySelector(${JSON.stringify(selector)});
    const read=(node,pseudo)=>{const c=getComputedStyle(node,pseudo);return {border:c.borderTopWidth,borderStyle:c.borderTopStyle,borderColor:c.borderTopColor,outline:c.outlineWidth,outlineStyle:c.outlineStyle,offset:c.outlineOffset,shadow:c.boxShadow,top:c.top,left:c.left,opacity:c.opacity,content:c.content};};
    return {element:read(el),before:read(el,'::before'),after:read(el,'::after'),focus:el.matches(':focus-visible'),rect:el.getBoundingClientRect().toJSON()};
  })()`);
  const capture = async (name) => {
    if (!outputDir) return;
    await mkdir(outputDir, { recursive: true });
    await writeFile(path.join(outputDir, `${name}.png`), (await window.webContents.capturePage()).toPNG());
  };
  const checkShadows = async () => {
    const bad = await run(`Array.from(document.querySelectorAll('body *')).flatMap(el=>[null,'::before','::after'].flatMap(p=>{
      const s=getComputedStyle(el,p).boxShadow;
      return /inset/.test(s)||s.split(/,(?![^()]*\\))/).some(v=>/0px 0px 0px [1-9]/.test(v)) ? [el.className+':'+p+':'+s]:[];
    }))`);
    assert.deepEqual(bad, [], "no control bevels or shadow rings in computed cascade");
  };
  for (const mode of ["normal", "reduced-motion", "forced-colors"]) {
    await window.webContents.debugger.sendCommand("Emulation.setEmulatedMedia", { features: mode === "normal" ? [] : [{ name: mode === "forced-colors" ? "forced-colors" : "prefers-reduced-motion", value: mode === "forced-colors" ? "active" : "reduce" }] });
    await key("Tab");
    for (const id of ["glass", "primary", "ordinary", "default", "borderless", "tab", "swatch"]) {
      await key("Tab");
      await run(`document.getElementById('${id}').focus()`);
      await delay(180);
      const state = await sample(`#${id}`);
      assert.equal(state.focus, true, `${mode}/${id} keyboard focus`);
      assert.equal(state.element.outline, "1px", `${mode}/${id} single thin focus`);
      assert.equal(state.element.offset, "-1px", `${mode}/${id} focus shares boundary band`);
      assert.ok(Number.parseFloat(state.element.border) <= 1, `${mode}/${id} no thick native border`);
      assert.ok(["none", "solid"].includes(state.element.borderStyle), `${mode}/${id} no native bevel`);
      assert.equal(state.after.border, "0px", `${mode}/${id} pseudo adds no border`);
      evidence.push({ mode, id, state });
    }
    for (const id of ["field", "plugin-field"]) {
      await run(`document.getElementById('${id}').focus()`);
      await delay(180);
      const inner = await sample(`#${id}`);
      const wrapper = await sample(`.overlay-field-control:has(#${id})`);
      assert.equal(inner.element.outline, "0px", `${mode}/${id} wrapper owns focus`);
      assert.equal(inner.element.shadow, "none");
      assert.equal(wrapper.element.border, "1px");
      assert.equal(wrapper.element.outline, "1px", `${mode}/${id} wrapper focus stays visible even with error state`);
      evidence.push({ mode, id, inner, wrapper });
    }
    await run("document.getElementById('toggle').focus()");
    await delay(180);
    const toggle = await sample(".plugin-toggle-track");
    assert.equal(toggle.element.outline, "1px");
    assert.equal(toggle.element.offset, "-1px");
    await checkShadows();
    await capture(mode);
  }
  await window.webContents.debugger.sendCommand("Emulation.setEmulatedMedia", { features: [] });
  await run("document.activeElement.blur()");
  window.webContents.sendInputEvent({ type: "mouseMove", x: 760, y: 700 });
  await delay(240);
  const edgeRect = (await sample("#glass")).rect;
  const beforeEdge = await window.webContents.capturePage();
  window.webContents.sendInputEvent({ type: "mouseMove", x: Math.round(edgeRect.left + edgeRect.width / 2), y: Math.floor(edgeRect.top) - 2 });
  await delay(240);
  const afterEdge = await window.webContents.capturePage();
  const beforePixels = beforeEdge.toBitmap();
  const afterPixels = afterEdge.toBitmap();
  const width = afterEdge.getSize().width;
  let changedBorderPixels = 0;
  const y = Math.floor(edgeRect.top);
  for (let x = Math.ceil(edgeRect.left + 12); x < Math.floor(edgeRect.right - 12); x++) {
    const index = (y * width + x) * 4;
    if ([0, 1, 2].reduce((sum, c) => sum + Math.abs(beforePixels[index + c] - afterPixels[index + c]), 0) > 10) changedBorderPixels++;
  }
  assert.ok(changedBorderPixels > 5, `pointer edge actually paints on native border band (${changedBorderPixels} pixels)`);
  evidence.push({ mode: "pointer-paint", changedBorderPixels });
  await capture("pointer-edge-unfocused");
  await run("document.getElementById('glass').focus()");
  const rect = (await sample("#glass")).rect;
  window.webContents.sendInputEvent({ type: "mouseMove", x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) });
  await delay(240);
  const pointer = await sample("#glass");
  assert.equal(pointer.after.left, "-1px", "pointer edge aligns with native border, not inside it");
  assert.equal(pointer.after.top, "-1px");
  assert.ok(Number(pointer.after.opacity) > 0, "global pointer gradient remains active");
  assert.equal(pointer.element.outline, "1px");
  evidence.push({ mode: "pointer-and-focus", pointer });
  await capture("pointer-and-focus");
  await checkShadows();
  if (outputDir) await writeFile(path.join(outputDir, "computed-states.json"), JSON.stringify(evidence, null, 2));
  console.log("SINGLE_OUTLINE_OK");
  window.destroy();
  app.exit(0);
} catch (error) {
  console.error(error);
  window?.destroy();
  app.exit(1);
}
}
void main();

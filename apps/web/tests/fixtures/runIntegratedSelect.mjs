import { app, BrowserWindow } from "electron";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

app.commandLine.appendSwitch("disable-gpu");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ width: 1000, height: 760, show: true, opacity: 0, skipTaskbar: true,
    webPreferences: { contextIsolation: true, sandbox: true, backgroundThrottling: false } });
  await window.loadURL(process.argv[2]);
  window.focus();
  window.webContents.debugger.attach("1.3");
  let handled = 0;
  const deadline = Date.now() + 55_000;
  while (Date.now() < deadline) {
    const state = await window.webContents.executeJavaScript('({ result: window.integratedSelectResult, request: window.integratedSelectInput })');
    if (state.result) {
      if (process.env.MANGOO_SELECT_REPORT) {
        await mkdir(path.dirname(process.env.MANGOO_SELECT_REPORT), { recursive: true });
        await writeFile(process.env.MANGOO_SELECT_REPORT, JSON.stringify(state.result, null, 2));
      }
      process.stdout.write(`INTEGRATED_SELECT_RESULT:${JSON.stringify(state.result)}\n`, () => { window.destroy(); app.exit(0); });
      return;
    }
    const request = state.request;
    if (request && request.id !== handled) {
      if (request.type === "capture") {
        if (process.env.MANGOO_SELECT_SCREENSHOTS) {
          await mkdir(process.env.MANGOO_SELECT_SCREENSHOTS, { recursive: true });
          await writeFile(path.join(process.env.MANGOO_SELECT_SCREENSHOTS, `${request.name}.png`), (await window.webContents.capturePage()).toPNG());
        }
      } else if (request.type === "motion") {
        await window.webContents.debugger.sendCommand("Emulation.setEmulatedMedia", {
          features: [{ name: "prefers-reduced-motion", value: request.reduced ? "reduce" : "no-preference" }]
        });
      } else if (request.type === "key") {
        const key = request.key;
        const codes = { Tab: 9, Enter: 13, Escape: 27, ArrowUp: 38, ArrowDown: 40, Home: 36, End: 35, " ": 32 };
        const params = { key, code: key === " " ? "Space" : key.length === 1 ? `Key${key.toUpperCase()}` : key,
          windowsVirtualKeyCode: codes[key] ?? key.toUpperCase().charCodeAt(0), modifiers: request.shift ? 8 : 0 };
        await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { ...params, type: "keyDown", text: key.length === 1 ? key : "" });
        await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { ...params, type: "keyUp" });
      } else {
        const params = { x: request.x, y: request.y };
        await window.webContents.debugger.sendCommand("Input.dispatchMouseEvent", { ...params, type: "mouseMoved" });
        if (request.type === "click") {
          await window.webContents.debugger.sendCommand("Input.dispatchMouseEvent", { ...params, type: "mousePressed", button: "left", clickCount: 1 });
          await window.webContents.debugger.sendCommand("Input.dispatchMouseEvent", { ...params, type: "mouseReleased", button: "left", clickCount: 1 });
        }
      }
      handled = request.id;
      await window.webContents.executeJavaScript(`window.integratedSelectAck = ${handled}`);
    }
    await delay(12);
  }
  throw new Error("Integrated select fixture timed out");
}
main().catch((error) => process.stdout.write(`INTEGRATED_SELECT_RESULT:${JSON.stringify({ ok: false, error: String(error.stack ?? error) })}\n`, () => app.exit(1)));

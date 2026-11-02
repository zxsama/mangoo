import { app, BrowserWindow } from "electron";
import { writeFile } from "node:fs/promises";
import path from "node:path";

app.commandLine.appendSwitch("disable-gpu");
app.setPath("userData", path.join(process.env.MANGOO_VIDEO_HOVER_EVIDENCE, "electron-profile"));
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let window;
async function main() {
  try {
    await app.whenReady();
    window = new BrowserWindow({ show: true, opacity: 0, skipTaskbar: true, width: 800, height: 600,
      webPreferences: { backgroundThrottling: false, contextIsolation: true, sandbox: true } });
    await window.loadURL(process.argv[2]);
    window.focus();
    const memory = { baselineWorkingSetKiB: 0, peakWorkingSetKiB: 0, peakSampledWorkingSetKiB: 0, peakPrivateKiB: 0, samples: 0 };
    const deadline = Date.now() + 90_000;
    let result = null, handled = 0;
    while (Date.now() < deadline && !result) {
      const metrics = app.getAppMetrics().filter((metric) => metric.type === "Tab");
      const workingSet = metrics.reduce((total, metric) => total + metric.memory.workingSetSize, 0);
      const peakWorkingSet = metrics.reduce((total, metric) => total + metric.memory.peakWorkingSetSize, 0);
      const privateBytes = metrics.reduce((total, metric) => total + (metric.memory.privateBytes ?? 0), 0);
      if (!memory.samples) memory.baselineWorkingSetKiB = workingSet;
      memory.peakSampledWorkingSetKiB = Math.max(memory.peakSampledWorkingSetKiB, workingSet);
      memory.peakWorkingSetKiB = Math.max(memory.peakWorkingSetKiB, peakWorkingSet);
      memory.peakPrivateKiB = Math.max(memory.peakPrivateKiB, privateBytes);
      memory.samples++;
      const state = await window.webContents.executeJavaScript("({ result: window.__videoHoverResult, input: window.__videoHoverInput })", true);
      result = state.result;
      if (state.input && handled !== state.input.id) {
        if (state.input.type === "screenshot") {
          const capture = await window.webContents.capturePage();
          await writeFile(path.join(process.env.MANGOO_VIDEO_HOVER_EVIDENCE, `${state.input.name}.png`), capture.toPNG());
        } else if (state.input.type === "hidden") {
          window.hide(); await delay(180); window.show(); window.focus();
        } else throw new Error(`Unknown fixture input ${state.input.type}`);
        handled = state.input.id;
        await window.webContents.executeJavaScript(`window.__videoHoverInputDone = ${handled}`, true);
      }
      if (!result) await delay(50);
    }
    process.stdout.write(`VIDEO_HOVER_RESULT:${JSON.stringify({ ...(result ?? { ok: false, error: "Video hover timed out" }), rendererMemory: memory })}\n`, () => { window.destroy(); app.exit(0); });
  } catch (error) {
    process.stdout.write(`VIDEO_HOVER_RESULT:${JSON.stringify({ ok: false, error: error.stack ?? String(error) })}\n`, () => { window?.destroy(); app.exit(1); });
  }
}
void main();

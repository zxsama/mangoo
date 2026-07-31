import { app, BrowserWindow } from "electron";

app.commandLine.appendSwitch("disable-gpu");

const fixtureUrl = process.argv[2];
const resultExpression = "window.__overlayLifecycleResult ?? null";

async function main() {
  await app.whenReady();
  const window = new BrowserWindow({
    show: true,
    opacity: 0,
    skipTaskbar: true,
    width: 800,
    height: 600,
    webPreferences: {
      backgroundThrottling: false,
      contextIsolation: true,
      sandbox: true
    }
  });

  await window.loadURL(fixtureUrl);
  window.focus();
  const deadline = Date.now() + 30_000;
  let result = null;
  while (Date.now() < deadline && result === null) {
    result = await window.webContents.executeJavaScript(resultExpression, true);
    if (result === null) {
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
  }

  const output = result ?? {
    ok: false,
    error: "overlay lifecycle fixture timed out"
  };
  process.stdout.write(`OVERLAY_LIFECYCLE_RESULT:${JSON.stringify(output)}\n`, () => {
    window.destroy();
    app.exit(0);
  });
}

main().catch((error) => {
  process.stdout.write(`OVERLAY_LIFECYCLE_RESULT:${JSON.stringify({
    ok: false,
    error: error instanceof Error ? error.stack ?? error.message : String(error)
  })}\n`, () => app.exit(0));
});

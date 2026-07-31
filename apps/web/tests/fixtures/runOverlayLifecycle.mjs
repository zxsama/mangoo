import { app, BrowserWindow } from "electron";

app.commandLine.appendSwitch("disable-gpu");

const fixtureUrl = process.argv[2];
const stateExpression = `({
  result: window.__overlayLifecycleResult ?? null,
  inputRequest: window.__overlayLifecycleInputRequest ?? null
})`;

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function dispatchNativeInput(window, request) {
  if (request.type === "key") {
    window.webContents.sendInputEvent({
      type: "keyDown",
      keyCode: request.keyCode,
      modifiers: request.modifiers ?? []
    });
    window.webContents.sendInputEvent({
      type: "keyUp",
      keyCode: request.keyCode,
      modifiers: request.modifiers ?? []
    });
  } else if (request.type === "mouse-click") {
    window.webContents.sendInputEvent({ type: "mouseMove", x: request.x, y: request.y });
    window.webContents.sendInputEvent({
      type: "mouseDown",
      button: "left",
      clickCount: 1,
      x: request.x,
      y: request.y
    });
    await delay(16);
    window.webContents.sendInputEvent({
      type: "mouseUp",
      button: "left",
      clickCount: 1,
      x: request.x,
      y: request.y
    });
  } else if (request.type === "mouse-move") {
    window.webContents.sendInputEvent({ type: "mouseMove", x: request.x, y: request.y });
  } else {
    throw new Error(`unknown overlay lifecycle input request: ${request.type}`);
  }
  await delay(30);
  await window.webContents.executeJavaScript(
    `window.__overlayLifecycleInputResponseId = ${JSON.stringify(request.id)}`,
    true
  );
}

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
  window.show();
  window.focus();
  await delay(100);
  const deadline = Date.now() + 30_000;
  let result = null;
  let handledInputId = 0;
  while (Date.now() < deadline && result === null) {
    const state = await window.webContents.executeJavaScript(stateExpression, true);
    result = state.result;
    if (state.inputRequest && state.inputRequest.id !== handledInputId) {
      await dispatchNativeInput(window, state.inputRequest);
      handledInputId = state.inputRequest.id;
    }
    if (result === null) {
      await delay(25);
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

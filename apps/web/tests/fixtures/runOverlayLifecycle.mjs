import { app, BrowserWindow } from "electron";

app.commandLine.appendSwitch("disable-gpu");

const fixtureUrl = process.argv[2];
const stateExpression = `({
  result: window.__overlayLifecycleResult ?? null,
  inputRequest: window.__overlayLifecycleInputRequest ?? null
})`;

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function dispatchNativeInput(window, request) {
  if (request.type === "window-hidden-cycle") {
    window.hide();
    await delay(180);
    window.show();
    window.focus();
  } else if (request.type === "cdp-key") {
    if (!window.webContents.debugger.isAttached()) window.webContents.debugger.attach("1.3");
    const key = request.keyCode;
    const windowsVirtualKeyCode = { Tab: 9, Enter: 13, Escape: 27 }[key];
    await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { type: "keyDown", key, code: key, windowsVirtualKeyCode, text: key === "Enter" ? "\r" : "" });
    await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { type: "keyUp", key, code: key, windowsVirtualKeyCode });
  } else if (request.type === "key") {
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
  } else if (request.type === "mouse-drag") {
    window.webContents.sendInputEvent({ type: "mouseMove", x: request.x, y: request.y });
    window.webContents.sendInputEvent({ type: "mouseDown", button: "left", clickCount: 1, x: request.x, y: request.y });
    await delay(30);
    window.webContents.sendInputEvent({ type: "mouseMove", x: request.endX, y: request.endY });
    await delay(30);
    window.webContents.sendInputEvent({ type: "mouseUp", button: "left", clickCount: 1, x: request.endX, y: request.endY });
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
  // The registration follow-up visits all 502 choices with native input plus
  // grouped navigation, search, custom colors and every appearance entrypoint.
  // The previous 384-choice sweep took about 80s; keep this larger sweep bounded
  // without changing the ordinary overlay lifecycle deadline.
  const deadline = Date.now() + (new URL(fixtureUrl).searchParams.has("followup") ? 140_000 : 30_000);
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

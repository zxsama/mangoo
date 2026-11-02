// Inspect the running Mangoo UI through CDP. Captures:
// 1. window.mangooDesktop bridge presence
// 2. number of media tiles in the DOM
// 3. number of <img> elements rendered
// 4. console errors / network failures
//
// Usage: node tools/dev/cdp-inspect.mjs

import { WebSocket } from "ws";

const PAGE_WS = process.argv[2];
if (!PAGE_WS) {
  console.error("Usage: cdp-inspect.mjs <page-ws-url>");
  process.exit(2);
}

let nextId = 1;
const pending = new Map();
const events = [];

const ws = new WebSocket(PAGE_WS);

function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

ws.on("message", (data) => {
  const msg = JSON.parse(data.toString());
  if (msg.id !== undefined) {
    const handler = pending.get(msg.id);
    if (handler) {
      pending.delete(msg.id);
      if (msg.error) handler.reject(msg.error);
      else handler.resolve(msg.result);
    }
  } else {
    events.push(msg);
  }
});

ws.on("open", async () => {
  await send("Runtime.enable");
  await send("Network.enable");
  await send("Log.enable");

  // give the UI a moment in case we just connected
  await new Promise((r) => setTimeout(r, 1000));

  const evalSnippet = `
    (function() {
      const bridge = window.mangooDesktop ?? null;
      const tiles = document.querySelectorAll(".media-tile").length;
      const tiles_ready = document.querySelectorAll(".tile-thumb-ready").length;
      const tiles_loading = document.querySelectorAll(".tile-thumb-loading").length;
      const tiles_failed = document.querySelectorAll(".tile-thumb-failed").length;
      const imgs = document.querySelectorAll(".tile-thumb-image").length;
      const grid = document.querySelector(".virtual-grid");
      const onboard = document.querySelector(".onboarding-hero");
      const sidebar_root_count = (() => {
        const el = document.querySelector(".panel-subtitle");
        return el ? el.textContent : null;
      })();
      const errorStrip = document.querySelector(".error-strip");
      return JSON.stringify({
        hasBridge: !!bridge,
        coreUrl: bridge ? bridge.coreUrl : null,
        hasToken: bridge ? !!bridge.sessionToken : null,
        tiles, tiles_ready, tiles_loading, tiles_failed, imgs,
        gridPresent: !!grid,
        onboardPresent: !!onboard,
        sidebar_root_count,
        errorText: errorStrip ? errorStrip.textContent : null
      });
    })()`;

  const result = await send("Runtime.evaluate", {
    expression: evalSnippet,
    returnByValue: true
  });
  console.log("DOM:", result.result.value);

  // Print recent console errors / warnings
  const consoleEvents = events
    .filter((e) => e.method === "Runtime.consoleAPICalled" || e.method === "Log.entryAdded")
    .slice(-20);
  if (consoleEvents.length === 0) {
    console.log("CONSOLE: (no events captured during this short window)");
  } else {
    for (const ev of consoleEvents) {
      if (ev.method === "Runtime.consoleAPICalled") {
        const args = ev.params.args.map((a) => a.value ?? a.description ?? a.unserializableValue ?? "?");
        console.log(`CONSOLE ${ev.params.type}:`, ...args);
      } else {
        console.log(`LOG ${ev.params.entry.level}:`, ev.params.entry.text);
      }
    }
  }

  ws.close();
  process.exit(0);
});

ws.on("error", (err) => {
  console.error("ws error:", err);
  process.exit(1);
});

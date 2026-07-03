// Desktop CDP scroll-trace smoke. Drives the running Electron renderer, scrolls
// the virtual grid, and asserts render-thread health (no long tasks over budget,
// no tiles stuck loading/failed, no console errors).
//
// Terminal 1: MEGLE_REMOTE_DEBUG=1 MEGLE_AUTO_ADD_ROOT="Y:/Repository/Billfish/Artists" npm run dev
// Terminal 2:
//   WS=$(curl -sS http://127.0.0.1:9222/json | python -c "import json,sys; print([p for p in json.load(sys.stdin) if p['type']=='page'][0]['webSocketDebuggerUrl'])")
//   node tools/dev/desktop-scroll-trace.mjs "$WS"

import { WebSocket } from "ws";
import { assessScrollTrace } from "./hot-loop/traceAssertions.mjs";

const PAGE_WS = process.argv[2];
if (!PAGE_WS) {
  console.error("Usage: desktop-scroll-trace.mjs <page-ws-url>");
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
  await send("Log.enable");

  await send("Runtime.evaluate", {
    expression: `
      window.__megleLongTasks = [];
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) window.__megleLongTasks.push(entry.duration);
      }).observe({ entryTypes: ["longtask"] });
      true;
    `,
    returnByValue: true
  });

  for (let i = 0; i < 30; i++) {
    await send("Runtime.evaluate", {
      expression: 'const g=document.querySelector(".virtual-grid"); if(g){g.scrollTop += 2000;} true;',
      returnByValue: true
    });
    await new Promise((r) => setTimeout(r, 120));
  }

  await new Promise((r) => setTimeout(r, 2000));

  const evalSnippet = `JSON.stringify({
    longTasks: window.__megleLongTasks || [],
    finalTiles: {
      ready: document.querySelectorAll(".tile-thumb-ready").length,
      loading: document.querySelectorAll(".tile-thumb-loading").length,
      failed: document.querySelectorAll(".tile-thumb-failed").length
    }
  })`;
  const result = await send("Runtime.evaluate", { expression: evalSnippet, returnByValue: true });
  const parsed = JSON.parse(result.result.value);

  const consoleErrors = events
    .filter(
      (e) =>
        (e.method === "Runtime.consoleAPICalled" && e.params.type === "error") ||
        (e.method === "Log.entryAdded" && e.params.entry.level === "error")
    )
    .map((e) =>
      e.method === "Runtime.consoleAPICalled"
        ? e.params.args.map((a) => a.value ?? a.description ?? a.unserializableValue ?? "?").join(" ")
        : e.params.entry.text
    );

  const trace = { ...parsed, consoleErrors };

  const budget = { maxLongTaskMs: Number(process.env.MEGLE_TRACE_MAX_LONGTASK_MS ?? 100) };
  const verdict = assessScrollTrace(trace, budget);
  console.log("[scroll-trace] trace:", JSON.stringify(trace));
  console.log("[scroll-trace] verdict:", JSON.stringify(verdict));
  ws.close();
  process.exit(verdict.ok ? 0 : 1);
});

ws.on("error", (err) => {
  console.error("ws error:", err);
  process.exit(1);
});

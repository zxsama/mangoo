// Minimal CDP-over-WebSocket client for driving the running desktop renderer.
// Shared by the real desktop interaction tests. No framework, just the few
// Runtime/Page/Log methods those tests need.
import { WebSocket } from "ws";

export function connectCdp(pageWsUrl) {
  const socket = new WebSocket(pageWsUrl);
  let nextId = 1;
  const pending = new Map();
  const consoleErrors = [];

  socket.on("message", (data) => {
    const message = JSON.parse(data.toString());
    if (message.id !== undefined && pending.has(message.id)) {
      const handler = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) handler.reject(message.error);
      else handler.resolve(message.result);
      return;
    }
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") {
      consoleErrors.push(message.params.entry.text.slice(0, 160));
    }
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") {
      const text = message.params.args
        .map((arg) => arg.value ?? arg.description ?? arg.unserializableValue ?? "?")
        .join(" ");
      consoleErrors.push(text.slice(0, 160));
    }
  });

  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });

  return {
    ready: new Promise((resolve, reject) => {
      socket.on("open", resolve);
      socket.on("error", reject);
    }),
    send,
    consoleErrors,
    // Evaluate an expression that returns a JSON-serializable value; the caller
    // passes a raw JS expression, we wrap it in JSON.stringify and parse back.
    async evalJson(expression) {
      const result = await send("Runtime.evaluate", {
        expression: `JSON.stringify(${expression})`,
        returnByValue: true
      });
      if (result.exceptionDetails) {
        throw new Error(`eval failed: ${result.exceptionDetails.text}`);
      }
      const value = result.result.value;
      return value === undefined ? undefined : JSON.parse(value);
    },
    // Fire-and-forget side-effecting JS (clicks, scroll, key events).
    async run(expression) {
      await send("Runtime.evaluate", { expression: `${expression}; true`, returnByValue: true });
    },
    async screenshot(filePath) {
      const shot = await send("Page.captureScreenshot", { format: "png" });
      (await import("node:fs")).writeFileSync(filePath, Buffer.from(shot.data, "base64"));
    },
    close() {
      socket.close();
    }
  };
}

// Resolve the real page target (not omnibox/devtools internals) on a CDP port.
export async function resolvePageWsUrl(debugPort) {
  const response = await fetch(`http://127.0.0.1:${debugPort}/json`);
  const targets = await response.json();
  const page = targets.find((t) => t.type === "page" && !/omnibox|devtools/.test(t.url));
  return page?.webSocketDebuggerUrl ?? null;
}

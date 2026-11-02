import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after, before, it } from "node:test";
import { createServer, type ViteDevServer } from "./viteTestServer.mts";

const webRoot = fileURLToPath(new URL("../", import.meta.url));
let server: ViteDevServer;
let result: { ok: boolean; error?: string; cases: string[]; evidence: unknown };
before(async () => {
  server = await createServer({ root: webRoot, logLevel: "error", server: { host: "127.0.0.1", port: 0 } });
  await server.listen();
  const address = server.httpServer?.address(); assert.ok(address && typeof address === "object");
  const electron = await import("electron");
  const env = { ...process.env }; delete env.ELECTRON_RUN_AS_NODE;
  result = await new Promise((resolve, reject) => {
    const child = spawn(electron.default as unknown as string, [fileURLToPath(new URL("./fixtures/runIntegratedSelect.mjs", import.meta.url)), `http://127.0.0.1:${address.port}/tests/fixtures/integrated-select.html`], { cwd: webRoot, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    child.stdout.on("data", (value) => { stdout += value; }); child.stderr.on("data", (value) => { stderr += value; });
    child.on("error", reject);
    child.on("exit", () => { const line = stdout.split(/\r?\n/).find((line) => line.startsWith("INTEGRATED_SELECT_RESULT:")); line ? resolve(JSON.parse(line.slice("INTEGRATED_SELECT_RESULT:".length))) : reject(new Error(`${stdout}\n${stderr}`)); });
  });
  assert.equal(result.ok, true, JSON.stringify(result));
});
after(async () => server?.close());
for (const name of [
  "stable first visible frames and compact spacing in both motion modes",
  "native forms, labels and defaults",
  "empty choices, all-disabled choices and controlled ownership",
  "controlled selection and real HTMLSelectElement change event",
  "disabled options and optgroups",
  "arrows, Home, End, typeahead, Enter and Escape",
  "Tab, Shift Tab and outside dismissal",
  "dynamic options and disabled control",
  "uncontrolled selection, empty value and form reset",
  "joined geometry and single global pointer owner",
  "SDK parity, long scrolling list, upward placement and unmount",
  "modal parent preservation, stacking, Escape and native Tab order",
  "sibling dropdown transfer outside and inside modal",
  "reopening and final cleanup"
]) it(`mounted integrated select: ${name}`, () => assert.ok(result.cases.includes(name)));

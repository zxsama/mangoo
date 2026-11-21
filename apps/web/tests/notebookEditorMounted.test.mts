import assert from "node:assert/strict";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { it } from "node:test";
import { createServer } from "./viteTestServer.mts";

it("edits Markdown, collects attachments and runs an opt-in isolated HTML notebook", async () => {
  const webRoot = fileURLToPath(new URL("../", import.meta.url));
  const cacheDir = fileURLToPath(new URL("../../../.tmp/notebook-editor-vite", import.meta.url));
  const server = await createServer({ root: webRoot, cacheDir, logLevel: "error", server: { host: "127.0.0.1", port: 0 } });
  try {
    await server.listen();
    const address = server.httpServer!.address();
    assert.ok(address && typeof address === "object");
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    const electron = await import("electron");
    const { stdout } = await promisify(execFile)(electron.default as unknown as string, [
      fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)),
      `http://127.0.0.1:${address.port}/tests/fixtures/notebook-editor.html`
    ], { cwd: webRoot, windowsHide: true, env, timeout: 60_000 });
    const result = JSON.parse(stdout.slice(stdout.lastIndexOf("OVERLAY_LIFECYCLE_RESULT:") + "OVERLAY_LIFECYCLE_RESULT:".length));
    assert.equal(result.ok, true, result.error);
  } finally { await server.close(); }
});

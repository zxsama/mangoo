import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { it } from "node:test";
import { createServer } from "./viteTestServer.mts";

it("directory preferences use actual App menus, ordered pages and Library fences", async () => {
  const webRoot = fileURLToPath(new URL("../", import.meta.url));
  const server = await createServer({ root: webRoot, logLevel: "error", server: { host: "127.0.0.1", port: 0 } });
  try {
    await server.listen();
    const address = server.httpServer!.address();
    assert.ok(address && typeof address === "object");
    const electron = await import("electron");
    const env = { ...process.env }; delete env.ELECTRON_RUN_AS_NODE;
    const result = await new Promise<{ ok: boolean; error?: string; scenarios: string[] }>((resolve, reject) => {
      const child = spawn(electron.default as unknown as string, [
        fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)),
        `http://127.0.0.1:${address.port}/tests/fixtures/directory-preferences.html`
      ], { cwd: webRoot, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
      let stdout = "", stderr = "";
      child.stdout.on("data", (data) => stdout += data);
      child.stderr.on("data", (data) => stderr += data);
      child.on("error", reject);
      child.on("exit", () => {
        const marker = "OVERLAY_LIFECYCLE_RESULT:", index = stdout.lastIndexOf(marker);
        if (index < 0) reject(new Error(`${stdout}\n${stderr}`));
        else resolve(JSON.parse(stdout.slice(index + marker.length).trim()));
      });
    });
    assert.equal(result.ok, true, result.error);
    assert.deepEqual(result.scenarios, ["paged-order-stale-load-selection", "cursor-recovery-recursion-reload-retry", "root-read-and-library-fences", "actual-root-menu-offline-before-scan", "actual-sort-menu-pending-error-retry-reset", "actual-ordinary-folder-menu"]);
  } finally { await server.close(); }
});

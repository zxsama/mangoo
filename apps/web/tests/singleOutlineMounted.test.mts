import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { it } from "node:test";
import { createServer } from "./viteTestServer.mts";

it("mounted controls share one boundary through pointer, keyboard and accessibility states", async () => {
  const server = await createServer({ root: fileURLToPath(new URL("../", import.meta.url)), logLevel: "error", server: { host: "127.0.0.1", port: 0 } });
  try {
    await server.listen();
    const address = server.httpServer?.address();
    assert.ok(address && typeof address === "object");
    const electron = (await import("electron")).default as unknown as string;
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    const profileRoot = fileURLToPath(new URL("../../../.tmp/single-outline/", import.meta.url));
    mkdirSync(profileRoot, { recursive: true });
    const profile = mkdtempSync(`${profileRoot}/electron-profile-`);
    const result = await new Promise<{ code: number | null; output: string }>((resolve, reject) => {
      const child = spawn(electron, [fileURLToPath(new URL("./fixtures/runSingleOutline.mjs", import.meta.url)), `http://127.0.0.1:${address.port}/tests/fixtures/single-outline.html`, `--user-data-dir=${profile}`], { env, stdio: ["ignore", "pipe", "pipe"] });
      let output = "";
      const timeout = setTimeout(() => { child.kill(); reject(new Error(`single outline timed out: ${output}`)); }, 60000);
      child.stdout.on("data", (chunk) => { output += String(chunk); });
      child.stderr.on("data", (chunk) => { output += String(chunk); });
      child.on("error", reject);
      child.on("exit", (code) => { clearTimeout(timeout); resolve({ code, output }); });
    });
    assert.equal(result.code, 0, result.output);
    assert.match(result.output, /SINGLE_OUTLINE_OK/);
  } finally { await server.close(); }
});

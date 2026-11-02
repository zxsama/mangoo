import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { it } from "node:test";
import { createServer } from "./viteTestServer.mts";

it("mounted managed Library form preserves creation and lifecycle contracts", async () => {
  const server = await createServer({ root: fileURLToPath(new URL("../", import.meta.url)), logLevel: "error", server: { host: "127.0.0.1", port: 0 } });
  try {
    await server.listen();
    const address = server.httpServer?.address();
    assert.ok(address && typeof address === "object");
    const electron = (await import("electron")).default as unknown as string;
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    const output = await new Promise<string>((resolve, reject) => {
      const child = spawn(electron, [fileURLToPath(new URL("./fixtures/runManagedLibraryWizard.mjs", import.meta.url)), `http://127.0.0.1:${address.port}/tests/fixtures/managed-library-wizard.html`], { env, stdio: ["ignore", "pipe", "pipe"] });
      let stdout = "";
      let stderr = "";
      const timeout = setTimeout(() => { child.kill(); reject(new Error(`wizard timed out: ${stderr}`)); }, 60000);
      child.stdout.on("data", (chunk) => { stdout += String(chunk); });
      child.stderr.on("data", (chunk) => { stderr += String(chunk); });
      child.on("error", reject);
      child.on("exit", () => { clearTimeout(timeout); resolve(stdout || stderr); });
    });
    const line = output.split("\n").find((value) => value.startsWith("WIZARD_RESULT:"));
    assert.ok(line, output);
    const result = JSON.parse(line.slice("WIZARD_RESULT:".length));
    assert.equal(result.ok, true, result.error);
  } finally { await server.close(); }
});

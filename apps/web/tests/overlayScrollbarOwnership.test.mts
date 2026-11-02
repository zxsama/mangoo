import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { after, before, it } from 'node:test';
import { createServer, type ViteDevServer } from './viteTestServer.mts';
interface ScrollbarResult { ok: boolean; error?: string; }
const webRoot = fileURLToPath(new URL("../", import.meta.url));
const electronRunner = fileURLToPath(
  new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)
);
const resultPrefix = "OVERLAY_LIFECYCLE_RESULT:";

let server: ViteDevServer;
let result: ScrollbarResult;

async function runFixture(url: string): Promise<ScrollbarResult> {
  const electronModule = await import("electron");
  const electronPath = electronModule.default as unknown as string;
  const environment = { ...process.env };
  delete environment.ELECTRON_RUN_AS_NODE;

  return await new Promise<ScrollbarResult>((resolve, reject) => {
    const child = spawn(electronPath, [electronRunner, url], {
      cwd: webRoot,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"]
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill();
      reject(new Error(`overlay lifecycle Electron runner timed out\n${stderr}`));
    }, 45_000);

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on("exit", (code) => {
      clearTimeout(timeout);
      const markerIndex = stdout.lastIndexOf(resultPrefix);
      if (markerIndex < 0) {
        reject(new Error(
          `overlay lifecycle Electron runner exited ${code} without a result\n${stdout}\n${stderr}`
        ));
        return;
      }
      const payload = stdout.slice(markerIndex + resultPrefix.length).trim();
      resolve(JSON.parse(payload) as ScrollbarResult);
    });
  });
}

before(async () => {
  server = await createServer({
    root: webRoot,
    logLevel: "error",
    server: {
      host: "127.0.0.1",
      port: 0,
      strictPort: false
    }
  });
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address === "object");
  result = await runFixture(
    `http://127.0.0.1:${address.port}/tests/fixtures/overlayScrollbarOwnership.html`
  );
  assert.equal(result.ok, true, result.error);
});

after(async () => {
  await server?.close();
});

it('keeps popup-owned body scrollbars inside the overlay family through dragging and disposal', () => {
  assert.equal(result.ok, true, result.error);
});

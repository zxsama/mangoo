import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, existsSync, mkdtempSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { it } from "node:test";
import { createServer } from "./viteTestServer.mts";

it("native video hover plays, scrubs, resumes and releases across four layouts", async () => {
  const webRoot = fileURLToPath(new URL("../", import.meta.url));
  const mediaDir = fileURLToPath(new URL("../../../.tmp/all-file-library-validation/media/", import.meta.url));
  mkdirSync(mediaDir, { recursive: true });
  const profile = mkdtempSync(`${mediaDir}/hover-electron-profile-`);
  const clip = `${mediaDir}/00-hover-video.mp4`;
  if (!existsSync(clip)) {
    const generated = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100", "-t", "12", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart", "-y", clip]);
    assert.equal(generated.status, 0, generated.stderr?.toString() ?? generated.error?.message);
  }
  const server = await createServer({ root: webRoot, logLevel: "error", server: { host: "127.0.0.1", port: 0 },
    plugins: [{ name: "native-video-fixture", configureServer(vite) {
      vite.middlewares.use((request, _response, next) => {
        if (request.url?.startsWith("/api/playback/")) request.url = `/@fs/${clip.replaceAll("\\", "/")}`;
        next();
      });
    } }] });
  await server.listen();
  try {
    const address = server.httpServer!.address();
    assert.ok(address && typeof address === "object");
    const electron = await import("electron");
    const env = { ...process.env }; delete env.ELECTRON_RUN_AS_NODE;
    const fixtureUrl = `http://127.0.0.1:${address.port}/tests/fixtures/video-hover.html?clip=${encodeURIComponent(`/@fs/${clip.replaceAll("\\", "/")}`)}`;
    const result = await new Promise<{ ok: boolean; error?: string; modes: string[]; requests: number; revoked: number }>((resolve, reject) => {
      const child = spawn(electron.default as unknown as string, [fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)), fixtureUrl, `--user-data-dir=${profile}`], { cwd: webRoot, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
      let stdout = "", stderr = "";
      child.stdout.on("data", (data) => stdout += data); child.stderr.on("data", (data) => stderr += data);
      child.on("error", reject);
      child.on("exit", () => {
        const marker = "OVERLAY_LIFECYCLE_RESULT:", index = stdout.lastIndexOf(marker);
        if (index < 0) reject(new Error(`${stdout}\n${stderr}`));
        else resolve(JSON.parse(stdout.slice(index + marker.length).trim()));
      });
    });
    assert.equal(result.ok, true, result.error);
    assert.deepEqual(result.modes, ["adaptive", "waterfall", "grid", "list"]);
    assert.ok(result.requests >= 8);
    assert.ok(result.revoked >= 8);
  } finally { await server.close(); }
});

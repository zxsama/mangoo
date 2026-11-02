import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { mkdir, open, readFile, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { createServer } from "vite";
import { createAndActivateManagedLibrary, addExistingMediaRoot, managedLibraryHostEnv, resolveCoreBinary } from "../dev/managed-library-harness.mjs";

const root = path.resolve(fileURLToPath(new URL("../../", import.meta.url)));
const runDir = path.join(root, ".tmp", "video-hover-integration", `run-${Date.now()}`);
const mediaDir = path.join(runDir, "media");
await mkdir(mediaDir, { recursive: true });
const short = path.join(mediaDir, "short.mp4");
const generated = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100", "-t", "12", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart", "-y", short]);
assert.equal(generated.status, 0, generated.stderr?.toString() ?? generated.error?.message);

// Insert a sparse ISO-BMFF free box before mdat and adjust stco offsets. This
// stays a real decodable clip whose media payload is over 512 MiB from moov.
const bytes = await readFile(short);
const padding = 512 * 1024 * 1024;
let insertion = 0;
for (let offset = 0; offset < bytes.length;) {
  const size = bytes.readUInt32BE(offset);
  assert.ok(size >= 8);
  if (bytes.toString("ascii", offset + 4, offset + 8) === "mdat") { insertion = offset; break; }
  offset += size;
}
assert.ok(insertion > 0);
function patchChunkOffsets(start, end) {
  for (let offset = start; offset + 8 <= end;) {
    const size = bytes.readUInt32BE(offset), type = bytes.toString("ascii", offset + 4, offset + 8);
    assert.ok(size >= 8 && offset + size <= end);
    if (["moov", "trak", "mdia", "minf", "stbl"].includes(type)) patchChunkOffsets(offset + 8, offset + size);
    if (type === "stco") {
      const count = bytes.readUInt32BE(offset + 12);
      for (let index = 0; index < count; index++) {
        const position = offset + 16 + index * 4, value = bytes.readUInt32BE(position);
        if (value >= insertion) bytes.writeUInt32BE(value + padding, position);
      }
    }
    offset += size;
  }
}
patchChunkOffsets(0, bytes.length);
const large = path.join(mediaDir, "large.mp4");
const file = await open(large, "w");
try {
  await file.truncate(bytes.length + padding);
  await file.write(bytes.subarray(0, insertion), 0, insertion, 0);
  const box = Buffer.alloc(8); box.writeUInt32BE(padding); box.write("free", 4, "ascii");
  await file.write(box, 0, 8, insertion);
  await file.write(bytes.subarray(insertion), 0, bytes.length - insertion, insertion + padding);
} finally { await file.close(); }

const token = randomUUID();
let core, server, memorySampler;
let stdout = "", stderr = "";
let transferredBytes = 0;
let playbackPhase = "positive-playback";
const playbackReads = [];
const ranges = [];
const statuses = [];
const streamUrls = new Set();
const apiPaths = [];
const coreMemory = { baselineWorkingSetBytes: 0, peakWorkingSetBytes: 0, peakSampledWorkingSetBytes: 0, samples: 0 };
let closedBodies = 0, finishedBodies = 0;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
try {
  const probe = createHttpServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  const baseUrl = `http://127.0.0.1:${port}/api`;
  core = spawn(resolveCoreBinary({ root }), [], { cwd: root, windowsHide: true,
    env: { ...process.env, ...managedLibraryHostEnv(runDir), MANGOO_SESSION_TOKEN: token, MANGOO_CORE_ADDR: `127.0.0.1:${port}`, MANGOO_BASIC_AUTH: "", MANGOO_PARENT_PID: String(process.pid), MANGOO_SERVE_WEB: "0" }, stdio: ["ignore", "pipe", "pipe"] });
  core.stdout.on("data", (data) => stdout += data); core.stderr.on("data", (data) => stderr += data);
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await fetch(`${baseUrl}/health`, { headers: { "x-mangoo-session": token } }).then((r) => r.ok).catch(() => false)) break;
    if (core.exitCode !== null) throw new Error(`Core exited ${core.exitCode}: ${stderr}`);
    await delay(100);
  }
  const library = await createAndActivateManagedLibrary({ baseUrl, token, name: "Hover fixture", databasePath: path.join(runDir, "Library", "Database"), contentRoots: [{ path: path.join(runDir, "Library", "Files"), createIfMissing: true }] });
  await addExistingMediaRoot({ baseUrl, token, contextRevision: library.contextRevision, mediaRoot: mediaDir });
  let media = [];
  for (let attempt = 0; attempt < 100; attempt++) {
    const result = await fetch(`${baseUrl}/media`, { headers: { "x-mangoo-session": token } }).then((r) => r.json());
    media = result.items ?? [];
    if (media.length >= 2) break;
    await delay(100);
  }
  assert.equal(media.length, 2);
  if (process.platform === "win32") {
    memorySampler = spawn("powershell.exe", ["-NoProfile", "-Command", `$taskProcess = Get-Process -Id ${core.pid}; while (-not $taskProcess.HasExited) { $taskProcess.Refresh(); Write-Output ($taskProcess.WorkingSet64.ToString() + ',' + $taskProcess.PeakWorkingSet64.ToString()); Start-Sleep -Milliseconds 250 }`], { windowsHide: true, stdio: ["ignore", "pipe", "ignore"] });
    let pending = "";
    memorySampler.stdout.on("data", (data) => {
      pending += data;
      const lines = pending.split(/\r?\n/); pending = lines.pop();
      for (const line of lines) {
        const [workingSet, peak] = line.split(",").map(Number);
        if (!workingSet || !peak) continue;
        if (!coreMemory.samples) coreMemory.baselineWorkingSetBytes = workingSet;
        coreMemory.samples++;
        coreMemory.peakWorkingSetBytes = Math.max(coreMemory.peakWorkingSetBytes, peak);
        coreMemory.peakSampledWorkingSetBytes = Math.max(coreMemory.peakSampledWorkingSetBytes, workingSet);
      }
    });
  }
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const webPort = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  server = await createServer({ root: path.join(root, "apps", "web"), logLevel: "error", server: { host: "127.0.0.1", port: webPort, strictPort: true },
    plugins: [{ name: "core-playback-integration", configureServer(vite) {
      vite.middlewares.use((request, response, next) => {
        if (request.url === "/hover-fixture-data") {
          response.setHeader("content-type", "application/json");
          response.end(JSON.stringify({ media, token })); return;
        }
        if (request.url === "/hover-fixture-switch" && request.method === "POST") {
          playbackPhase = "library-switch";
          void createAndActivateManagedLibrary({ baseUrl, token, name: "Hover switch fixture", databasePath: path.join(runDir, "SecondLibrary", "Database"), contentRoots: [{ path: path.join(runDir, "SecondLibrary", "Files"), createIfMissing: true }] })
            .then(() => { response.statusCode = 204; response.end(); })
            .catch((error) => { response.statusCode = 500; response.end(String(error)); });
          return;
        }
        if (request.url === "/hover-fixture-remove-short" && request.method === "POST") {
          playbackPhase = "missing-source";
          void unlink(short).then(() => { response.statusCode = 204; response.end(); })
            .catch((error) => { response.statusCode = 500; response.end(String(error)); });
          return;
        }
        if (request.url === "/hover-fixture-corrupt-short" && request.method === "POST") {
          playbackPhase = "corrupt-source";
          void writeFile(short, Buffer.alloc(128)).then(() => { response.statusCode = 204; response.end(); })
            .catch((error) => { response.statusCode = 500; response.end(String(error)); });
          return;
        }
        if (!request.url?.startsWith("/api/")) { next(); return; }
        apiPaths.push(request.url.replace(/\/playback\/[^/?]+/, "/playback/<grant>"));
        const playback = request.url.startsWith("/api/playback/");
        if (playback) { ranges.push(request.headers.range ?? "none"); streamUrls.add(request.url); }
        const read = playback ? { phase: playbackPhase, status: null, errorBody: "" } : null;
        if (read) playbackReads.push(read);
        const upstream = httpRequest(`http://127.0.0.1:${port}${request.url}`, { method: request.method, headers: request.headers }, (source) => {
          response.writeHead(source.statusCode, source.headers);
          if (playback) {
            statuses.push(source.statusCode);
            read.status = source.statusCode;
            if (source.statusCode !== 200 && source.statusCode !== 206) {
              source.on("data", (chunk) => { read.errorBody = (read.errorBody + chunk.toString("utf8")).slice(0, 4096); });
            }
            source.on("data", (data) => transferredBytes += data.length);
            source.on("end", () => finishedBodies++);
            source.on("close", () => closedBodies++);
          }
          source.pipe(response);
          response.on("close", () => source.destroy());
        });
        upstream.on("error", () => response.destroy());
        response.on("close", () => upstream.destroy());
        request.pipe(upstream);
      });
    } }] });
  await server.listen();
  const address = server.httpServer.address();
  const electron = await import("electron");
  const env = { ...process.env, MANGOO_VIDEO_HOVER_EVIDENCE: runDir }; delete env.ELECTRON_RUN_AS_NODE;
  const result = await new Promise((resolve, reject) => {
    const child = spawn(electron.default, [path.join(root, "apps/web/tests/fixtures/runVideoHoverCore.mjs"), `http://127.0.0.1:${address.port}/tests/fixtures/video-hover-core.html`], { cwd: root, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    let output = "", errors = "";
    child.stdout.on("data", (data) => output += data); child.stderr.on("data", (data) => errors += data);
    child.on("error", reject); child.on("exit", (code) => {
      const marker = "VIDEO_HOVER_RESULT:", index = output.lastIndexOf(marker);
      if (index < 0) reject(new Error(`Electron exited ${code}: ${output}${errors}`)); else resolve(JSON.parse(output.slice(index + marker.length)));
    });
  });
  await delay(200);
  const afterSwitchStatuses = [];
  for (const url of streamUrls) afterSwitchStatuses.push(await fetch(`http://127.0.0.1:${port}${url}`, { method: "HEAD" }).then((response) => response.status));
  const report = { ...result, sourceBytes: bytes.length + padding, transferredBytes, ranges, statuses, playbackReads, closedBodies, finishedBodies, coreMemory, afterSwitchStatuses,
    fullPreviewRequests: apiPaths.filter((url) => /\/media\/\d+\/preview/.test(url)).length };
  await writeFile(path.join(runDir, "result.json"), JSON.stringify(report, null, 2));
  assert.equal(result.ok, true, result.error);
  assert.deepEqual(result.modes.map((entry) => entry.mode), ["adaptive", "waterfall", "grid", "list"]);
  assert.ok(ranges.some((range) => Number(/^bytes=(\d+)/.exec(range)?.[1]) >= padding), "browser requested distant payload range");
  assert.ok(transferredBytes < 32 * 1024 * 1024, `bounded transferred bytes across repeated activations: ${transferredBytes}`);
  for (const read of playbackReads) {
    if (read.status === 200 || read.status === 206) continue;
    // The deliberate rewrite can invalidate the source between grant creation
    // and native reading. Only this negative scenario may reject that identity.
    assert.equal(read.phase, "corrupt-source", `unexpected playback status ${read.status}`);
    assert.equal(read.status, 409);
    assert.equal(JSON.parse(read.errorBody).code, "playback_source_changed");
  }
  assert.equal(report.fullPreviewRequests, 0);
  assert.equal(closedBodies, statuses.length, "all browser stream bodies closed");
  assert.ok(finishedBodies < closedBodies, "at least one browser response cancelled before EOF");
  assert.ok(afterSwitchStatuses.every((status) => status === 404), "old capabilities unavailable after actual activation");
  if (process.platform === "win32") assert.ok(coreMemory.samples > 0 && coreMemory.peakWorkingSetBytes < padding, "Core peak working set below full source size");
  assert.ok(result.rendererMemory.samples > 0 && result.rendererMemory.peakWorkingSetKiB * 1024 < padding, "renderer peak working set below full source size");
  console.log(JSON.stringify({ ...report, evidence: path.relative(root, path.join(runDir, "result.json")) }, null, 2));
} finally {
  await server?.close();
  if (core && core.exitCode === null) core.kill();
  memorySampler?.kill();
  await writeFile(path.join(runDir, "core.log"), stdout + stderr);
}

import net from "node:net";
import { randomInt } from "node:crypto";

export const DEFAULT_WEB_URL = "http://127.0.0.1:5173";

export async function isMangooWebServerReady(url) {
  try {
    const response = await fetch(url, { cache: "no-store", signal: AbortSignal.timeout(1_000) });
    if (!response.ok || !response.headers.get("content-type")?.includes("text/html")) return false;
    const html = await response.text();
    return html.includes("<title>Mangoo</title>") && html.includes("/src/main.tsx");
  } catch {
    return false;
  }
}

function probeLoopbackPort(port) {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen({ host: "127.0.0.1", port, exclusive: true }, () => {
      const selected = server.address().port;
      server.close((error) => error ? reject(error) : resolve(selected));
    });
  });
}

export async function availableLoopbackPort(port, probePort = probeLoopbackPort) {
  if (port !== 0) return probePort(port);
  // Windows can allocate a low ephemeral port (for example 2049) that Chromium
  // rejects. Probe the dynamic/private range rather than trusting listen(0).
  let lastError;
  for (let attempt = 0; attempt < 128; attempt += 1) {
    try {
      return await probePort(randomInt(49152, 65536));
    } catch (error) {
      if (error.code !== "EACCES" && error.code !== "EADDRINUSE") throw error;
      lastError = error;
    }
  }
  throw lastError;
}

export async function waitForMangooWebServer(url, child, {
  timeoutMs = 20_000,
  isReady = isMangooWebServerReady
} = {}) {
  let spawnError;
  const onError = (error) => { spawnError = error; };
  child.on("error", onError);
  try {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null || child.signalCode !== null) {
        throw new Error(`Web dev server exited before becoming ready at ${url} (exit ${child.exitCode ?? child.signalCode})`);
      }
      if (await isReady(url)) return;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error(`Timed out waiting for Mangoo web dev server at ${url}`);
  } finally {
    child.off("error", onError);
  }
}

export async function resolveWebDevUrl({
  configuredUrl = process.env.MANGOO_WEB_URL,
  defaultUrl = DEFAULT_WEB_URL,
  isReady = isMangooWebServerReady,
  probePort = availableLoopbackPort,
  log = console.log
} = {}) {
  // A configured address is an explicit contract: never silently change it.
  if (configuredUrl) return configuredUrl;
  if (await isReady(defaultUrl)) return defaultUrl;
  try {
    await probePort(Number(new URL(defaultUrl).port));
    return defaultUrl;
  } catch (error) {
    if (error.code !== "EACCES" && error.code !== "EADDRINUSE") throw error;
    const port = await probePort(0);
    const url = `http://127.0.0.1:${port}`;
    log(`[mangoo] Default web address ${defaultUrl} unavailable (${error.code}); using ${url}`);
    return url;
  }
}

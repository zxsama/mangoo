import { spawn } from "node:child_process";
import path from "node:path";
import { isMangooWebServerReady, resolveWebDevUrl } from "./web-dev-url.mjs";

const webUrl = await resolveWebDevUrl();
console.log(`[mangoo] Starting Web at ${webUrl}`);
if (await isMangooWebServerReady(webUrl)) {
  console.log(`[mangoo] Reusing existing Mangoo web dev server at ${webUrl}`);
} else {
  const url = new URL(webUrl);
  const child = spawn(process.execPath, [
    path.resolve(import.meta.dirname, "../../node_modules/vite/bin/vite.js"),
    "--host", url.hostname,
    "--port", url.port || (url.protocol === "https:" ? "443" : "80"),
    "--strictPort"
  ], {
    cwd: path.resolve(import.meta.dirname, "../../apps/web"),
    env: { ...process.env, MANGOO_WEB_URL: webUrl },
    stdio: "inherit",
    windowsHide: true
  });
  child.once("error", (error) => { console.error(error); process.exitCode = 1; });
  child.once("exit", (code) => { process.exitCode = code ?? 1; });
  process.on("exit", () => { if (!child.killed) child.kill(); });
}

import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const root = process.cwd();
const resultDir = path.resolve(root, "results");
const port = Number(process.env.PORT ?? "5177");
const count = Number(process.env.ITEM_COUNT ?? "1000000");
const url = `http://127.0.0.1:${port}/?count=${count}`;

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForServer(proc) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      throw new Error(`vite exited with code ${proc.exitCode}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      await wait(250);
    }
  }
  throw new Error("vite server did not become ready");
}

const vite = spawn(
  process.platform === "win32" ? "npm.cmd" : "npm",
  ["exec", "vite", "--", "--host", "127.0.0.1", "--port", String(port)],
  { cwd: root, stdio: "inherit", shell: process.platform === "win32" }
);

try {
  await waitForServer(vite);
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.goto(url);
  await page.waitForFunction(() => window.__mangooReady === true);
  const result = await page.evaluate(() => window.runVirtualGridBenchmark());
  await browser.close();

  await mkdir(resultDir, { recursive: true });
  const report = {
    url,
    viewport: { width: 1440, height: 900 },
    result
  };
  const reportPath = path.join(resultDir, `virtual_grid_${count}.json`);
  await writeFile(reportPath, JSON.stringify(report, null, 2), "utf8");
  console.log(JSON.stringify(report, null, 2));
} finally {
  vite.kill();
}

import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { copyFile, mkdir, readdir, writeFile } from "node:fs/promises";
import path from "node:path";

const root = process.cwd();
const repoRoot = path.resolve(root, "../../..");
const sourceThumbDir = path.join(repoRoot, "tools", "bench", "thumbnail", "results", "thumbnail-output", "images");
const publicThumbDir = path.join(root, "public", "thumbs");
const resultDir = path.join(root, "results");
const port = Number(process.env.PORT ?? "5187");
const count = Number(process.env.ITEM_COUNT ?? "1000000");
const iterations = Number(process.env.ITERATIONS ?? "1000");
const url = `http://127.0.0.1:${port}/?count=${count}`;

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function prepareThumbs() {
  await mkdir(publicThumbDir, { recursive: true });
  const files = (await readdir(sourceThumbDir))
    .filter((name) => name.endsWith(".webp"))
    .sort();
  if (!files.length) {
    throw new Error(`no webp thumbnails found in ${sourceThumbDir}`);
  }
  for (const file of files) {
    await copyFile(path.join(sourceThumbDir, file), path.join(publicThumbDir, file));
  }
  await writeFile(
    path.join(publicThumbDir, "manifest.json"),
    JSON.stringify({ files }, null, 2),
    "utf8"
  );
  return files.length;
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

await prepareThumbs();

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
  await page.waitForFunction(() => window.__mangooPreviewReady === true);

  const cached = await page.evaluate(
    (runs) => window.runPreviewSwitchBenchmark?.({ mode: "cached", iterations: runs, step: 17, preloadRadius: 12 }),
    iterations
  );
  const prefetch = await page.evaluate(
    (runs) => window.runPreviewSwitchBenchmark?.({ mode: "prefetch", iterations: runs, step: 1, preloadRadius: 12 }),
    iterations
  );
  const burst = await page.evaluate(() =>
    window.runPreviewBurstBenchmark?.({ bursts: 200, changesPerBurst: 20, step: 19 })
  );

  await browser.close();
  await mkdir(resultDir, { recursive: true });
  const report = {
    url,
    viewport: { width: 1440, height: 900 },
    cached,
    prefetch,
    burst
  };
  const reportPath = path.join(resultDir, `preview_switch_${count}_${iterations}.json`);
  await writeFile(reportPath, JSON.stringify(report, null, 2), "utf8");
  console.log(JSON.stringify(report, null, 2));
} finally {
  vite.kill();
}

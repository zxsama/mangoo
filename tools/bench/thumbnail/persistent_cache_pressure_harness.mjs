import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const root = path.resolve(import.meta.dirname, "..", "..", "..");
const sweepScript = path.join(root, "tools", "dev", "artists-million-desktop-sweep.mjs");
const sweepSummaryPath = path.join(
  root,
  ".tmp",
  "visual-check",
  "logs",
  "artists-million-desktop-sweep-summary.json"
);
const resultsDir = path.join(root, "tools", "bench", "thumbnail", "results");
const defaultArtistsRoot = process.env.MANGOO_ARTISTS_SWEEP_ROOT ?? "Y:\\Repository\\Billfish\\Artists";
const defaultOperationCount = process.env.MANGOO_ARTISTS_SWEEP_OPERATION_COUNT ?? "500";
const defaultWebUrl = process.env.MANGOO_ARTISTS_SWEEP_WEB_URL ?? "http://127.0.0.1:5181";
const defaultDebugPort = Number(process.env.MANGOO_ARTISTS_SWEEP_DEBUG_PORT ?? "9251");
const scenarioDebugPortBase = defaultDebugPort + 10;
const runId = new Date().toISOString().replace(/[:.]/g, "-");
const sharedHotCacheDataDir = path.join(
  root,
  ".tmp",
  "visual-check",
  `data-artists-million-cold-hot-${runId}`
);
const warmSeenFileIdsPath = path.join(
  root,
  ".tmp",
  "visual-check",
  `artists-million-seen-file-ids-${runId}.json`
);

const scenarios = [
  {
    name: "cold-cache",
    env: {
      MANGOO_ARTISTS_SWEEP_DATA_DIR: sharedHotCacheDataDir,
      MANGOO_ARTISTS_SWEEP_RESET_DB: "1",
      MANGOO_ARTISTS_SWEEP_THUMBNAIL_CACHE_MODE: "clear",
      MANGOO_ARTISTS_SWEEP_OPERATION_COUNT: defaultOperationCount
    }
  },
  {
    name: "warm-seen-thumbnails",
    env: {
      MANGOO_ARTISTS_SWEEP_DATA_DIR: sharedHotCacheDataDir,
      MANGOO_ARTISTS_SWEEP_RESET_DB: "0",
      MANGOO_ARTISTS_SWEEP_THUMBNAIL_CACHE_MODE: "warmSeen",
      MANGOO_ARTISTS_SWEEP_WARM_FILE_IDS_PATH: warmSeenFileIdsPath,
      MANGOO_ARTISTS_SWEEP_OPERATION_COUNT: "0"
    }
  },
  {
    name: "hot-cache",
    env: {
      MANGOO_ARTISTS_SWEEP_DATA_DIR: sharedHotCacheDataDir,
      MANGOO_ARTISTS_SWEEP_RESET_DB: "0",
      MANGOO_ARTISTS_SWEEP_THUMBNAIL_CACHE_MODE: "none",
      MANGOO_ARTISTS_SWEEP_OPERATION_COUNT: defaultOperationCount
    }
  }
];

async function runScenario(scenario, index) {
  const env = {
    ...process.env,
    MANGOO_ARTISTS_SWEEP_ROOT: defaultArtistsRoot,
    MANGOO_ARTISTS_SWEEP_WEB_URL: defaultWebUrl,
    MANGOO_ARTISTS_SWEEP_DEBUG_PORT: String(scenarioDebugPortBase + index),
    ...scenario.env
  };
  await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [sweepScript], {
      cwd: root,
      env,
      stdio: "inherit",
      windowsHide: true
    });
    child.on("exit", (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${scenario.name} exited with code ${code ?? "null"}`));
      }
    });
    child.on("error", reject);
  });

  const summary = JSON.parse(await readFile(sweepSummaryPath, "utf8"));
  const scenarioSummaryPath = path.join(
    resultsDir,
    `persistent-cache-pressure-${scenario.name}.json`
  );
  await writeFile(scenarioSummaryPath, JSON.stringify(summary, null, 2));
  return {
    name: scenario.name,
    summaryPath: scenarioSummaryPath,
    summary
  };
}

await mkdir(resultsDir, { recursive: true });

const scenarioResults = [];
for (const [index, scenario] of scenarios.entries()) {
  console.log(`Running persistent cache pressure scenario: ${scenario.name}`);
  const result = await runScenario(scenario, index);
  scenarioResults.push(result);
  if (scenario.name === "cold-cache") {
    await writeFile(
      warmSeenFileIdsPath,
      JSON.stringify(result.summary.seenMediaIds ?? [], null, 2)
    );
  }
}

const report = {
  generatedAtUtc: new Date().toISOString(),
  root: defaultArtistsRoot,
  scenarios: scenarioResults.map((result) => ({
    name: result.name,
    summaryPath: result.summaryPath,
    run: result.summary.run,
    thumbnailCache: result.summary.thumbnailCache,
    databaseFiles: result.summary.databaseFiles,
    stats: result.summary.stats
  }))
};

const reportPath = path.join(resultsDir, "persistent-cache-pressure-summary.json");
await writeFile(reportPath, JSON.stringify(report, null, 2));
console.log(`Persistent cache pressure harness complete. Summary: ${reportPath}`);

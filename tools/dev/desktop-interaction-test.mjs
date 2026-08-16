// Real desktop interaction test. Launches the actual Electron app against a real
// library and drives the interactions users reported problems with — folder
// switching, opening files, scrolling, deep-scroll, all while the background
// scan + thumbnail generation runs — then records per-operation latency plus a
// hang/failure/flicker log and asserts against budgets. This is the harness for
// catching interaction bugs the API-only hot-loop test cannot see.
//
// Run from repo root:
//   node tools/dev/desktop-interaction-test.mjs
// Env:
//   MEGLE_INTERACTION_ROOT     library to add (default Y:\Repository\Billfish\Artists)
//   MEGLE_INTERACTION_OPS      number of operations (default 60)
//   MEGLE_INTERACTION_PORT     CDP debug port (default 9240)
//   MEGLE_INTERACTION_WARMUP_MS   wait before driving (default 18000)
//   MEGLE_KEEP_DATA=1          keep the temp DB + screenshots
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { connectCdp, resolvePageWsUrl } from "./cdp-client.mjs";
import { managedLibraryHostEnv } from "./managed-library-harness.mjs";

const ROOT = (process.env.MEGLE_INTERACTION_ROOT ?? "Y:/Repository/Billfish/Artists").replace(/\\/g, "/");
const OPS = Number(process.env.MEGLE_INTERACTION_OPS ?? 60);
const PORT = Number(process.env.MEGLE_INTERACTION_PORT ?? 9240);
const WARMUP_MS = Number(process.env.MEGLE_INTERACTION_WARMUP_MS ?? 18000);
const KEEP = process.env.MEGLE_KEEP_DATA === "1";
const DATA_DIR = path.join(process.cwd(), ".tmp", "interaction-test");
const MANAGED_ROOT = path.join(DATA_DIR, "library");
const SHOT_DIR = path.join(DATA_DIR, "shots");

// Per-operation budgets (ms). A folder switch or preview open slower than this,
// or that never completes, is a failure.
const BUDGET = {
  folderSwitchMs: 3000,
  previewOpenMs: 3500,
  firstTilesMs: 3000
};

mkdirSync(SHOT_DIR, { recursive: true });

const dev = spawn("npm", ["run", "dev"], {
  cwd: process.cwd(),
  env: {
    ...process.env,
    MEGLE_REMOTE_DEBUG: "1",
    MEGLE_REMOTE_DEBUG_PORT: String(PORT),
    MEGLE_AUTO_ADD_ROOT: ROOT,
    MEGLE_AUTO_LIBRARY_ROOT: MANAGED_ROOT,
    ...managedLibraryHostEnv(DATA_DIR)
  },
  stdio: ["ignore", "pipe", "pipe"],
  shell: true,
  windowsHide: true
});
dev.stdout.on("data", (d) => process.stderr.write(`[dev] ${d}`));
dev.stderr.on("data", (d) => process.stderr.write(`[dev] ${d}`));

async function stop() {
  for (const args of [
    ["/PID", String(dev.pid), "/T", "/F"],
    ["/F", "/IM", "electron.exe", "/T"],
    ["/F", "/IM", "megle-core.exe", "/T"]
  ]) {
    await new Promise((res) => {
      const k = spawn("taskkill", args, { stdio: "ignore" });
      k.on("close", res);
      k.on("error", res);
    });
  }
}

const log = (...a) => console.log("[interaction]", ...a);

// A single DOM snapshot of everything the test cares about.
const SNAP = `(() => {
  const grid = document.querySelector(".virtual-grid");
  const spacer = grid?.querySelector(".virtual-grid-spacer");
  const tree = document.querySelector(".tree-list");
  return {
    treeFolders: document.querySelectorAll(".tree-item[data-folder-id]").length,
    treeExpanded: document.querySelectorAll('.tree-item[aria-expanded="true"]').length,
    treeMaxDepth: Math.max(0, ...[...document.querySelectorAll(".tree-item[data-depth]")].map((e) => Number(e.getAttribute("data-depth")) || 0)),
    treeScrollTop: tree ? Math.round(tree.scrollTop) : null,
    treeScrollHeight: tree ? Math.round(tree.scrollHeight) : null,
    treeClientHeight: tree ? Math.round(tree.clientHeight) : null,
    selectedFolderId: document.querySelector(".tree-item.selected")?.getAttribute("data-folder-id") || null,
    tiles: document.querySelectorAll(".media-tile").length,
    imgs: document.querySelectorAll(".tile-thumb-image").length,
    subfolderCards: document.querySelectorAll(".subfolder-card").length,
    subfolderCoverImgs: document.querySelectorAll(".subfolder-card-cover-image").length,
    skeletons: document.querySelectorAll(".media-tile.skeleton").length,
    ready: document.querySelectorAll(".tile-thumb-ready").length,
    loading: document.querySelectorAll(".tile-thumb-loading").length,
    failed: document.querySelectorAll(".tile-thumb-failed").length,
    scrollTop: grid ? Math.round(grid.scrollTop) : null,
    scrollHeight: grid ? Math.round(grid.scrollHeight) : null,
    clientHeight: grid ? Math.round(grid.clientHeight) : null,
    previewOpen: Boolean(document.querySelector(".central-preview-stage")),
    previewReady: Boolean(document.querySelector(".central-preview-stage img, .central-preview-stage video, .central-preview-stage .preview-placeholder.ready")),
    errorStrip: document.querySelector(".error-strip")?.textContent?.slice(0, 120) || null
  };
})()`;

const results = { ops: [], failures: [], samples: {} };
function record(kind, ms) {
  (results.samples[kind] ??= []).push(ms);
}
function fail(op, reason, detail) {
  results.failures.push({ op, reason, detail });
  log(`FAIL op${op}: ${reason} ${detail ?? ""}`);
}

// --- Sidebar tree operations ---

// Scroll the sidebar tree itself (as the user would to reach far-down folders).
async function scrollTree(cdp, snap, op) {
  const s0 = await snap();
  if (!s0.treeScrollHeight || s0.treeScrollHeight <= s0.treeClientHeight) return; // not scrollable
  const targetFrac = 0.3 + ((op * 29) % 60) / 100;
  await cdp.run(
    `(() => { const t = document.querySelector(".tree-list"); if (t) t.scrollTop = Math.round((t.scrollHeight - t.clientHeight) * ${targetFrac.toFixed(3)}); })()`
  );
  await delay(250);
  const s1 = await snap();
  if (s1.treeScrollTop === 0 && s0.treeScrollHeight - s0.treeClientHeight > 50) {
    fail(op, "tree scroll did not move", `scrollHeight=${s1.treeScrollHeight}`);
  }
}

// --- Folder-switch coverage suite ---------------------------------------------

// Discover a diverse catalog of folders across tree levels: expand collapsed
// nodes breadth-first (and "Load more children") until we have `want` distinct
// folders spanning multiple depths. Returns [{id, depth}].
async function discoverFolders(cdp, snap, want) {
  const seen = new Map();
  for (let round = 0; round < 40 && seen.size < want; round += 1) {
    const items = await cdp.evalJson(
      `[...document.querySelectorAll(".tree-item[data-folder-id]")].map((e) => ({ id: e.getAttribute("data-folder-id"), depth: Number(e.getAttribute("data-depth")) || 0, expanded: e.getAttribute("aria-expanded") }))`
    );
    for (const it of items) if (!seen.has(it.id)) seen.set(it.id, it);
    if (seen.size >= want) break;
    // Expand the next collapsed node (deepest-first widens level coverage), or
    // click a "Load more children" if present, to reveal more folders.
    const expanded = await cdp.evalJson(
      `(() => {
        const collapsed = [...document.querySelectorAll('.tree-item[data-folder-id][aria-expanded="false"]')];
        if (collapsed.length) { collapsed[0].querySelector(".tree-disclosure")?.click(); return "expand"; }
        const more = document.querySelector(".tree-load-more:not([disabled])");
        if (more) { more.click(); return "loadmore"; }
        return "none";
      })()`
    );
    if (expanded === "none") break;
    await delay(700);
  }
  return [...seen.values()];
}

// Switch to a folder by id; assert selection changes and (unless the folder is
// empty / entirely skipped_small) that real thumbnail <img> tiles render within
// a few seconds. Records timing under `label`.
async function switchToFolder(cdp, snap, op, folderId, label) {
  const t0 = Date.now();
  await cdp.run(
    `document.querySelector('.tree-item[data-folder-id="${folderId}"] .tree-label')?.click()`
  );
  let selected = false;
  for (let i = 0; i < 80; i++) {
    const s = await snap();
    if (s.selectedFolderId === String(folderId)) {
      selected = true;
      break;
    }
    await delay(60);
  }
  if (!selected) {
    fail(op, `${label}: folder never selected`, `folder ${folderId}, ${Date.now() - t0}ms`);
    return;
  }
  record(label, Date.now() - t0);
  // Wait for content to render: either media thumbnails (folders with direct
  // media) or subfolder cover images (subfolders-only folders like the root).
  let s = await snap();
  for (
    let i = 0;
    i < 70 &&
    ((s.tiles > 0 && s.imgs < Math.min(s.tiles, 6)) ||
      (s.tiles === 0 && s.subfolderCards > 0 && s.subfolderCoverImgs === 0));
    i += 1
  ) {
    await delay(150);
    s = await snap();
  }
  if (s.tiles > 0 && s.imgs === 0) {
    fail(op, `${label}: media thumbnails never rendered`, `folder ${folderId} tiles=${s.tiles} imgs=0`);
  } else if (s.tiles === 0 && s.subfolderCards > 0 && s.subfolderCoverImgs === 0) {
    fail(op, `${label}: subfolder covers never rendered`, `folder ${folderId} cards=${s.subfolderCards} coverImgs=0`);
  }
}

async function main() {
  log(`launching desktop against ${ROOT} (ops=${OPS}, warmup=${WARMUP_MS}ms)`);
  let ws = null;
  for (let i = 0; i < 180; i++) {
    try {
      ws = await resolvePageWsUrl(PORT);
      if (ws) break;
    } catch {}
    await delay(1000);
  }
  if (!ws) throw new Error("CDP page target never came up");

  const cdp = connectCdp(ws);
  await cdp.ready;
  await cdp.send("Runtime.enable");
  await cdp.send("Log.enable");
  const snap = () => cdp.evalJson(SNAP);

  await delay(WARMUP_MS);
  const first = await snap();
  log(`warmed up: treeFolders=${first.treeFolders} tiles=${first.tiles} scrollHeight=${first.scrollHeight}`);
  await cdp.screenshot(path.join(SHOT_DIR, "00-warm.png"));

  // Discover a diverse folder catalog across tree levels (expands the tree).
  const catalog = await discoverFolders(cdp, snap, 40);
  const depths = [...new Set(catalog.map((f) => f.depth))].sort((a, b) => a - b);
  log(`discovered ${catalog.length} folders across depths [${depths.join(", ")}]`);
  await cdp.screenshot(path.join(SHOT_DIR, "01-tree-expanded.png"));

  // Build a ~30-switch sequence covering the real navigation scenarios:
  //  - cold first-visits across different levels (top-level + deep),
  //  - hot re-visits of already-seen folders,
  //  - switch-out-and-back (A -> B -> A),
  //  - rapid consecutive switches,
  //  - alternating shallow/deep.
  const byDepthShallow = catalog.filter((f) => f.depth <= 1);
  const byDepthDeep = catalog.filter((f) => f.depth >= 2);
  const pool = catalog.length > 0 ? catalog : [];
  const seq = [];
  if (pool.length > 0) {
    const pick = (arr, i) => (arr.length ? arr[i % arr.length] : pool[i % pool.length]);
    // 1) cold first visits, alternating shallow and deep to cover levels.
    for (let i = 0; i < 12; i += 1) {
      seq.push({ folder: i % 2 === 0 ? pick(byDepthShallow, i) : pick(byDepthDeep, i), tag: "cold" });
    }
    // 2) switch-out-and-back pairs (revisit a just-left folder).
    for (let i = 0; i < 4; i += 1) {
      const a = pick(pool, i * 3);
      const b = pick(pool, i * 3 + 1);
      seq.push({ folder: a, tag: "outback-a" });
      seq.push({ folder: b, tag: "outback-b" });
      seq.push({ folder: a, tag: "outback-return" });
    }
    // 3) rapid consecutive switches across the whole pool.
    for (let i = 0; i < 6; i += 1) {
      seq.push({ folder: pick(pool, i * 5 + 2), tag: "rapid" });
    }
  }
  // Trim/pad to exactly the switch budget.
  const SWITCH_COUNT = Math.min(30, Math.max(seq.length, 0));
  log(`running ${SWITCH_COUNT} folder switches (cold/hot/out-back/rapid) over depths [${depths.join(", ")}]`);

  let op = 0;
  for (let i = 0; i < SWITCH_COUNT; i += 1) {
    const entry = seq[i];
    if (!entry?.folder) continue;
    await switchToFolder(cdp, snap, op, entry.folder.id, `switch:${entry.tag}:d${entry.folder.depth}`);
    op += 1;

    // Interleave the other real interactions so switches happen amid scrolling
    // and preview open/close (mirrors real use, and stresses races).
    if (i % 3 === 0) {
      // Grid deep scroll in the just-opened folder.
      await cdp.run(
        `(() => { const g = document.querySelector(".virtual-grid"); if (g) g.scrollTop = Math.round((g.scrollHeight - g.clientHeight) * ${(0.2 + ((i * 37) % 60) / 100).toFixed(3)}); })()`
      );
      await delay(300);
      const s = await snap();
      if (s.scrollTop > 200 && s.tiles === 0) fail(op, "deep-scroll blank viewport", `scrollTop=${s.scrollTop}`);
      op += 1;
    }
    if (i % 4 === 1) {
      // Open + close a preview — but only when the current view actually has a
      // media tile. A subfolders-only view (e.g. the root with "show subfolder
      // contents" on) renders .subfolder-card covers, not .media-tile, so there
      // is nothing to preview; that is not a hang.
      const hasTile = await cdp.evalJson(`Boolean(document.querySelector(".media-tile"))`);
      if (hasTile) {
        const t0 = Date.now();
        // Double-click and confirm a tile actually received it (guards the race
        // where a scan-refresh replaces tiles between the check and the click).
        const clicked = await cdp.evalJson(
          `(() => { const t = document.querySelector(".media-tile"); if (!t) return false; t.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })); return true; })()`
        );
        if (clicked) {
          let previewMs = null;
          for (let k = 0; k < 70; k += 1) {
            const s = await snap();
            if (s.previewReady) {
              previewMs = Date.now() - t0;
              break;
            }
            await delay(60);
          }
          if (previewMs === null) fail(op, "preview never became ready", `${Date.now() - t0}ms`);
          else {
            record("previewOpen", previewMs);
            if (previewMs > BUDGET.previewOpenMs) fail(op, "preview slow", `${previewMs}ms`);
          }
          await cdp.run(`document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`);
          await delay(150);
          op += 1;
        }
      }
    }
    if (i % 5 === 2) {
      await scrollTree(cdp, snap, op);
      op += 1;
    }

    const s = await snap();
    if (s.errorStrip) fail(op, "error strip visible", s.errorStrip);
    if (i % 8 === 4) await cdp.screenshot(path.join(SHOT_DIR, `switch${i}.png`));
  }
  log(`completed ${op} total operations (${SWITCH_COUNT} switches + interleaved scroll/preview/tree)`);

  await cdp.screenshot(path.join(SHOT_DIR, "99-final.png"));

  // Summarize.
  const summary = {};
  for (const [kind, samples] of Object.entries(results.samples)) {
    const sorted = [...samples].sort((a, b) => a - b);
    summary[kind] = {
      count: sorted.length,
      p50: sorted[Math.floor(sorted.length * 0.5)] ?? null,
      p95: sorted[Math.max(0, Math.ceil(sorted.length * 0.95) - 1)] ?? null,
      max: sorted[sorted.length - 1] ?? null
    };
  }
  log("summary:", JSON.stringify(summary, null, 2));
  log(`console errors: ${cdp.consoleErrors.length}`);
  if (cdp.consoleErrors.length) log("  " + cdp.consoleErrors.slice(0, 8).join("\n  "));
  log(`failures: ${results.failures.length}`);
  writeFileSync(
    path.join(DATA_DIR, "summary.json"),
    JSON.stringify({ summary, failures: results.failures, consoleErrors: cdp.consoleErrors }, null, 2)
  );
  log(`screenshots + summary in ${DATA_DIR}`);
  cdp.close();
  if (results.failures.length > 0 || cdp.consoleErrors.length > 0) {
    throw new Error(`${results.failures.length} interaction failure(s), ${cdp.consoleErrors.length} console error(s)`);
  }
}

main()
  .then(() => log("=== PASS ==="))
  .catch((error) => {
    console.error("[interaction] FAILED:", error.message || error);
    process.exitCode = 1;
  })
  .finally(async () => {
    await stop();
    if (!KEEP) {
      try {
        rmSync(DB_PATH, { force: true });
        rmSync(`${DB_PATH}-wal`, { force: true });
        rmSync(`${DB_PATH}-shm`, { force: true });
      } catch {}
    } else {
      log(`MEGLE_KEEP_DATA=1, kept ${DATA_DIR}`);
    }
  });

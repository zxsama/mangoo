import { existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function requireFile(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath) || !statSync(absolutePath).isFile()) {
    fail(`missing file ${relativePath}`);
  }
}

function requireDir(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath) || !statSync(absolutePath).isDirectory()) {
    fail(`missing directory ${relativePath}`);
  }
}

function forbidPath(relativePath) {
  if (existsSync(path.join(root, relativePath))) {
    fail(`generated path should not be present: ${relativePath}`);
  }
}

function forbidLegacyPath(relativePath) {
  if (existsSync(path.join(root, relativePath))) {
    fail(`retired path should not be present: ${relativePath}`);
  }
}

function readJson(relativePath) {
  return JSON.parse(readFileSync(path.join(root, relativePath), "utf8"));
}

function readText(relativePath) {
  return readFileSync(path.join(root, relativePath), "utf8").replace(/\r\n/g, "\n");
}

function markdownSection(document, headingAliases) {
  const lines = document.replace(/<!--[\s\S]*?-->/g, "").split("\n");
  const aliases = new Set(headingAliases.map((heading) => heading.toLocaleLowerCase()));
  for (let index = 0; index < lines.length; index += 1) {
    const heading = lines[index].match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (!heading || !aliases.has(heading[2].toLocaleLowerCase())) continue;
    const level = heading[1].length;
    let end = index + 1;
    while (end < lines.length) {
      const nextHeading = lines[end].match(/^(#{1,6})\s+/);
      if (nextHeading && nextHeading[1].length <= level) break;
      end += 1;
    }
    return lines.slice(index + 1, end).join("\n").trim();
  }
  return null;
}

function requireDocumentSection(documentPath, headingAliases, requiredMarkers = []) {
  const section = markdownSection(readText(documentPath), headingAliases);
  if (section === null) {
    fail(`${documentPath} missing section heading: ${headingAliases.join(" / ")}`);
    return "";
  }
  for (const marker of requiredMarkers) {
    if (!section.includes(marker)) {
      fail(`${documentPath} section ${headingAliases[0]} missing semantic marker: ${marker}`);
    }
  }
  return section;
}

function normalizedGuidanceText(source) {
  return source
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/[‐‑‒–—−]/g, "-")
    .replace(/\s+/g, " ")
    .trim();
}

function markdownGuidanceStatements(document) {
  const lines = document.replace(/<!--[\s\S]*?-->/g, "").split("\n");
  const headings = [];
  const statements = [];
  for (const line of lines) {
    const heading = line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (heading) {
      const level = heading[1].length;
      headings.length = level - 1;
      headings[level - 1] = heading[2];
      statements.push(headings.filter(Boolean).join(" "));
      continue;
    }
    const content = line.trim();
    if (!content) continue;
    for (const sentence of content.split(/[.!?。！？;；]+/)) {
      const trimmed = sentence.trim();
      if (trimmed) statements.push([...headings.filter(Boolean), trimmed].join(" "));
    }
  }
  return statements;
}

function findRetiredSurfaceRequirement(document) {
  const surfacePatterns = [
    /(?<![\p{L}\p{N}_-])task(?:[\s-]+)?(?:drawers?|centers?)(?![\p{L}\p{N}_-])/iu,
    /(?<![\p{L}\p{N}_-])recent(?:[\s-]+)?(?:ops|operations)(?:[\s-]+)?(?:panels?|drawers?)(?![\p{L}\p{N}_-])/iu,
    /(?:任务抽屉|任务中心|最近操作(?:面板|抽屉))/u
  ];
  const retiredSurfaceProhibition =
    /\b(?:(?:(?:do|must|should)\s+not|never)\s+(?:be\s+)?(?:reintroduce(?:d)?|restore(?:d)?|use(?:d)?|add(?:ed)?|render(?:ed)?|open(?:ed)?|implement(?:ed)?|require(?:d)?|provide(?:d)?|keep|kept|show(?:n)?|bring\s+back|brought\s+back)|avoid\s+(?:reintroducing|restoring|using|adding|rendering|opening|implementing|requiring|providing|keeping|showing|bringing\s+back))\b|(?:(?:不要|不得|禁止)(?:再|重新)?|不再)(?:引入|恢复|使用|添加|提供|保留|展示|打开|实现|要求)/u;
  const explicitRetiredStatus =
    /\b(?:must|should|will)\s+(?:remain|stay)\s+(?:retired|removed|replaced|superseded|deprecated)\b|(?:必须|应该|将)(?:保持|继续)(?:退役|已移除|已删除|已取代|已替换)/u;
  const historicalContext =
    /\b(?:historical|history|legacy|retired|removed|replaced|superseded|deprecated|formerly|previously|no\s+longer)\b|(?:历史|旧版|退役|已移除|已删除|已取代|已替换|不再)/u;
  const positiveRequirement =
    /\b(?:must|should|required|require|use|restore|reintroduce|add|provide|keep|show|render|open|implement)\b|(?:必须|应该|需要|使用|恢复|重新引入|添加|提供|保留|展示|打开|实现)/u;

  for (const statement of markdownGuidanceStatements(document)) {
    const normalized = normalizedGuidanceText(statement);
    const matchedSurface = surfacePatterns.map((pattern) => normalized.match(pattern)?.[0]).find(Boolean);
    if (!matchedSurface) continue;
    if (retiredSurfaceProhibition.test(normalized) || explicitRetiredStatus.test(normalized)) continue;
    if (historicalContext.test(normalized) && !positiveRequirement.test(normalized)) continue;
    return matchedSurface;
  }
  return null;
}

function missingNotificationReadmeConcepts(section) {
  const normalized = normalizedGuidanceText(section);
  const crossLibrary =
    /\b(?:all|every|cross)[\s-]+librar(?:y|ies)\b/u.test(normalized) ||
    /\bacross\s+(?:(?:all|every)\s+)?librar(?:y|ies)\b/u.test(normalized) ||
    /(?:跨(?:资源|资料)?库|全(?:资源|资料)?库|所有(?:资源|资料)?库)/u.test(normalized);
  const persistent =
    /\b(?:persist\w*|durable)\b/u.test(normalized) ||
    /(?:持久化|持久|持续保留|重启后(?:仍|继续)?保留)/u.test(normalized);
  const aggregation =
    /\b(?:aggregat\w*|group\w*|consolidat\w*|summari[sz]\w*)\b/u.test(normalized) ||
    /(?:聚合|汇总|合并|归并)/u.test(normalized);
  const task =
    /\b(?:tasks?|background\s+(?:work|jobs?|operations?))\b/u.test(normalized) ||
    /(?:任务|后台(?:工作|任务|操作))/u.test(normalized);
  const progress = /\bprogress\b/u.test(normalized) || /进度/u.test(normalized);
  const local = /\blocal(?:ly)?\b/u.test(normalized) || /(?:本地|本机|会话内)/u.test(normalized);
  const processLogs =
    /\b(?:process(?:es)?|runtime|application)\s+logs?\b/u.test(normalized) ||
    /\blogs?\s+(?:from\s+)?(?:the\s+)?(?:local(?:ly)?\s+)?(?:process(?:es)?|runtime|application)\b/u.test(normalized) ||
    /(?:进程日志|运行日志)/u.test(normalized);

  const missing = [];
  if (!crossLibrary || !persistent) missing.push("persistent all-library");
  if (!aggregation || !task || !progress) missing.push("aggregated task progress");
  if (!local || !processLogs) missing.push("local process logs");
  return missing;
}

function assertMarkdownSectionContracts() {
  const probe = `
<!--
## Unified Notification Center
comment-only-marker
-->
outside-marker
## Unified Notification Center
current-marker
## Next Section
after-marker
`;
  const section = markdownSection(probe, ["Unified Notification Center"]);
  if (
    section !== "current-marker" ||
    section.includes("comment-only-marker") ||
    section.includes("outside-marker") ||
    section.includes("after-marker")
  ) {
    fail("Markdown section checks must ignore comments and markers outside the active section");
  }
}

function assertRetiredSurfaceGuidanceContracts() {
  for (const requiredGuidance of [
    "Use the Task Drawer for active work.",
    "Task-Drawers are required for background jobs.",
    "TaskCenter is required for background jobs.",
    "The task-center is required for background jobs.",
    "Render the Recent Ops Panel beside the library.",
    "RecentOpsPanel should be used beside the library.",
    "Show the Recent Ops Drawer beside the library.",
    "Recent-Operations Panels are required for audit history.",
    "Open the recent-operations drawer for audit history.",
    "The legacy Task Center should be restored.",
    "Avoid removing the Task Drawer.",
    "The Task Center must not be removed.",
    "不要移除任务抽屉。"
  ]) {
    if (findRetiredSurfaceRequirement(requiredGuidance) === null) {
      fail(`retired surface guidance guard must reject: ${requiredGuidance}`);
    }
  }
  for (const allowedGuidance of [
    "### Historical Design\nThe Task Drawer and Task Center were replaced by the notification center.",
    "The recent-ops panel is retired and no longer part of the product.",
    "The Task Drawer should remain retired.",
    "Do not reintroduce the Recent Operations Drawer.",
    "不得重新引入最近操作面板。",
    "Avoid reintroducing the task-center.",
    "The notification-task-center metric name remains valid.",
    "The notification-recent-ops-panel metric name remains valid.",
    "Task centerline measurements are unrelated."
  ]) {
    if (findRetiredSurfaceRequirement(allowedGuidance) !== null) {
      fail(`retired surface guidance guard must allow explicit historical/retired context: ${allowedGuidance}`);
    }
  }
}

function assertNotificationReadmeConceptContracts() {
  for (const validSection of [
    "A persistent all-library notification center with aggregated task progress and local process logs.",
    "Notifications persist across libraries and aggregate task progress with logs from local processes.",
    "持久化跨库通知中心，汇总后台任务进度，并提供本地进程日志。",
    "面向所有资料库的通知会持续保留，汇总后台工作进度及本机运行日志。"
  ]) {
    if (missingNotificationReadmeConcepts(validSection).length !== 0) {
      fail(`README notification concepts must allow equivalent English/Chinese wording: ${validSection}`);
    }
  }
  const emptyMissing = missingNotificationReadmeConcepts("");
  if (emptyMissing.length !== 3) {
    fail("README notification concepts must reject an empty section");
  }
  const boundedEmptySection = markdownSection(
    "## Unified Notification Center\n## Start Here\nPersistent all-library progress and local process logs.",
    ["Unified Notification Center"]
  );
  if (boundedEmptySection === null || missingNotificationReadmeConcepts(boundedEmptySection).length !== 3) {
    fail("README notification concepts must not be satisfied by content after the next section heading");
  }
  const partialMissing = missingNotificationReadmeConcepts(
    "A durable cross-library notification center with local runtime logs."
  );
  if (partialMissing.length !== 1 || partialMissing[0] !== "aggregated task progress") {
    fail("README notification concepts must report the specifically missing concept");
  }
}

assertMarkdownSectionContracts();
assertRetiredSurfaceGuidanceContracts();
assertNotificationReadmeConceptContracts();

const requiredDirs = [
  "apps/desktop/src",
  "apps/web/src",
  "contracts/core-api",
  "contracts/plugins",
  "crates/core/src",
  "crates/core/migrations",
  "docs/performance-results/raw/2026-05-16",
  "packages/core-client",
  "packages/notification-contract",
  "tests",
  "tools/bench",
  "tools/checks",
  "tools/dev"
];

const requiredFiles = [
  ".gitignore",
  "package.json",
  "tsconfig.base.json",
  "Cargo.toml",
  "apps/desktop/package.json",
  "apps/desktop/src/core-process.ts",
  "apps/desktop/src/core-session.ts",
  "apps/desktop/src/main.ts",
  "apps/desktop/src/notification-ipc.ts",
  "apps/desktop/src/notification-service.ts",
  "apps/desktop/src/notification-store.ts",
  "apps/desktop/src/preload.cjs",
  "apps/web/package.json",
  "apps/web/src/core/client.ts",
  "apps/web/src/core/types.ts",
  "apps/web/src/core/useLibraryData.ts",
  "apps/web/src/features/notifications/NotificationHistoryView.tsx",
  "apps/web/src/features/notifications/NotificationPopover.tsx",
  "apps/web/src/features/notifications/NotificationStatusView.tsx",
  "apps/web/src/features/notifications/NotificationWindow.tsx",
  "apps/web/src/features/notifications/ProcessLogView.tsx",
  "apps/web/src/features/notifications/useNotifications.ts",
  "packages/core-client/package.json",
  "packages/core-client/src/index.ts",
  "packages/core-client/src/generated-contract.ts",
  "packages/core-client/src/client.ts",
  "packages/notification-contract/package.json",
  "packages/notification-contract/src/index.ts",
  "contracts/core-api/openapi.yaml",
  "contracts/plugins/manifest.schema.json",
  "crates/core/Cargo.toml",
  "crates/core/migrations/0001_initial.sql",
  "crates/core/migrations/0021_notification_outbox.sql",
  "crates/core/src/db/notification_outbox.rs",
  "docs/project-structure.md",
  "docs/testing-strategy.md",
  "tools/dev/run-dev.mjs"
];

const forbiddenGeneratedPaths = [
  "bench-results",
  "tools/bench/fs-scan/results",
  "tools/bench/thumbnail/results",
  "tools/bench/virtual-grid/results",
  "tools/bench/preview-switch/results",
  "tools/bench/preview-switch/public/thumbs",
  "tools/bench/virtual-grid/node_modules",
  "tools/bench/thumbnail/node_modules",
  "tools/bench/preview-switch/node_modules"
];

const forbiddenLegacyPaths = [
  `apps/web/src/features/tasks/${["Task", "Panel"].join("")}.tsx`,
  `apps/web/src/features/tasks/${["Task", "Center"].join("")}.tsx`,
  `apps/web/src/features/tasks/${["Task", "Overlay"].join("")}.tsx`,
  `apps/web/src/features/file-ops/${["Recent", "Ops", "Panel"].join("")}.tsx`
];

for (const dir of requiredDirs) requireDir(dir);
for (const file of requiredFiles) requireFile(file);
for (const generatedPath of forbiddenGeneratedPaths) forbidPath(generatedPath);
for (const legacyPath of forbiddenLegacyPaths) forbidLegacyPath(legacyPath);

const packageJson = readJson("package.json");
if (!packageJson.workspaces?.includes("packages/notification-contract")) {
  fail("root workspaces must include packages/notification-contract");
}
for (const scriptName of [
  "check:structure",
  "check:core-api",
  "check:notification-contract",
  "check:schema",
  "check:rust",
  "test"
]) {
  if (!packageJson.scripts?.[scriptName]) {
    fail(`package.json missing script ${scriptName}`);
  }
}

const rootCargo = readFileSync(path.join(root, "Cargo.toml"), "utf8");
if (!rootCargo.includes('"crates/core"')) {
  fail("Cargo workspace must include crates/core");
}
if (rootCargo.includes('"crates/indexer"') || rootCargo.includes('"crates/thumbnails"')) {
  fail("Phase 1 should not pre-split indexer/thumbnails into workspace crates");
}

const openApi = readFileSync(path.join(root, "contracts/core-api/openapi.yaml"), "utf8");
for (const pathFragment of [
  "/health:",
  "/roots:",
  "/media:",
  "/tasks:",
  "/file-ops/rename:",
  "/plugins:"
]) {
  if (!openApi.includes(pathFragment)) {
    fail(`Core API contract missing ${pathFragment}`);
  }
}

const pluginManifest = readJson("contracts/plugins/manifest.schema.json");
const capabilities = pluginManifest.properties?.capabilities?.items?.enum ?? [];
for (const capability of ["decoder", "metadata", "action", "import-provider"]) {
  if (!capabilities.includes(capability)) {
    fail(`plugin manifest missing capability ${capability}`);
  }
}

const readmeNotificationSection = requireDocumentSection(
  "README.md",
  ["Unified Notification Center", "统一通知中心"]
);
for (const missingConcept of missingNotificationReadmeConcepts(readmeNotificationSection)) {
  fail(`README.md Unified Notification Center section missing concept: ${missingConcept}`);
}
requireDocumentSection(
  "CLAUDE.md",
  ["Unified Notification Center", "统一通知中心"],
  [
    "notification_operation_heads",
    "notification_outbox",
    "apps/desktop/src/notification-service.ts",
    "packages/notification-contract",
    "apps/web/src/features/notifications/"
  ]
);
requireDocumentSection(
  "CLAUDE.md",
  ["Contract-First Boundaries", "契约优先边界"],
  [
    "crates/core/migrations/0024_file_operation_object_binding.sql",
    "apply_interactive_folder_scan_task_migration",
    "tools/checks/validate_sqlite_schema.py"
  ]
);
requireDocumentSection(
  "docs/project-structure.md",
  ["Unified Notification Center Modules", "统一通知中心模块"],
  [
    "crates/core/src/db/notification_outbox.rs",
    "apps/desktop/src/notification-service.ts",
    "packages/notification-contract",
    "apps/web/src/features/notifications/",
    "operation_group_id",
    "history_only",
    "status_only"
  ]
);
requireDocumentSection(
  "docs/architecture.md",
  ["Unified Notification Center", "统一通知中心"],
  ["notification_operation_heads", "notification_outbox", "notifications.sqlite", "@megle/notification-contract"]
);
requireDocumentSection(
  "CLAUDE.md",
  ["Historical Phase Summary (1–10)", "历史阶段摘要（1–10）"]
);
const retiredSurfaceRequirement = findRetiredSurfaceRequirement(readText("CLAUDE.md"));
if (retiredSurfaceRequirement) {
  fail(`CLAUDE.md current guidance must not require retired surface: ${retiredSurfaceRequirement}`);
}

if (!process.exitCode) {
  console.log("PASS: structure and contracts");
}

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

for (const [documentPath, requiredMarkers] of [
  [
    "README.md",
    ["persistent all-library notification center", "aggregated task progress", "local process logs"]
  ],
  [
    "CLAUDE.md",
    [
      "### Unified Notification Center",
      "notification_operation_heads",
      "userData/notifications.sqlite",
      "@megle/notification-contract",
      "no notification polling",
      "no per-thumbnail notifications"
    ]
  ],
  [
    "docs/project-structure.md",
    ["## Unified Notification Center Modules", "logical grouping key", "presentation class"]
  ],
  ["docs/architecture.md", ["## Unified Notification Center", "notification_outbox"]]
]) {
  const document = readFileSync(path.join(root, documentPath), "utf8");
  for (const marker of requiredMarkers) {
    if (!document.includes(marker)) {
      fail(`${documentPath} missing unified notification documentation marker: ${marker}`);
    }
  }
}

if (!process.exitCode) {
  console.log("PASS: structure and contracts");
}

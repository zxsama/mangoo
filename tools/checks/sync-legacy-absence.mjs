import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { root } from "./sync-release-lib.mjs";

const checks = [
  {
    file: "crates/core/src/main.rs",
    required: ["MEGLE_HOST_DB_PATH", "MEGLE_LIBRARIES_DIR", "AppState::new_host"],
    forbidden: ["MEGLE_DB_PATH", "MEGLE_ROOT_PATH", "open_legacy_catalog", "adopt_legacy_root"],
  },
  {
    file: "crates/core/src/library_manager.rs",
    required: ["create_library", "restore_active", "atomic_write_marker_guarded"],
    forbidden: ["MEGLE_DB_PATH", "legacy_root", "adopt_existing_root", "select_catalog_path"],
  },
  {
    file: "apps/desktop/src/core-process.ts",
    required: ["MEGLE_HOST_DB_PATH", "MEGLE_LIBRARIES_DIR"],
    forbidden: ["MEGLE_DB_PATH", "libraries.json", "sessionForDbPath"],
  },
  {
    file: "apps/desktop/src/main.ts",
    required: ["createColdStartSession", "hostDbPath", "librariesDir"],
    forbidden: ["MEGLE_DB_PATH", "libraries.json", "restartCoreForLibrary", "pick-library-file", "open-library"],
  },
  {
    file: "tools/dev/managed-library-harness.mjs",
    required: ["MEGLE_HOST_DB_PATH", "MEGLE_LIBRARIES_DIR", "createAndActivateManagedLibrary"],
    forbidden: ["MEGLE_DB_PATH", "MEGLE_ROOT_PATH", "adoptExisting"],
  },
];

const violations = [];
for (const check of checks) {
  const source = await readFile(path.join(root, check.file), "utf8");
  for (const marker of check.required) {
    if (!source.includes(marker)) violations.push(`${check.file}: missing managed Host marker ${marker}`);
  }
  for (const marker of check.forbidden) {
    if (source.includes(marker)) violations.push(`${check.file}: retained legacy selector/adoption marker ${marker}`);
  }
}

const result = {
  schemaVersion: 1,
  kind: "sync-legacy-absence",
  checkedFiles: checks.map((check) => check.file),
  requiredMarkers: checks.flatMap((check) => check.required.map((marker) => `${check.file}:${marker}`)),
  forbiddenMarkers: checks.flatMap((check) => check.forbidden.map((marker) => `${check.file}:${marker}`)),
  violations,
  passed: violations.length === 0,
};
console.log(`SYNC_LEGACY_ABSENCE_JSON=${JSON.stringify(result)}`);
if (!result.passed) process.exitCode = 1;

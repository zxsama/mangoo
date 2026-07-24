import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const source = readFileSync(
  new URL("../src/core/useLibraryData.ts", import.meta.url),
  "utf8"
);
const appSource = readFileSync(new URL("../src/app/App.tsx", import.meta.url), "utf8");
const settingsWindowSource = readFileSync(
  new URL("../src/features/settings/SettingsWindow.tsx", import.meta.url),
  "utf8"
);
const settingsPanelsSource = readFileSync(
  new URL("../src/features/settings/settingsPanels.tsx", import.meta.url),
  "utf8"
);
const libraryViewSource = readFileSync(
  new URL("../src/features/library/LibraryView.tsx", import.meta.url),
  "utf8"
);
const stylesSource = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

describe("legacy Recent runtime removal", () => {
  it("does not list legacy file-operation history while browsing or after commands", () => {
    assert.doesNotMatch(source, /\blistFileOperations\b/);
    assert.doesNotMatch(source, /\bloadRecentOps\b/);
    assert.doesNotMatch(source, /\brecentOps(?:Loading)?\b/);
  });

  it("keeps file-operation commands and refreshes the authoritative library view", () => {
    for (const command of ["renameFileOp", "moveFileOps", "deleteFileOps"]) {
      assert.match(source, new RegExp(`client\\.${command}\\(`));
    }
    assert.match(source, /await\s+loadLibrary\(\)/);
  });

  it("routes broad workspace status through the renderer notification boundary", () => {
    assert.match(source, /publishThumbnailCacheStats\(stats, requestLibraryContext/);
    assert.match(source, /createLibraryErrorReportAttempt/);
    assert.match(source, /reportLibraryErrorAndRethrow/);
    assert.match(source, /thumbnailCacheStatsLibraryId/);
    assert.doesNotMatch(source, /\bsetError\b/);
    assert.doesNotMatch(source, /\berror:\s*string\s*\|\s*null/);

    const fileOperationSection = source.slice(
      source.indexOf("const renameFile ="),
      source.indexOf("const generateCurrentFolderThumbnailCache =")
    );
    assert.doesNotMatch(fileOperationSection, /reportLibrary(?:Error|Failure)\(/);

    const perThumbnailRequestSection = source.slice(
      source.indexOf("const requestThumbnailStates ="),
      source.indexOf("const loadFolderChildren =")
    );
    assert.doesNotMatch(perThumbnailRequestSection, /reportLibrary(?:Error|Failure)\(/);
  });

  it("keeps compact notifications read-only and leaves refresh to the shared stats schedule", () => {
    assert.doesNotMatch(libraryViewSource, /error-strip|library\.error/);
    assert.doesNotMatch(stylesSource, /\.error-strip\s*\{/);
    const compactOpenSection = appSource.slice(
      appSource.indexOf("const notificationsOpen"),
      appSource.indexOf("const shellRightActions")
    );
    assert.doesNotMatch(
      compactOpenSection,
      /refreshThumbnailCacheStats|queueMicrotask|setInterval|setTimeout/
    );
    assert.doesNotMatch(
      compactOpenSection,
      /useEffect\(/,
      "StrictMode effect replay must not create compact-open requests"
    );
    assert.match(settingsPanelsSource, /statsPollActive\s*\?\s*8000\s*:\s*20000/);
    assert.match(settingsPanelsSource, /window\.setInterval\(/);

    const navigationReloadSection = source.slice(
      source.indexOf("const selectLibraryFolder ="),
      source.indexOf("const navigateFolderHistory =")
    );
    assert.doesNotMatch(navigationReloadSection, /reportLibraryFailure\([\s\S]*navigation-load-failed/);
  });

  it("opens Settings failure details without republishing a stale local snapshot", () => {
    assert.doesNotMatch(settingsPanelsSource, /publishThumbnailCacheStats/);
    assert.match(settingsPanelsSource, /openNotificationsWindow\(intent\)/);
  });

  it("propagates scan failures to backoff and handles every fire-and-forget task refresh", () => {
    const scanRefreshSection = source.slice(
      source.indexOf("const refreshCurrentScanView ="),
      source.indexOf("const loadLibrary =")
    );
    const scanBackoffSection = source.slice(
      source.indexOf("if (!scanActiveRootTask || scanRefreshFailures >= 3)"),
      source.indexOf("if (!scanActiveRootTask) {", source.indexOf("if (!scanActiveRootTask || scanRefreshFailures >= 3)"))
    );
    const loadTasksSection = source.slice(
      source.indexOf("const loadTasks ="),
      source.indexOf("const createScanRefreshSelectionToken =")
    );
    assert.match(scanRefreshSection, /reportLibraryErrorAndRethrow/);
    assert.match(scanRefreshSection, /throw|return\s+reportLibraryErrorAndRethrow/);
    assert.match(scanBackoffSection, /setScanRefreshFailures\(0\)/);
    assert.match(scanBackoffSection, /setScanRefreshFailures\(\(failures\)\s*=>\s*failures\s*\+\s*1\)/);
    assert.match(scanBackoffSection, /retryLibraryErrorReport\(cause\)/);
    assert.doesNotMatch(scanBackoffSection, /reportLibraryFailure\(/);
    assert.doesNotMatch(source, /void\s+loadTasks\(\);/);
    assert.match(source, /const loadTasksSafely = useCallback\([\s\S]*?catch \(cause\)[\s\S]*?await reportLibraryFailure/);
    assert.match(loadTasksSection, /reportLibraryErrorAndRethrow/);
    assert.match(loadTasksSection, /requestGeneration === libraryRequestGenerationRef\.current/);
    assert.match(scanRefreshSection, /void loadTasksSafely\(/);
  });

  it("provides captured active-library context from both renderer windows", () => {
    assert.match(appSource, /activeLibraryName,/);
    assert.match(appSource, /phase === "loading-media"[\s\S]*?setActiveLibraryId\(null\)/);
    assert.match(settingsWindowSource, /listLibraries/);
    assert.match(settingsWindowSource, /subscribeLibraryPhase/);
    assert.match(settingsWindowSource, /createLibraryContextTracker/);
    assert.match(settingsWindowSource, /tracker\.onPhase\(phase\)/);
    assert.match(settingsWindowSource, /activeLibraryId/);
    assert.match(settingsWindowSource, /activeLibraryName/);
    assert.match(source, /attempt: LibraryErrorReportAttempt \| null/);
    assert.doesNotMatch(source, /attempt\?: LibraryErrorReportAttempt/);
    assert.match(source, /phase === "loading-media"[\s\S]*?activeLibraryContextRef\.current = null/);
  });
});

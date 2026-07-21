import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const source = readFileSync(
  new URL("../src/core/useLibraryData.ts", import.meta.url),
  "utf8"
);
const appSource = readFileSync(new URL("../src/app/App.tsx", import.meta.url), "utf8");
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
    assert.match(source, /publishThumbnailCacheStats\(stats\)/);
    assert.match(source, /reportLibraryError\(\{/);
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

  it("removes the broad workspace banner while preserving deferred snapshot refresh", () => {
    assert.doesNotMatch(libraryViewSource, /error-strip|library\.error/);
    assert.doesNotMatch(stylesSource, /\.error-strip\s*\{/);
    assert.match(appSource, /notificationThumbnailRefreshRequestedRef/);
    assert.match(appSource, /queueMicrotask\(\(\)\s*=>/);
    assert.match(appSource, /library\.refreshThumbnailCacheStats\(\)/);

    const deferredRefreshSection = appSource.slice(
      appSource.indexOf("notificationThumbnailRefreshRequestedRef"),
      appSource.indexOf("const shellRightActions")
    );
    assert.doesNotMatch(deferredRefreshSection, /setInterval\(/);
  });
});

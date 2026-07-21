import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const source = readFileSync(
  new URL("../src/core/useLibraryData.ts", import.meta.url),
  "utf8"
);

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
});

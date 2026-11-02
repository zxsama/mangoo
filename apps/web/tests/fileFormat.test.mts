import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { resolveFileFormat } from "../src/features/media-grid/fileFormat.ts";

describe("file format presentation", () => {
  it("hides only JPG/JPEG/PNG case-insensitively", () => {
    for (const ext of ["jpg", "JPEG", ".PnG"]) assert.equal(resolveFileFormat({ name: "photo", ext }).showBadge, false);
    for (const ext of ["gif", "webp", "tiff", "psd", "mp4", "mov", "pdf", "zip", "ts", "mp3", "ttf", "unknown"]) {
      const lower = resolveFileFormat({ name: "file", ext });
      const upper = resolveFileFormat({ name: "file", ext: ext.toUpperCase() });
      assert.equal(lower.showBadge, true);
      assert.equal(lower.label, ext.toUpperCase());
      assert.equal(lower.color, upper.color);
    }
  });
  it("classifies artwork without claiming decoder support", () => {
    for (const [ext, category] of [["psd", "image"], ["mov", "video"], ["pdf", "document"], ["7z", "archive"], ["rs", "code"], ["flac", "audio"], ["woff2", "font"], ["glb", "model"], ["odd", "file"]]) {
      assert.equal(resolveFileFormat({ name: "file", ext }).category, category);
    }
  });
  it("handles names, dots, control characters, Unicode and long suffixes deterministically", () => {
    for (const name of ["README", ".gitignore", "file."]) assert.equal(resolveFileFormat({ name, ext: null }).label, "FILE");
    assert.equal(resolveFileFormat({ name: "a.b.PDF", ext: null }).label, "PDF");
    assert.equal(resolveFileFormat({ name: "x", ext: "\u202et\u0000xt" }).label, "TXT");
    for (const ext of ["图像", "long".repeat(100)]) assert.equal(resolveFileFormat({ name: "x", ext }).label, ext.toUpperCase());
  });
});

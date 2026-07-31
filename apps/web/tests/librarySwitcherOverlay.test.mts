import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("library switcher overlay migration", () => {
  const appSource = read("src/app/App.tsx");
  const switcherSource = read("src/features/library/LibrarySwitcher.tsx");
  const libraryGlyphSource = read("src/features/library/libraryGlyph.tsx");
  const settingsPanelsSource = read("src/features/settings/settingsPanels.tsx");

  it("uses the shared root, nested, and modal overlay primitives", () => {
    assert.match(appSource, /isRootOpen\("library"\)/);
    assert.match(switcherSource, /AnchoredOverlay/);
    assert.match(switcherSource, /ModalSurface/);
    assert.match(switcherSource, /surfaceRole="nested"/);
  });

  it("removes local popup and backdrop implementations", () => {
    assert.doesNotMatch(switcherSource, /library-switcher-backdrop/);
    assert.doesNotMatch(
      switcherSource,
      /role="dialog"[^]*onClick=\{\(e\) => e\.stopPropagation/,
    );
  });

  it("keeps the shared icon picker content-only and Settings isolated", () => {
    assert.doesNotMatch(libraryGlyphSource, /OverlaySurface|AnchoredOverlay/);
    assert.doesNotMatch(settingsPanelsSource, /nested-overlay|library-switcher/);
  });
});

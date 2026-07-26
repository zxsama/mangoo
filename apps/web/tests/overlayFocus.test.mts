import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("shared overlay focus contract", () => {
  const provider = read("src/design/overlays/OverlayProvider.tsx");
  const menu = read("src/design/overlays/OverlayMenu.tsx");
  const dialog = read("src/design/overlays/OverlayDialog.tsx");

  it("restores only a connected trigger requested by the dismissed layer", () => {
    assert.match(provider, /restoreFocus/);
    assert.match(provider, /trigger\?\.isConnected|trigger\.isConnected/);
    assert.match(provider, /trigger\?\.focus\(|trigger\.focus\(/);
    assert.match(provider, /focusin/);
  });

  it("keeps one menu keyboard implementation", () => {
    for (const key of ["ArrowUp", "ArrowDown", "Home", "End", "Enter"]) {
      assert.match(menu, new RegExp(JSON.stringify(key).slice(1, -1)));
    }
    assert.match(menu, /["'] ["']/);
    assert.match(menu, /data-overlay-menu-item/);
    assert.match(menu, /disabled|aria-disabled/);
  });

  it("reuses the existing modal focus trap", () => {
    assert.match(dialog, /useOverlayFocusTrap/);
    assert.doesNotMatch(dialog, /document\.addEventListener\(["']keydown["']/);
  });
});

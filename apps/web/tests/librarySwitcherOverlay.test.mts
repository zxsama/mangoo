import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("library switcher overlay migration", () => {
  const appSource = read("src/app/App.tsx");
  const shellTopBarSource = read("src/app-shell/ShellTopBar.tsx");
  const switcherSource = read("src/features/library/LibrarySwitcher.tsx");
  const libraryGlyphSource = read("src/features/library/libraryGlyph.tsx");
  const settingsPanelsSource = read("src/features/settings/settingsPanels.tsx");
  const stylesSource = read("src/styles.css");
  const uiValidatorSource = read("../../tools/checks/validate-ui-design.mjs");

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

  it("wires the titlebar trigger as the controlled library dialog owner", () => {
    assert.match(shellTopBarSource, /aria-controls="library-root"/);
    assert.match(shellTopBarSource, /aria-expanded=\{librarySwitcherOpen\}/);
    assert.match(shellTopBarSource, /aria-haspopup="dialog"/);
    assert.match(shellTopBarSource, /active=\{librarySwitcherOpen\}/);
    assert.match(appSource, /restoreFocusTarget=\{librarySwitcherSource\}/);
    assert.match(appSource, /autoFocus=\{librarySwitcherAutoFocus\}/);
  });

  it("uses shared radio-menu semantics for the overlay icon picker", () => {
    assert.match(libraryGlyphSource, /OverlayMenuList/);
    assert.match(libraryGlyphSource, /OverlayMenuItem/);
    assert.match(libraryGlyphSource, /role="menuitemradio"/);
    assert.match(libraryGlyphSource, /checked=\{/);
  });

  it("keeps retired local LibrarySwitcher surfaces out of the global stylesheet", () => {
    for (const selector of [
      ".library-switcher-backdrop",
      ".library-switcher-confirm-backdrop",
      ".library-switcher-confirm",
      ".library-switcher-remove-menu",
    ]) {
      assert.doesNotMatch(stylesSource, new RegExp(selector.replaceAll(".", "\\.")));
    }
    assert.doesNotMatch(stylesSource, /\.library-switcher\s*\{[^}]*backdrop-filter/s);
    assert.match(uiValidatorSource, /retiredLibrarySwitcherSelectors/);
    assert.match(uiValidatorSource, /retired LibrarySwitcher selector/);
  });
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

const cssRuleSelectors = (source: string) => {
  const withoutComments = source.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...withoutComments.matchAll(/(?:^|[{}])\s*([^{}]+?)\s*\{/g)]
    .map((match) => match[1].trim())
    .filter((selector) => !selector.startsWith("@"));
};

const hasCompleteSelector = (source: string, selector: string) =>
  cssRuleSelectors(source)
    .flatMap((rule) => rule.split(","))
    .some((candidate) => new RegExp(`^${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?:[\\s:.#[>+~]|$)`).test(candidate.trim()));

describe("shared overlay source contract", () => {
  const mainSource = read("src/main.tsx");
  const settingsWindowSource = read("src/features/settings/SettingsWindow.tsx");
  const pluginsWindowSource = read("src/features/plugins/PluginsWindow.tsx");
  const legacyStyles = read("src/styles.css");
  const overlayCss = read("src/design/overlays/overlay.css");
  const anchoredOverlaySource = read("src/design/overlays/AnchoredOverlay.tsx");
  const overlayPositionSource = read("src/design/overlays/overlayPosition.ts");
  const overlaySourceBundle = [
    "overlayTypes.ts",
    "overlayMeasurement.ts",
    "overlayPosition.ts",
    "overlayStack.ts",
    "overlayRuntime.ts",
    "overlayMenuNavigation.ts",
    "overlayPresence.ts",
    "useOverlayPresence.ts",
    "OverlayProvider.tsx",
    "OverlaySurface.tsx",
    "AnchoredOverlay.tsx",
    "OverlayMenu.tsx",
    "useOverlayFocusTrap.ts",
    "OverlayDialog.tsx",
    "OverlayField.tsx",
    "index.ts"
  ].map((path) => read(`src/design/overlays/${path}`)).join("\n");

  it("mounts overlays only around the main app", () => {
    assert.match(mainSource, /design\/overlays\/overlay\.css/);
    assert.match(mainSource, /<OverlayProvider>/);
    assert.doesNotMatch(settingsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
    assert.doesNotMatch(pluginsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
  });

  it("uses existing material variables and no forbidden dependencies", () => {
    assert.match(overlayCss, /var\(--glass-dialog-fill\)/);
    assert.match(overlayCss, /var\(--glass-center-stroke\)/);
    assert.doesNotMatch(overlaySourceBundle, /fetch\s*\(|node:fs|@radix|@floating-ui/);
  });

  it("keeps migrated overlay ownership in overlay.css", () => {
    for (const selector of [
      ".context-menu",
      ".dialog-backdrop",
      ".library-switcher-backdrop",
      ".sort-menu-list",
      ".filter-menu-popover",
      ".notification-popover",
      ".notification-window-body"
    ]) {
      assert.equal(hasCompleteSelector(legacyStyles, selector), false, `${selector} remains in styles.css`);
      assert.equal(hasCompleteSelector(overlayCss, selector), true, `${selector} is missing from overlay.css`);
    }

    assert.doesNotMatch(legacyStyles, /rgba\(20,\s*20,\s*22,\s*0\.98\)/);
    assert.doesNotMatch(legacyStyles, /rgba\(23,\s*27,\s*32,\s*0\.99\)/);
    assert.doesNotMatch(legacyStyles, /#1c1c1e/i);
  });

  it("preserves the locked anchor API and one rendered width path", () => {
    assert.doesNotMatch(
      anchoredOverlaySource,
      /interface AnchoredOverlayProps\s*{[\s\S]*?\boffset\?:[\s\S]*?}/
    );
    assert.match(anchoredOverlaySource, /measureOverlayIntrinsicWidth\(surface\)/);
    assert.match(anchoredOverlaySource, /width:\s*layout\?\.width/);
    assert.match(
      overlayPositionSource,
      /ROOT_OVERLAY_OFFSET_PX,[\s\S]*?from "\.\/overlayTypes\.ts"/
    );
    assert.doesNotMatch(overlayPositionSource, /const ROOT_OVERLAY_OFFSET_PX/);
    assert.doesNotMatch(overlayPositionSource, /const NESTED_OVERLAY_OFFSET_PX/);
  });

  it("uses translation and opacity presence motion without scale", () => {
    assert.doesNotMatch(overlayCss, /scale\s*\(/);
    assert.match(
      overlayCss,
      /overlay-surface-popover\[data-overlay-state="opening"\][\s\S]*?140ms/
    );
    assert.match(
      overlayCss,
      /overlay-surface-popover\[data-overlay-state="closing"\][\s\S]*?140ms/
    );
    assert.match(
      overlayCss,
      /overlay-surface-nested\[data-overlay-state="opening"\][\s\S]*?110ms/
    );
    assert.match(
      overlayCss,
      /overlay-surface-nested\[data-overlay-state="closing"\][\s\S]*?110ms/
    );
    assert.match(overlayCss, /overlay-modal-backdrop\[data-overlay-state="opening"\]/);
    assert.match(overlayCss, /overlay-modal-backdrop\[data-overlay-state="closing"\]/);
    assert.match(overlayCss, /prefers-reduced-motion:\s*reduce[\s\S]*?animation:\s*none/);
  });
});

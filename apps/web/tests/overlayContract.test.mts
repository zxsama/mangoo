import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("shared overlay source contract", () => {
  const mainSource = read("src/main.tsx");
  const settingsWindowSource = read("src/features/settings/SettingsWindow.tsx");
  const pluginsWindowSource = read("src/features/plugins/PluginsWindow.tsx");
  const overlayCss = read("src/design/overlays/overlay.css");
  const overlaySourceBundle = [
    "overlayTypes.ts",
    "overlayPosition.ts",
    "overlayStack.ts",
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
});

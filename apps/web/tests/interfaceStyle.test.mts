import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  DEFAULT_INTERFACE_STYLE,
  INTERFACE_STYLE_LIMITS,
  interfaceStyleToCssVariables,
  normalizeInterfaceStyle
} from "../src/features/settings/interfaceStyle.ts";

const paneVariables = [
  "--secondary-window-pane-dark",
  "--secondary-window-pane-light",
  "--secondary-window-pane-end"
] as const;

function rgbChannels(color: string) {
  return color.slice(0, color.indexOf(" / "));
}

describe("unified panel and control radius", () => {
  test("starts from 8 and ignores every retired radius key", () => {
    const normalized = normalizeInterfaceStyle({
      surfaceCornerRadius: 16,
      controlCornerRadius: 14,
      panelRadius: 12,
      controlRadius: 10
    });

    assert.equal(normalized.panelControlCornerRadius, 8);
    assert.equal("surfaceCornerRadius" in normalized, false);
    assert.equal("controlCornerRadius" in normalized, false);
  });

  test("clamps the combined setting to 0 through 16", () => {
    assert.deepEqual(INTERFACE_STYLE_LIMITS.panelControlCornerRadius, {
      min: 0,
      max: 16,
      step: 1
    });
    assert.equal(
      normalizeInterfaceStyle({ panelControlCornerRadius: -1 }).panelControlCornerRadius,
      0
    );
    assert.equal(
      normalizeInterfaceStyle({ panelControlCornerRadius: 20 }).panelControlCornerRadius,
      16
    );
  });

  test("maps panels controls surfaces and overlays to the same radius", () => {
    for (const radius of [0, 8, 16]) {
      const variables = interfaceStyleToCssVariables({
        ...DEFAULT_INTERFACE_STYLE,
        panelControlCornerRadius: radius
      });

      for (const token of [
        "--radius-overlay",
        "--radius-panel",
        "--radius-surface",
        "--radius-control"
      ]) {
        assert.equal(variables[token], `${radius}px`);
      }
      assert.equal(variables["--radius-tight"], `${Math.max(radius - 4, 0)}px`);
    }
  });
});

describe("secondary window pane colors", () => {
  test("derives distinct pane tints from the default side color", () => {
    const variables = interfaceStyleToCssVariables({ ...DEFAULT_INTERFACE_STYLE });

    assert.equal(variables["--secondary-window-pane-dark"], "rgb(6 9 12 / 0.56)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(52 56 59 / 0.2)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(7 11 14 / 0.48)");
  });

  test("makes every pane transparent when side opacity is zero", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOpacity: 0
    });

    for (const variable of paneVariables) {
      assert.match(variables[variable], / \/ 0\)$/);
    }
  });

  test("makes every pane transparent when side overlay strength is zero", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOverlayStrength: 0
    });

    for (const variable of paneVariables) {
      assert.match(variables[variable], / \/ 0\)$/);
    }
  });

  test("preserves default split contrast when side material controls are maximal", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOpacity: 2,
      sideOverlayStrength: 2
    });

    assert.equal(variables["--secondary-window-pane-dark"], "rgb(6 9 12 / 1)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(52 56 59 / 0.8)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(7 11 14 / 1)");
    assert.equal(variables["--glass-side-fill"], "rgb(8 12 16 / 1)");
  });

  test("preserves split contrast when the selected tint matches the former light target", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOverlayColor: "#d6e1e3",
      sideOpacity: 2,
      sideOverlayStrength: 2
    });

    assert.equal(variables["--glass-side-fill"], "rgb(214 225 227 / 1)");
    assert.equal(variables["--secondary-window-pane-dark"], "rgb(163 171 173 / 1)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(221 230 232 / 0.8)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(188 198 200 / 1)");

    const rootChannels = rgbChannels(variables["--glass-side-fill"]);
    for (const variable of paneVariables) {
      assert.notEqual(rgbChannels(variables[variable]), rootChannels);
    }
  });

  test("darkens the split when the selected tint is pure white", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOverlayColor: "#ffffff",
      sideOpacity: 2,
      sideOverlayStrength: 2
    });

    assert.equal(variables["--glass-side-fill"], "rgb(255 255 255 / 1)");
    assert.equal(variables["--secondary-window-pane-dark"], "rgb(194 194 194 / 1)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(255 255 255 / 0.8)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(224 224 224 / 1)");
    assert.notEqual(
      rgbChannels(variables["--secondary-window-pane-dark"]),
      rgbChannels(variables["--glass-side-fill"])
    );
  });

  test("lightens the split when the selected tint is pure black", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOverlayColor: "#000000",
      sideOpacity: 2,
      sideOverlayStrength: 2
    });

    assert.equal(variables["--glass-side-fill"], "rgb(0 0 0 / 1)");
    assert.equal(variables["--secondary-window-pane-dark"], "rgb(0 0 0 / 1)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(46 46 46 / 0.8)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(0 0 0 / 1)");
    assert.notEqual(
      rgbChannels(variables["--secondary-window-pane-light"]),
      rgbChannels(variables["--glass-side-fill"])
    );
  });
});

describe("dialog content pane colors", () => {
  const dialogPaneVariables = [
    "--glass-dialog-pane-light",
    "--glass-dialog-pane-mid",
    "--glass-dialog-pane-end"
  ] as const;

  test("derives the Settings-content gradient from the center color", () => {
    const variables = interfaceStyleToCssVariables({ ...DEFAULT_INTERFACE_STYLE });

    assert.equal(variables["--glass-dialog-pane-light"], "rgb(52 56 59 / 0.2)");
    assert.equal(variables["--glass-dialog-pane-mid"], "rgb(8 12 16 / 0.38)");
    assert.equal(variables["--glass-dialog-pane-end"], "rgb(7 11 14 / 0.48)");
    assert.equal(variables["--glass-dialog-fill"], variables["--glass-dialog-pane-mid"]);
  });

  test("makes every dialog pane transparent when dialog opacity is zero", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      dialogOpacity: 0
    });

    for (const variable of dialogPaneVariables) {
      assert.match(variables[variable], / \/ 0\)$/);
    }
  });

  test("makes every dialog pane transparent when dialog overlay strength is zero", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      dialogOverlayStrength: 0
    });

    for (const variable of dialogPaneVariables) {
      assert.match(variables[variable], / \/ 0\)$/);
    }
  });
});

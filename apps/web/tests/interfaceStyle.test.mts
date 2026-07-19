import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  DEFAULT_INTERFACE_STYLE,
  interfaceStyleToCssVariables
} from "../src/features/settings/interfaceStyle.ts";

const paneVariables = [
  "--secondary-window-pane-dark",
  "--secondary-window-pane-light",
  "--secondary-window-pane-end"
] as const;

function rgbChannels(color: string) {
  return color.slice(0, color.indexOf(" / "));
}

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

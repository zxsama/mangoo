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

describe("secondary window pane colors", () => {
  test("mixes a cool light tint into the default light pane", () => {
    const variables = interfaceStyleToCssVariables({ ...DEFAULT_INTERFACE_STYLE });

    assert.equal(variables["--secondary-window-pane-dark"], "rgb(8 12 16 / 0.56)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(45 50 54 / 0.2)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(8 12 16 / 0.48)");
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

  test("preserves light-pane contrast when side material controls are maximal", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      sideOpacity: 2,
      sideOverlayStrength: 2
    });

    assert.equal(variables["--secondary-window-pane-dark"], "rgb(8 12 16 / 1)");
    assert.equal(variables["--secondary-window-pane-light"], "rgb(45 50 54 / 0.8)");
    assert.equal(variables["--secondary-window-pane-end"], "rgb(8 12 16 / 1)");
    assert.equal(variables["--glass-side-fill"], "rgb(8 12 16 / 1)");
  });
});

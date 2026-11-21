import assert from "node:assert/strict";
import { it } from "node:test";
import { hexToHsv, hsvToHex, normalizePickerHex } from "../src/design/colorPickerValues.ts";

it("normalizes editable shorthand without accepting partial or invalid colors", () => {
  assert.equal(normalizePickerHex(" CB7 "), "#ccbb77");
  assert.equal(normalizePickerHex("#12Ff30"), "#12ff30");
  for (const invalid of ["#", "#23", "#1234", "rgb(0,0,0)", "#ffg000"]) {
    assert.equal(normalizePickerHex(invalid), null);
  }
});

it("preserves selected colors across all hue sectors and the gray axis", () => {
  for (const hex of ["#ff0000", "#ffff00", "#00ff00", "#00ffff", "#0000ff", "#ff00ff",
    "#000000", "#ffffff", "#777777", "#124578", "#f0eeb2", "#01fefc"]) {
    assert.equal(hsvToHex(hexToHsv(hex)), hex);
  }
  assert.equal(hexToHsv("#777777", 230).hue, 230);
  assert.equal(hsvToHex({ hue: 360, saturation: 100, brightness: 100 }), "#ff0000");
});

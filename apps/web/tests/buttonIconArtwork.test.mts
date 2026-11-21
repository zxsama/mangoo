import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { buttonIconGlyphs, glyphPath, inkBounds } from "../../../tools/build/generate-button-icon-transforms.mjs";

test("button artwork uses each real bold path and preserves all four original painted edges", async t => {
  const css = await readFile(new URL("../src/design/buttonIconArtwork.css", import.meta.url), "utf8");
  const regular = new Map<string, { path: string; transform: string }>();
  const light = new Map<string, string>();
  for (const [, selectors, body] of css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]+)\}/g)) {
    const transform = body.match(/--button-icon-transform:\s*(matrix\([^()]+\));/)?.[1];
    assert.ok(transform, selectors);
    for (const selector of selectors.split(",")) {
      const match = /^\[data-button-icon="(\w+)"\](\[data-icon-weight="light"\])?$/.exec(selector.trim());
      assert.ok(match, selector);
      const [, name, lightOverride] = match;
      if (lightOverride) {
        assert.ok(!light.has(name), name);
        light.set(name, transform);
      } else {
        const path = body.match(/--button-icon-path:\s*path\("([^"]+)"\);/)?.[1];
        assert.ok(path && !regular.has(name), name);
        regular.set(name, { path, transform });
      }
    }
  }

  const glyphs = await buttonIconGlyphs();
  assert.deepEqual(new Set(regular.keys()), new Set(glyphs.keys()));
  assert.deepEqual(new Set(light.keys()), new Set(["ChevronLeft", "ChevronRight"]));
  let maxError = 0;
  // Rerender the checked-in artwork at a different resolution to catch stale
  // paths, incorrect matrices or drift at the actual 20 CSS px design scale.
  const samples = 20 * 32;
  for (const [name, Glyph] of glyphs) {
    const artwork = regular.get(name)!;
    assert.equal(artwork.path, glyphPath(Glyph, "bold"), name);
    const cases = [["regular", artwork.transform]];
    if (light.has(name)) cases.push(["light", light.get(name)!]);
    for (const [weight, transform] of cases) {
      const expected = await inkBounds(glyphPath(Glyph, weight), samples);
      const fitted = await inkBounds(artwork.path, samples, transform);
      const error = Math.max(...expected.map((edge: number, i: number) => Math.abs(edge - fitted[i]) * 20 / 256));
      assert.ok(error < 0.1, `${name} (${weight}) differs by ${error} CSS px`);
      maxError = Math.max(maxError, error);
    }
  }
  t.diagnostic(`${regular.size} button icons + ${light.size} light overrides. Max edge error at 20px: ${maxError.toFixed(4)} CSS px`);
});

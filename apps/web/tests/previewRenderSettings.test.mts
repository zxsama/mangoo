import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  CHANNEL_MATRICES,
  PREVIEW_CHANNELS,
  PREVIEW_RENDER_FILTERS,
  channelFilterId,
  imageRenderingForFilter
} = await import("../src/features/preview/previewRenderSettings.ts");

describe("preview scaling filter", () => {
  it("maps each filter to the CSS keyword that runs that algorithm", () => {
    assert.equal(imageRenderingForFilter("nearest"), "pixelated");
    assert.equal(imageRenderingForFilter("bilinear"), "auto");
  });

  it("names every option after a real algorithm, not a quality word", () => {
    const labels = PREVIEW_RENDER_FILTERS.map((option) => option.label).join(" ");
    // A row labelled by vibe would claim behaviour we do not control. Chromium
    // 150 reports CSS.supports("image-rendering","high-quality") === false, so a
    // "High quality" row would silently render as plain `auto`.
    for (const banned of ["Smooth", "High quality", "Fast"]) {
      assert.ok(!labels.includes(banned), `label must not say "${banned}": ${labels}`);
    }
    assert.ok(/Nearest/i.test(labels), labels);
    assert.ok(/Bilinear/i.test(labels), labels);
  });
});

describe("preview channel isolation", () => {
  it("applies NO filter for the default RGBA view", () => {
    assert.equal(channelFilterId("rgba"), null);
  });

  it("returns a distinct filter id for every non-default channel", () => {
    const ids = ["rgb", "r", "g", "b", "a"].map((c) => channelFilterId(c as never));
    assert.ok(ids.every((id) => typeof id === "string" && id.length > 0), String(ids));
    assert.equal(new Set(ids).size, ids.length, "ids must be unique");
  });

  it("defines a matrix for every id it can return", () => {
    const defined = new Set(CHANNEL_MATRICES.map((m) => m.id));
    for (const option of PREVIEW_CHANNELS) {
      const id = channelFilterId(option.value);
      if (id === null) continue;
      assert.ok(defined.has(id), `no matrix defined for ${option.value} -> ${id}`);
    }
  });

  it("forces alpha to 1 in every matrix so nothing is multiplied by alpha", () => {
    // The point of the feature: a TGA's A channel is data, not transparency,
    // and RGB must show as stored rather than darkened by it.
    for (const matrix of CHANNEL_MATRICES) {
      const nums = matrix.values.trim().split(/\s+/).map(Number);
      assert.equal(nums.length, 20, `${matrix.id} must be a 4x5 matrix`);
      assert.deepEqual(
        nums.slice(15),
        [0, 0, 0, 0, 1],
        `${matrix.id} alpha row must force A=1`
      );
    }
  });

  it("broadcasts a single channel to R, G and B so it reads as greyscale", () => {
    const byId = new Map(CHANNEL_MATRICES.map((m) => [m.id, m.values]));
    for (const [channel, sourceIndex] of [
      ["r", 0],
      ["g", 1],
      ["b", 2]
    ] as const) {
      const nums = byId
        .get(channelFilterId(channel) as string)!
        .trim()
        .split(/\s+/)
        .map(Number);
      for (const outputRow of [0, 1, 2]) {
        assert.equal(
          nums[outputRow * 5 + sourceIndex],
          1,
          `${channel}: output row ${outputRow} must read source ${sourceIndex}`
        );
      }
    }
  });
});

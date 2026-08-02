import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  SCRUBBER_REVEAL_BAND_PX,
  scrubberFractionForIndex,
  scrubberIndexForPointerX,
  shouldShowScrubber
} = await import("../src/features/preview/previewScrubber.ts");

const TRACK = { left: 100, width: 800 };

describe("scrubber pointer mapping", () => {
  it("maps the track ends to the first and last image", () => {
    assert.equal(scrubberIndexForPointerX(100, TRACK, 31), 0);
    assert.equal(scrubberIndexForPointerX(900, TRACK, 31), 30);
  });

  it("maps the midpoint near the middle of the folder", () => {
    const middle = scrubberIndexForPointerX(500, TRACK, 31);
    assert.ok(middle >= 14 && middle <= 16, `got ${middle}`);
  });

  it("clamps a pointer dragged outside the track", () => {
    assert.equal(scrubberIndexForPointerX(-9999, TRACK, 31), 0);
    assert.equal(scrubberIndexForPointerX(9999, TRACK, 31), 30);
  });

  it("survives a single-image folder without dividing by zero", () => {
    for (const x of [100, 500, 900]) {
      const index = scrubberIndexForPointerX(x, TRACK, 1);
      assert.equal(index, 0, `x=${x}`);
      assert.ok(Number.isFinite(index));
    }
    assert.equal(scrubberFractionForIndex(0, 1), 0);
    assert.ok(Number.isFinite(scrubberFractionForIndex(0, 1)));
  });

  it("survives an empty folder without producing NaN", () => {
    const index = scrubberIndexForPointerX(500, TRACK, 0);
    assert.equal(index, 0);
    assert.ok(Number.isFinite(index));
    assert.ok(Number.isFinite(scrubberFractionForIndex(0, 0)));
  });

  it("survives a zero-width track", () => {
    const index = scrubberIndexForPointerX(500, { left: 100, width: 0 }, 31);
    assert.ok(Number.isFinite(index), `got ${index}`);
    assert.ok(index >= 0 && index <= 30);
  });

  it("round-trips index -> fraction -> index across a 20000-image folder", () => {
    // This is the property that makes thumb and image agree: wherever the thumb
    // is drawn, hit-testing that same spot must return the index it represents.
    const total = 20000;
    for (const index of [0, 1, 4999, 10000, 19998, 19999]) {
      const fraction = scrubberFractionForIndex(index, total);
      const pointerX = TRACK.left + fraction * TRACK.width;
      assert.equal(
        scrubberIndexForPointerX(pointerX, TRACK, total),
        index,
        `index ${index} did not round-trip (fraction ${fraction})`
      );
    }
  });

  it("produces the coarse step a large folder implies", () => {
    // 20000 images over 800px is ~25 per pixel. This is inherent to the
    // mapping, not throttling — it is why a drag that looks continuous visits
    // hundreds of images rather than 20000.
    const total = 20000;
    const atX = (x: number) => scrubberIndexForPointerX(x, TRACK, total);
    assert.equal(atX(500), atX(500), "same pixel must be stable");
    const step = atX(501) - atX(500);
    assert.ok(step > 10, `expected a coarse step, got ${step}`);
  });

  it("keeps fractions inside 0..1", () => {
    for (const [index, total] of [
      [0, 31],
      [30, 31],
      [0, 1],
      [19999, 20000]
    ] as const) {
      const fraction = scrubberFractionForIndex(index, total);
      assert.ok(fraction >= 0 && fraction <= 1, `${index}/${total} -> ${fraction}`);
    }
  });
});

describe("scrubber visibility", () => {
  const base = {
    previewOpen: true,
    pinned: false,
    pointerNearBottom: false,
    dragging: false
  };

  it("is hidden by default while the pointer is elsewhere", () => {
    assert.equal(shouldShowScrubber(base), false);
  });

  it("appears when the pointer nears the bottom", () => {
    assert.equal(shouldShowScrubber({ ...base, pointerNearBottom: true }), true);
  });

  it("stays visible when pinned regardless of the pointer", () => {
    assert.equal(shouldShowScrubber({ ...base, pinned: true }), true);
  });

  it("NEVER hides mid-drag, even when the pointer leaves the band", () => {
    // Releasing the thumb outside the bar must not make it vanish under the
    // cursor.
    assert.equal(
      shouldShowScrubber({ ...base, dragging: true, pointerNearBottom: false }),
      true
    );
  });

  it("is hidden whenever the preview is closed, whatever else is true", () => {
    for (const extra of [
      { pinned: true },
      { pointerNearBottom: true },
      { dragging: true },
      { pinned: true, dragging: true, pointerNearBottom: true }
    ]) {
      assert.equal(
        shouldShowScrubber({ ...base, ...extra, previewOpen: false }),
        false,
        JSON.stringify(extra)
      );
    }
  });

  it("exposes a reveal band tall enough to find without hunting", () => {
    assert.ok(SCRUBBER_REVEAL_BAND_PX >= 80, `got ${SCRUBBER_REVEAL_BAND_PX}`);
  });
});

const { DEFAULT_PREVIEW_PREFERENCES, normalizePreviewPreferences } = await import(
  "../src/features/preview/previewPreferences.ts"
);

describe("scrubber pin persistence", () => {
  it("defaults to unpinned", () => {
    assert.equal(DEFAULT_PREVIEW_PREFERENCES.previewScrubberPinned, false);
  });

  it("falls back for a blob written before the key existed", () => {
    const normalized = normalizePreviewPreferences({ previewBufferLimitMb: 900 });
    assert.equal(normalized.previewScrubberPinned, false);
    assert.equal(normalized.previewBufferLimitMb, 900, "unrelated fields survive");
  });

  it("round-trips a pinned bar", () => {
    assert.equal(
      normalizePreviewPreferences({ previewScrubberPinned: true }).previewScrubberPinned,
      true
    );
  });

  it("coerces a non-boolean stored value rather than passing it through", () => {
    assert.equal(
      normalizePreviewPreferences({ previewScrubberPinned: "yes" as never })
        .previewScrubberPinned,
      false
    );
  });
});

import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const { ANIMATION_CAPABLE_EXTENSIONS, canvasPaintTarget, selectPreviewRenderLane } = await import(
  "../src/features/preview/previewRenderLane.ts"
);

describe("preview render lane", () => {
  it("honors static presentation even when legacy kind reports an image", () => {
    assert.equal(selectPreviewRenderLane({ source: "original", kind: "image", ext: "psd", presentation: { previewKind: "static-thumbnail", thumbnail: "available", generation: 1 } }), "image");
  });
  it("sends still images to the canvas fast path", () => {
    for (const ext of ["jpg", "jpeg", "png", "bmp", "tif", "tiff", "heic", "psd"]) {
      assert.equal(
        selectPreviewRenderLane({ source: "original", kind: "image", ext }),
        "canvas",
        ext
      );
    }
  });

  it("keeps animation-capable formats on <img> so they still animate", () => {
    // A canvas paints exactly one bitmap, so an animated GIF/WebP/AVIF would
    // freeze on frame one.
    for (const ext of ["gif", "webp", "avif", "GIF", ".WebP"]) {
      assert.equal(
        selectPreviewRenderLane({ source: "original", kind: "image", ext }),
        "image",
        ext
      );
    }
    assert.deepEqual([...ANIMATION_CAPABLE_EXTENSIONS], ["gif", "webp", "avif"]);
  });

  it("keeps video on <video> and thumbnails on <img>", () => {
    assert.equal(
      selectPreviewRenderLane({ source: "original", kind: "video", ext: "mp4" }),
      "video"
    );
    // The thumbnail fallback is a 320px WebP that decodes instantly; pooling it
    // would spend the preview budget on thumbnails.
    assert.equal(
      selectPreviewRenderLane({ source: "thumbnail", kind: "image", ext: "jpg" }),
      "image"
    );
  });

  it("falls back to <img> when the kind or extension is unknown", () => {
    assert.equal(selectPreviewRenderLane({ source: "original", kind: null, ext: "jpg" }), "image");
    assert.equal(
      selectPreviewRenderLane({ source: "original", kind: "image", ext: null }),
      "image"
    );
    assert.equal(selectPreviewRenderLane({ source: "original", kind: "image", ext: "" }), "image");
  });

  it("sizes the backing store to the bitmap and the css box to natural pixels", () => {
    const target = canvasPaintTarget(
      { width: 3072, height: 1536 },
      { naturalWidth: 16384, naturalHeight: 8192 }
    );

    // 21 MB of backing store instead of 536 MB, while the layout box stays the
    // original's so the wrapper transform and "actual pixels" zoom keep working.
    assert.deepEqual(target, {
      backingWidth: 3072,
      backingHeight: 1536,
      cssWidth: 16384,
      cssHeight: 8192
    });
  });

  it("falls back to the bitmap size when natural dimensions are still pending", () => {
    const target = canvasPaintTarget({ width: 2048, height: 1024 }, null);

    assert.deepEqual(target, {
      backingWidth: 2048,
      backingHeight: 1024,
      cssWidth: 2048,
      cssHeight: 1024
    });
  });

  it("ignores non-positive natural dimensions", () => {
    const target = canvasPaintTarget(
      { width: 800, height: 600 },
      { naturalWidth: 0, naturalHeight: 0 }
    );

    assert.equal(target.cssWidth, 800);
    assert.equal(target.cssHeight, 600);
  });

  it("takes the css box's SHAPE from the bitmap, not from the natural dims", () => {
    // Core and the browser can disagree about orientation (Core stores raw
    // raster; createImageBitmap applies EXIF). Sizing a portrait bitmap into a
    // landscape box letterboxed the image and threw off both fit-long-edge and
    // "actual pixels" zoom. The long edge still comes from `natural`, so the
    // frame is the right SIZE; the aspect comes from what is actually painted.
    const target = canvasPaintTarget(
      { width: 3000, height: 4000 },
      { naturalWidth: 4000, naturalHeight: 3000 }
    );

    assert.equal(target.backingWidth, 3000);
    assert.equal(target.backingHeight, 4000);
    assert.equal(target.cssWidth, 3000, "css box must stay portrait");
    assert.equal(target.cssHeight, 4000);
  });

  it("scales a downscaled tier back up to the original's display size", () => {
    const target = canvasPaintTarget(
      { width: 3072, height: 1536 },
      { naturalWidth: 16384, naturalHeight: 8192 }
    );

    assert.equal(target.cssWidth, 16384);
    assert.equal(target.cssHeight, 8192);
  });

  it("is a no-op when the bitmap already is the natural size", () => {
    const target = canvasPaintTarget(
      { width: 1920, height: 1080 },
      { naturalWidth: 1920, naturalHeight: 1080 }
    );

    assert.deepEqual(target, {
      backingWidth: 1920,
      backingHeight: 1080,
      cssWidth: 1920,
      cssHeight: 1080
    });
  });
});

import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const { canNavigatePreview, canStartPreviewNavigation } = await import(
  "../src/app/previewNavigationGate.ts"
);

describe("preview navigation gate", () => {
  it("allows stepping while a frame is still decoding", () => {
    // The whole point of removing the lock: a held-down arrow key must keep
    // moving instead of waiting on each decode.
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 4,
        itemCount: 31,
        direction: "next"
      }),
      true
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 4,
        itemCount: 31,
        direction: "previous"
      }),
      true
    );
  });

  it("stops at both ends of the list", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 0,
        itemCount: 31,
        direction: "previous"
      }),
      false
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 30,
        itemCount: 31,
        direction: "next"
      }),
      false
    );
  });

  it("still allows the other direction at each end", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 0,
        itemCount: 31,
        direction: "next"
      }),
      true
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 30,
        itemCount: 31,
        direction: "previous"
      }),
      true
    );
  });

  it("refuses to navigate with no selection or an empty list", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: -1,
        itemCount: 31,
        direction: "next"
      }),
      false
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 0,
        itemCount: 0,
        direction: "next"
      }),
      false
    );
  });

  it("handles a single-item list in both directions", () => {
    for (const direction of ["previous", "next"] as const) {
      assert.equal(
        canNavigatePreview({
          previewOpen: true,
          selectedMediaIndex: 0,
          itemCount: 1,
          direction
        }),
        false,
        direction
      );
    }
  });

  it("is inert while the preview is closed", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: false,
        selectedMediaIndex: 4,
        itemCount: 31,
        direction: "next"
      }),
      false
    );
  });
});

describe("first-paint navigation gate", () => {
  const base = {
    previewOpen: true,
    selectedMediaIndex: 4,
    itemCount: 31,
    direction: "next" as const,
    selectedMediaId: 100,
    paintedMediaId: 100
  };

  it("allows a step once the selected media has painted", () => {
    assert.equal(canStartPreviewNavigation(base), true);
  });

  it("refuses while the selected media has not painted yet", () => {
    // The whole feature: selection moved to 101 but the canvas still shows 100,
    // so a further step would skip 101 entirely.
    assert.equal(
      canStartPreviewNavigation({ ...base, selectedMediaId: 101, paintedMediaId: 100 }),
      false
    );
  });

  it("refuses in both directions while unpainted", () => {
    for (const direction of ["previous", "next"] as const) {
      assert.equal(
        canStartPreviewNavigation({
          ...base,
          direction,
          selectedMediaId: 101,
          paintedMediaId: 100
        }),
        false,
        direction
      );
    }
  });

  it("still respects the list boundaries once painted", () => {
    assert.equal(
      canStartPreviewNavigation({ ...base, selectedMediaIndex: 30, direction: "next" }),
      false
    );
    assert.equal(
      canStartPreviewNavigation({ ...base, selectedMediaIndex: 0, direction: "previous" }),
      false
    );
  });

  it("is inert while the preview is closed", () => {
    assert.equal(canStartPreviewNavigation({ ...base, previewOpen: false }), false);
  });

  it("allows the first step when nothing is selected and nothing has painted", () => {
    // Both null means "no frame expected yet", not "a frame is pending".
    // Blocking this would wedge navigation before the first paint ever lands.
    assert.equal(
      canStartPreviewNavigation({ ...base, selectedMediaId: null, paintedMediaId: null }),
      true
    );
  });

  it("refuses when a media is selected but nothing has painted at all", () => {
    assert.equal(
      canStartPreviewNavigation({ ...base, selectedMediaId: 100, paintedMediaId: null }),
      false
    );
  });
});

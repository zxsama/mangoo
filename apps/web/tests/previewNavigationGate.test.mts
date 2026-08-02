import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const { canNavigatePreview } = await import("../src/app/previewNavigationGate.ts");

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

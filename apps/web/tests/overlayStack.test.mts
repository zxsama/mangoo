import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  closeOverlayRoot,
  createOverlayStackState,
  openOverlayRoot,
  popOverlayLayer,
  pushOverlayLayer,
  topOverlayLayer
} from "../src/design/overlays/overlayStack.ts";

const layer = (id: string, rootId: "filter" | "library", kind: "root" | "nested" | "modal") => ({
  id,
  rootId,
  kind,
  trigger: null,
  surface: null,
  dismissOnEscape: true,
  dismissOnFocusOutside: true,
  dismissOnOutsidePointer: true,
  dismissOnWindowBlur: true,
  restoreFocus: true,
  onDismiss: () => undefined
});

describe("overlay stack", () => {
  it("replaces the active root when another titlebar overlay opens", () => {
    let state = openOverlayRoot(createOverlayStackState(), "filter");
    state = pushOverlayLayer(state, layer("filter-root", "filter", "root"));
    state = openOverlayRoot(state, "library");
    assert.equal(state.activeRootId, "library");
    assert.deepEqual(state.layers, []);
  });

  it("keeps nested and modal layers ordered above their root", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    state = pushOverlayLayer(state, layer("icon-picker", "library", "nested"));
    state = pushOverlayLayer(state, layer("remove-confirm", "library", "modal"));
    assert.equal(topOverlayLayer(state)?.id, "remove-confirm");
    state = popOverlayLayer(state, "remove-confirm");
    assert.equal(topOverlayLayer(state)?.id, "icon-picker");
  });

  it("closes only the requested root family", () => {
    let state = openOverlayRoot(createOverlayStackState(), "filter");
    state = pushOverlayLayer(state, layer("filter-root", "filter", "root"));
    state = closeOverlayRoot(state, "filter");
    assert.equal(state.activeRootId, null);
    assert.deepEqual(state.layers, []);
  });
});

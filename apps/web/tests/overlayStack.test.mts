import assert from "node:assert/strict";
import { describe, it } from "node:test";
import * as overlayStack from "../src/design/overlays/overlayStack.ts";

const {
  closeOverlayRoot,
  createOverlayStackState,
  openOverlayRoot,
  popOverlayLayer,
  pushOverlayLayer,
  topOverlayLayer
} = overlayStack;

const layer = (
  id: string,
  rootId: "filter" | "library",
  kind: "root" | "nested" | "modal",
  parentId?: string
) => ({
  id,
  rootId,
  parentId,
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

const stackApi = overlayStack as typeof overlayStack & {
  closeTransientOverlayLayers: (
    state: ReturnType<typeof createOverlayStackState>
  ) => ReturnType<typeof createOverlayStackState>;
  unregisterOverlayLayer: (
    state: ReturnType<typeof createOverlayStackState>,
    registration: ReturnType<typeof layer>
  ) => ReturnType<typeof createOverlayStackState>;
};

const assertRootLayerInvariant = (state: ReturnType<typeof createOverlayStackState>) => {
  const rootIndexes = state.layers
    .map((item, index) => item.kind === "root" ? index : -1)
    .filter((index) => index >= 0);
  assert.ok(rootIndexes.length <= 1);
  if (rootIndexes.length === 1) assert.equal(rootIndexes[0], 0);
};

describe("overlay stack", () => {
  it("accepts a select only above its current modal parent and preserves the modal on close", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("dialog", "library", "modal"));
    state = pushOverlayLayer(state, layer("select", "library", "nested", "dialog"));
    assert.deepEqual(state.layers.map((item) => item.id), ["dialog", "select"]);
    assert.deepEqual(overlayStack.overlayLayerZIndexes(state.layers), [140, 150]);
    assert.equal(pushOverlayLayer(state, layer("sibling", "library", "nested", "dialog")), state);
    const dismissed = popOverlayLayer(state, "select");
    assert.equal(topOverlayLayer(dismissed)?.id, "dialog");
    assert.deepEqual(popOverlayLayer(state, "dialog").layers, []);
  });

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
    state = pushOverlayLayer(state, layer("icon-picker", "library", "nested", "library-root"));
    state = pushOverlayLayer(state, layer("remove-confirm", "library", "modal"));
    assert.equal(topOverlayLayer(state)?.id, "remove-confirm");
    state = popOverlayLayer(state, "remove-confirm");
    assert.equal(topOverlayLayer(state)?.id, "icon-picker");
  });

  it("preserves a layer position when the same id re-registers", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    state = pushOverlayLayer(state, layer("icon-picker", "library", "nested", "library-root"));
    state = pushOverlayLayer(state, layer("remove-confirm", "library", "modal"));

    const replacement = layer("icon-picker", "library", "nested", "library-root");
    state = pushOverlayLayer(state, replacement);

    assert.deepEqual(state.layers.map((item) => item.id), [
      "library-root",
      "icon-picker",
      "remove-confirm"
    ]);
    assert.equal(state.layers[1], replacement);
    assert.equal(topOverlayLayer(state)?.id, "remove-confirm");
  });

  it("ignores stale cleanup after a same-id layer re-registers", () => {
    assert.equal(typeof stackApi.unregisterOverlayLayer, "function");
    let state = openOverlayRoot(createOverlayStackState(), "library");
    const original = layer("library-root", "library", "root");
    const replacement = { ...original, onDismiss: () => undefined };
    state = pushOverlayLayer(state, original);
    state = pushOverlayLayer(state, replacement);
    const staleCleanup = stackApi.unregisterOverlayLayer(state, original);
    assert.equal(staleCleanup, state);
    assert.equal(staleCleanup.layers[0], replacement);
    assert.deepEqual(stackApi.unregisterOverlayLayer(state, replacement).layers, []);
  });

  it("rejects nested layers without a valid active-root parent", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    const original = state;

    state = pushOverlayLayer(state, layer("orphan", "library", "nested", "missing"));
    assert.equal(state, original);

    state = pushOverlayLayer(state, layer("wrong-root", "filter", "nested", "library-root"));
    assert.equal(state, original);
  });

  it("rejects descendants before their root registration", () => {
    const state = openOverlayRoot(createOverlayStackState(), "library");
    assert.equal(
      pushOverlayLayer(state, layer("nested", "library", "nested", "library-root")),
      state
    );
    assert.equal(
      pushOverlayLayer(state, layer("modal", "library", "modal", "library-root")),
      state
    );
    assertRootLayerInvariant(state);
  });

  it("never lets a late root become topmost above an existing layer", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, {
      ...layer("standalone-modal", "library", "modal"),
      dismissOnWindowBlur: false
    });
    assertRootLayerInvariant(state);
    const modalOnly = state;

    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    assert.equal(state, modalOnly);
    assert.equal(topOverlayLayer(state)?.id, "standalone-modal");
    assertRootLayerInvariant(state);
  });

  it("rejects modal layers owned by a different active root", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    const original = state;

    state = pushOverlayLayer(state, layer("filter-modal", "filter", "modal"));
    assert.equal(state, original);
  });

  it("closes transient layers on renderer departure without dismissing a modal owner", () => {
    assert.equal(typeof stackApi.closeTransientOverlayLayers, "function");
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    state = pushOverlayLayer(state, layer("nested", "library", "nested", "library-root"));
    state = pushOverlayLayer(state, {
      ...layer("modal", "library", "modal", "nested"),
      dismissOnWindowBlur: false
    });
    state = stackApi.closeTransientOverlayLayers(state);
    assert.equal(state.activeRootId, "library");
    assert.deepEqual(state.layers.map((item) => item.id), ["modal"]);

    const rootOnly = pushOverlayLayer(
      openOverlayRoot(createOverlayStackState(), "filter"),
      layer("filter-root", "filter", "root")
    );
    assert.deepEqual(stackApi.closeTransientOverlayLayers(rootOnly), createOverlayStackState());
  });

  it("closes only the requested root family", () => {
    let state = openOverlayRoot(createOverlayStackState(), "filter");
    state = pushOverlayLayer(state, layer("filter-root", "filter", "root"));
    state = closeOverlayRoot(state, "filter");
    assert.equal(state.activeRootId, null);
    assert.deepEqual(state.layers, []);
  });
});

import assert from "node:assert/strict";
import { describe, it } from "node:test";

interface OverlayMenuNavigationApi {
  findOverlayMenuTypeaheadMatch(
    labels: string[],
    query: string,
    activeIndex: number
  ): number | null;
  overlayMenuTabIndexes(itemCount: number, activeIndex: number): number[];
  resolveOverlayMenuKey(input: {
    key: string;
    activeIndex: number;
    itemCount: number;
    activeHasSubmenu: boolean;
    nested: boolean;
  }): { type: string; index?: number };
}

const loadNavigation = async (): Promise<OverlayMenuNavigationApi> => {
  try {
    return await import("../src/design/overlays/overlayMenuNavigation.ts") as OverlayMenuNavigationApi;
  } catch (error) {
    assert.fail(`overlay menu navigation seam is unavailable: ${String(error)}`);
  }
};

describe("overlay menu navigation", () => {
  it("keeps exactly one enabled item in the tab order", async () => {
    const navigation = await loadNavigation();
    assert.deepEqual(navigation.overlayMenuTabIndexes(4, 2), [-1, -1, 0, -1]);
    assert.deepEqual(navigation.overlayMenuTabIndexes(3, -1), [0, -1, -1]);
    assert.deepEqual(navigation.overlayMenuTabIndexes(0, -1), []);
  });

  it("retains wrapping arrows, Home, End, Enter, and Space", async () => {
    const navigation = await loadNavigation();
    const input = { activeIndex: 0, itemCount: 3, activeHasSubmenu: false, nested: false };
    assert.deepEqual(navigation.resolveOverlayMenuKey({ ...input, key: "ArrowDown" }), {
      type: "focus",
      index: 1
    });
    assert.deepEqual(navigation.resolveOverlayMenuKey({ ...input, key: "ArrowUp" }), {
      type: "focus",
      index: 2
    });
    assert.deepEqual(navigation.resolveOverlayMenuKey({ ...input, key: "Home" }), {
      type: "focus",
      index: 0
    });
    assert.deepEqual(navigation.resolveOverlayMenuKey({ ...input, key: "End" }), {
      type: "focus",
      index: 2
    });
    assert.deepEqual(navigation.resolveOverlayMenuKey({ ...input, key: "Enter" }), {
      type: "activate"
    });
    assert.deepEqual(navigation.resolveOverlayMenuKey({ ...input, key: " " }), {
      type: "activate"
    });
  });

  it("opens a submenu with ArrowRight and returns with ArrowLeft", async () => {
    const navigation = await loadNavigation();
    assert.deepEqual(navigation.resolveOverlayMenuKey({
      key: "ArrowRight",
      activeIndex: 1,
      itemCount: 3,
      activeHasSubmenu: true,
      nested: false
    }), { type: "open-submenu" });
    assert.deepEqual(navigation.resolveOverlayMenuKey({
      key: "ArrowLeft",
      activeIndex: 0,
      itemCount: 3,
      activeHasSubmenu: false,
      nested: true
    }), { type: "close-submenu" });
  });

  it("finds typeahead matches after the active item and cycles repeated letters", async () => {
    const navigation = await loadNavigation();
    const labels = ["Apple", "Apricot", "Banana", "Blueberry"];
    assert.equal(navigation.findOverlayMenuTypeaheadMatch(labels, "b", 0), 2);
    assert.equal(navigation.findOverlayMenuTypeaheadMatch(labels, "a", 0), 1);
    assert.equal(navigation.findOverlayMenuTypeaheadMatch(labels, "aa", 0), 1);
    assert.equal(navigation.findOverlayMenuTypeaheadMatch(labels, "z", 0), null);
  });
});

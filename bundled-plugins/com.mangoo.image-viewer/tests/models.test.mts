import assert from "node:assert/strict";
import test from "node:test";

import { EDGE_GAP, defaultPlacement, relayout, snapRect } from "../src/shared/floatingPanel.ts";
import { buildTimeline, frameAt, normalizeFrameDuration, stepFrame } from "../src/shared/frames.ts";
import {
  assignChord,
  chordFromEvent,
  formatChord,
  matchAction,
  normalizeChord,
  normalizeOverrides,
  removeChord,
  resetAction,
  resolveBindings
} from "../src/shared/shortcuts.ts";

const key = (key: string, extra: Partial<{ code: string; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; metaKey: boolean }> = {}) => ({
  key, code: extra.code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...extra
});

test("chords are canonical regardless of layout case and modifiers", () => {
  assert.equal(chordFromEvent(key("C", { code: "KeyC", ctrlKey: true, shiftKey: true })), "Ctrl+Shift+C");
  assert.equal(chordFromEvent(key("?", { code: "Slash", shiftKey: true })), "Shift+/");
  assert.equal(chordFromEvent(key(" ", { code: "Space" })), "Space");
  assert.equal(chordFromEvent(key("+", { code: "NumpadAdd" })), "=");
  assert.equal(chordFromEvent(key("Shift", { code: "ShiftLeft", shiftKey: true })), null, "modifier alone is not a chord");
  assert.equal(normalizeChord("ctrl+shift+c"), "Ctrl+Shift+C");
  assert.equal(normalizeChord("Ctrl++"), "Ctrl+=");
  assert.equal(normalizeChord("Hyper+X"), null);
  assert.equal(formatChord("Ctrl+="), "Ctrl + +");
  assert.equal(formatChord("ArrowRight"), "→");
});

test("defaults match, remaps move chords between actions, resets restore", () => {
  const defaults = resolveBindings({});
  assert.equal(matchAction(defaults, key("ArrowRight", { code: "ArrowRight" })), "next");
  assert.equal(matchAction(defaults, key("r", { code: "KeyR" })), "rotateRight");
  assert.equal(matchAction(defaults, key("R", { code: "KeyR", shiftKey: true })), "rotateLeft");
  let overrides = assignChord({}, "next", "H", null);
  const bindings = resolveBindings(overrides);
  assert.deepEqual(bindings.next, ["ArrowRight", "PageDown", "H"]);
  assert.deepEqual(bindings.flipHorizontal, [], "H was moved off flipHorizontal");
  assert.equal(matchAction(bindings, key("h", { code: "KeyH" })), "next");
  overrides = removeChord(overrides, "next", "PageDown");
  assert.deepEqual(resolveBindings(overrides).next, ["ArrowRight", "H"]);
  overrides = resetAction(resetAction(overrides, "next"), "flipHorizontal");
  assert.deepEqual(resolveBindings(overrides).flipHorizontal, ["H"]);
  // Stored garbage and reserved keys are dropped.
  assert.deepEqual(normalizeOverrides({ next: ["Escape", "bogus+key", "N"], unknown: ["X"] }), { next: ["N"] });
});

test("frame timeline maps playback time to frames and normalises tiny delays", () => {
  assert.equal(normalizeFrameDuration(0), 100);
  assert.equal(normalizeFrameDuration(10), 100);
  assert.equal(normalizeFrameDuration(40), 40);
  const timeline = buildTimeline([400, 100, 100]);
  assert.equal(timeline.total, 600);
  assert.equal(frameAt(timeline, 0), 0);
  assert.equal(frameAt(timeline, 399), 0);
  assert.equal(frameAt(timeline, 400), 1);
  assert.equal(frameAt(timeline, 550), 2);
  assert.equal(frameAt(timeline, 650), 0, "loops");
  assert.equal(stepFrame(3, 0, -1), 2);
  assert.equal(stepFrame(3, 2, 1), 0);
});

test("floating panels snap to edges and stay docked through resizes", () => {
  const bounds = { width: 1000, height: 700 };
  const insets = { top: 40, bottom: 92 };
  const initial = defaultPlacement({ width: 300, height: 400 }, bounds, insets);
  assert.deepEqual(initial.dock, { horizontal: "right", vertical: "top" });
  assert.equal(initial.rect.x, 1000 - 300 - EDGE_GAP);
  assert.equal(initial.rect.y, 40 + EDGE_GAP);
  const nearLeft = snapRect({ x: 20, y: 200, width: 300, height: 400 }, bounds, insets);
  assert.equal(nearLeft.dock.horizontal, "left");
  assert.equal(nearLeft.rect.x, EDGE_GAP);
  const floating = snapRect({ x: 300, y: 120, width: 300, height: 300 }, bounds, insets);
  assert.deepEqual(floating.dock, { horizontal: null, vertical: null });
  // Window shrinks: right-docked panel follows the right edge, never escapes.
  const moved = relayout(initial, { width: 700, height: 500 }, insets);
  assert.equal(moved.x, 700 - 300 - EDGE_GAP);
  assert.ok(moved.y + moved.height <= 500 - insets.bottom);
});

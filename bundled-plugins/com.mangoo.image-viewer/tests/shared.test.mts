import assert from "node:assert/strict";
import test from "node:test";

import { extensionOf, isAnimatedImageHeader, splitPath } from "../src/shared/formats.ts";
import {
  compareNames,
  formatBytes,
  indexAfterRemoval,
  indexOfPath,
  prefetchOrder,
  sortEntries,
  stepIndex,
  validateFileName,
  type ImageEntry
} from "../src/shared/navigation.ts";
import {
  clampPan,
  softClampPan,
  cssTransform,
  fitScale,
  identityTransform,
  nextZoomStop,
  renderLevel,
  rotate,
  zoomAt
} from "../src/shared/viewTransform.ts";

const entry = (name: string, modifiedAtMs = 0, sizeBytes = 0): ImageEntry => ({
  path: `C:\\Photos\\${name}`,
  name,
  ext: extensionOf(name),
  sizeBytes,
  modifiedAtMs
});

test("natural, case-insensitive ordering like a file manager", () => {
  const names = ["img10.jpg", "IMG2.jpg", "img1.jpg", "a.png"];
  assert.deepEqual(sortEntries(names.map((name) => entry(name)), "name", "asc").map((item) => item.name), [
    "a.png", "img1.jpg", "IMG2.jpg", "img10.jpg"
  ]);
  assert.ok(compareNames("photo 9", "photo 10") < 0);
  const byDate = sortEntries([entry("b.jpg", 2), entry("a.jpg", 2), entry("c.jpg", 1)], "modified", "desc");
  assert.deepEqual(byDate.map((item) => item.name), ["a.jpg", "b.jpg", "c.jpg"], "ties fall back to name");
  const bySize = sortEntries([entry("a.jpg", 0, 30), entry("b.jpg", 0, 10)], "size", "asc");
  assert.deepEqual(bySize.map((item) => item.name), ["b.jpg", "a.jpg"]);
  assert.equal(indexOfPath(byDate, "c:/photos/C.JPG".replace("C.JPG", "c.jpg")), 2, "Windows paths compare case-insensitively");
});

test("stepping wraps or clamps, removal keeps the position, prefetch is nearest-first", () => {
  assert.equal(stepIndex(5, 4, 1, true), 0);
  assert.equal(stepIndex(5, 0, -1, true), 4);
  assert.equal(stepIndex(5, 4, 1, false), 4);
  assert.equal(stepIndex(0, 0, 1, true), -1);
  assert.equal(indexAfterRemoval(3, 3), 2);
  assert.equal(indexAfterRemoval(0, 0), -1);
  assert.deepEqual(prefetchOrder(10, 0, 2, true), [1, 9, 2, 8]);
  assert.deepEqual(prefetchOrder(10, 0, 2, false), [1, 2]);
  assert.deepEqual(prefetchOrder(2, 0, 2, true), [1]);
});

test("file names reject separators, reserved names and trailing dots", () => {
  assert.equal(validateFileName("holiday.jpg"), null);
  assert.ok(validateFileName(""));
  assert.ok(validateFileName("a/b.jpg"));
  assert.ok(validateFileName("con.jpg"));
  assert.ok(validateFileName("name."));
  assert.equal(formatBytes(1536), "1.50 KB");
  assert.deepEqual(splitPath("C:\\a\\b.jpg"), { directory: "C:\\a", name: "b.jpg" });
});

test("fit modes, zoom at cursor and pan clamping", () => {
  const image = { width: 4000, height: 2000 };
  const viewport = { width: 1000, height: 1000 };
  assert.equal(fitScale(image, viewport, 0, "fit"), 0.25);
  assert.equal(fitScale(image, viewport, 90, "fit"), 0.25, "rotated box is 2000x4000");
  assert.equal(fitScale(image, viewport, 0, "fill"), 0.5);
  assert.equal(fitScale({ width: 100, height: 100 }, viewport, 0, "fit"), 1, "small pictures are not enlarged");
  assert.equal(fitScale({ width: 100, height: 100 }, viewport, 0, "fit", true), 10);
  // Zooming at a corner keeps the image point under the cursor fixed.
  const start = { ...identityTransform(), scale: 0.25 };
  const zoomed = zoomAt(start, 0.5, { x: 1000, y: 500 }, image, viewport);
  // Point under the cursor before: image x = (1000-500-0)/0.25 = 2000 from centre.
  const before = (1000 - 500 - start.panX) / start.scale;
  const after = (1000 - 500 - zoomed.panX) / zoomed.scale;
  assert.ok(Math.abs(before - after) < 1e-6);
  const clamped = clampPan({ ...identityTransform(), scale: 1, panX: 99999, panY: 99999 }, image, viewport);
  assert.equal(clamped.panX, 1500);
  assert.equal(clamped.panY, 500);
  const small = clampPan({ ...identityTransform(), scale: 0.1, panX: 50, panY: 50 }, image, viewport);
  assert.equal(small.panX, 0, "a picture smaller than the viewport stays centred");
  assert.equal(rotate(identityTransform(), -1).rotation, 270);
  assert.equal(rotate(rotate(identityTransform(), 1), 1).rotation, 180);
  assert.equal(nextZoomStop(1, 1), 1.25);
  assert.equal(nextZoomStop(1, -1), 0.75);
  assert.match(cssTransform({ ...identityTransform(), flipX: true }), /scale\(-1, 1\)$/);
});

test("canvas render level picks the power-of-two backing store the screen needs", () => {
  assert.equal(renderLevel(1), 1);
  assert.equal(renderLevel(0.8), 1);
  assert.equal(renderLevel(0.5), 1, "0.5 * 1.15 rounds up to full resolution");
  assert.equal(renderLevel(0.3), 0.5);
  assert.equal(renderLevel(0.08), 0.125);
  assert.equal(renderLevel(0), 1 / 32, "floored at 1/64 of need, rounded up");
  assert.equal(renderLevel(Number.NaN), 1);
});

test("animated headers are detected for GIF, animated WebP and AVIF sequences", () => {
  const bytes = (text: string) => Uint8Array.from(text, (char) => char.charCodeAt(0));
  assert.equal(isAnimatedImageHeader(bytes("GIF89a"), ".gif"), true);
  const webp = new Uint8Array(32);
  webp.set(bytes("RIFF"), 0);
  webp.set(bytes("WEBP"), 8);
  webp.set(bytes("VP8X"), 12);
  assert.equal(isAnimatedImageHeader(webp, ".webp"), false);
  webp[20] = 0x02;
  assert.equal(isAnimatedImageHeader(webp, ".webp"), true);
  const avif = new Uint8Array(24);
  avif.set([0, 0, 0, 24], 0);
  avif.set(bytes("ftypavis"), 4);
  assert.equal(isAnimatedImageHeader(avif, ".avif"), true);
  avif.set(bytes("avif"), 8);
  assert.equal(isAnimatedImageHeader(avif, ".avif"), false);
  assert.equal(isAnimatedImageHeader(bytes("GIF89a"), ".png"), false);
});

test("panning is free by hand; keyboard and wheel panning stop at the edge", () => {
  const image = { width: 4000, height: 2000 };
  const viewport = { width: 1000, height: 1000 };
  const start = { ...identityTransform(), scale: 1, panX: 1400, panY: 0 };
  // Zoom at a point does not clamp (a dragged-out picture stays where it is).
  const zoomed = zoomAt({ ...identityTransform(), scale: 1, panX: 5000, panY: 0 }, 1, { x: 500, y: 500 }, image, viewport);
  assert.equal(zoomed.panX, 5000);
  // Soft limit: stop at the edge (1500) ...
  assert.equal(softClampPan(start, { ...start, panX: 1700 }, image, viewport).panX, 1500);
  // ... but never pull back a picture already beyond it, and allow moving inwards.
  const outside = { ...start, panX: 3000 };
  assert.equal(softClampPan(outside, { ...outside, panX: 3100 }, image, viewport).panX, 3000);
  assert.equal(softClampPan(outside, { ...outside, panX: 2900 }, image, viewport).panX, 2900);
});

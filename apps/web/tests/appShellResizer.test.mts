import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const appShellSource = readFileSync(
  new URL("../src/app-shell/AppShell.tsx", import.meta.url),
  "utf8"
);
const stylesSource = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

test("workbench divider hover uses the shortened reveal delay", () => {
  assert.match(
    stylesSource,
    /\.shell-column-resizer:hover::after\s*\{\s*transition-delay:\s*250ms;\s*\}/
  );
});

test("pointer resize completion clears pointer-acquired focus and active state", () => {
  const pointerEndHandler = appShellSource.match(
    /const handleColumnResizePointerEnd = useCallback\([\s\S]*?\n  \}, \[\]\);/
  )?.[0];

  assert.ok(pointerEndHandler, "column resize pointer-end handler must remain present");
  assert.match(
    pointerEndHandler,
    /if \(event\.currentTarget\.hasPointerCapture\(event\.pointerId\)\) \{\s*event\.currentTarget\.releasePointerCapture\(event\.pointerId\);\s*\}/
  );
  assert.match(pointerEndHandler, /event\.currentTarget\.blur\(\);/);
  assert.match(pointerEndHandler, /document\.body\.dataset\.shellResizing = "false";/);
  assert.match(pointerEndHandler, /setActiveResizeSide\(null\);/);

  assert.equal(
    appShellSource.match(/onPointerCancel=\{handleColumnResizePointerEnd\}/g)?.length,
    2,
    "both dividers must clean up cancelled pointer gestures"
  );
  assert.equal(
    appShellSource.match(/onPointerUp=\{handleColumnResizePointerEnd\}/g)?.length,
    2,
    "both dividers must clean up completed pointer gestures"
  );
});

test("divider cleanup preserves keyboard resizing and seam geometry", () => {
  assert.equal(appShellSource.match(/role="separator"/g)?.length, 2);
  assert.equal(appShellSource.match(/tabIndex=\{0\}/g)?.length, 2);
  assert.equal(appShellSource.match(/onKeyDown=\{\(event\) => handleColumnResizeKeyDown/g)?.length, 2);
  assert.match(appShellSource, /event\.key !== "ArrowLeft" && event\.key !== "ArrowRight"/);

  assert.match(stylesSource, /--shell-column-resizer-width:\s*10px;/);
  assert.match(
    stylesSource,
    /\.shell-column-resizer::after\s*\{[\s\S]*?width:\s*1px;[\s\S]*?\}/
  );

  const workbenchColumnAfter = stylesSource.match(/\.workbench-column::after\s*\{([\s\S]*?)\}/)?.[1];
  assert.ok(workbenchColumnAfter, "workbench column seam override must remain present");
  assert.doesNotMatch(workbenchColumnAfter, /inset\s+0\s+0\s+\d+px/);
});

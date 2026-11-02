# Preview Navigation Gate & Render Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop preview navigation from outrunning the display, and add a titlebar popover that controls the image scaling filter and per-channel display.

**Architecture:** Two independent changes to the preview surface. (A) `startPreviewNavigation` gains a **first-paint** gate — a step is refused while the selected media has not yet painted, and extra input is dropped rather than queued. (B) A `PreviewRenderMenu` popover, structurally a copy of `SortMenu`, writes two values into the existing `PreviewPreferences` (localStorage + cross-window sync); those values are applied purely in CSS — `image-rendering` for the filter, and an SVG `feColorMatrix` referenced by `filter: url(#…)` for channel isolation — on the shared `.preview-image` class, so the canvas, `<img>`, and `<video>` lanes are all covered with one rule set and nothing repaints or re-decodes.

**Tech Stack:** React 18 + TypeScript + Vite; `node --test` for web units; existing `design/overlays` primitives; CSS + inline SVG filters.

## Global Constraints

- **Copy the spec's exact option sets.** Filter: `最近邻 Nearest neighbour` / `双线性 Bilinear`, default Bilinear. Channel: `RGBA` / `RGB` / `R` / `G` / `B` / `A`, default RGBA. No premultiply control exists.
- **RGBA is the default and must apply NO CSS filter at all**, so users who never open the panel pay nothing.
- **Channel views show the raw stored channel value, never multiplied by alpha.** This is the entire point — in a TGA the A channel is often data, not transparency.
- **Filter options are named after the real resampling algorithm.** Never label a row with an algorithm the browser is not actually running.
- **The button is always visible while the preview is open, including for video.**
- **Opening and closing the preview are NOT gated** — only stepping between images.
- **A media that fails to decode must not wedge navigation.** `PreviewCanvas`'s error path already calls `onMediaReady`; this must stay true and be tested.
- **Extra input during the gate is DROPPED, not queued.**
- **Web tests are plain `node --test` with NO DOM** (`node --experimental-strip-types --test tests/*.test.mts`). Testable logic lives in pure modules; follow `apps/web/tests/previewRenderLane.test.mts` for the `register("./resolve-source-imports.mjs", import.meta.url)` pattern.
- **No raw `fetch(` anywhere under `apps/web/src`** — `tools/checks/validate-web-client.mjs` fails the build on it.
- **Every scripted file write must pass `newline="\n"`.** Python's default text-mode write converts the file to CRLF on Windows, which breaks `check:ui-design` (its regex matches `\n  \}\n\n  function`). This cost a debugging cycle last time.
- **`git add` explicit paths only — never `-A`, never `.`** The user has unrelated uncommitted work: `docs/benchmarks/2026-07-20-notification-center-verification.md`, `docs/superpowers/plans/2026-07-24-notification-failure-drilldown.md`, and six `tools/dev/*.mjs` probe scripts.
- **Full gate before done:** `npm test` (structure, core-api, core-client, notification-contract, desktop, web, ui-design, schema, rust). Rust needs `export PATH="$HOME/.cargo/bin:$PATH"`.

---

## File Structure

**Create:**
- `apps/web/src/features/preview/previewRenderSettings.ts` — pure mapping from the two preference values to a CSS `image-rendering` keyword and an SVG filter id. No DOM, no React.
- `apps/web/src/features/preview/PreviewRenderMenu.tsx` — the trigger button + anchored popover. Presentation only; owns no state.
- `apps/web/tests/previewRenderSettings.test.mts` — unit tests for the pure mapping.

**Modify:**
- `apps/web/src/app/previewNavigationGate.ts` — add `canStartPreviewNavigation` beside the existing `canNavigatePreview`.
- `apps/web/src/app/App.tsx` — painted-id ref, gate call, menu open state, new preference wiring, stage data attributes.
- `apps/web/src/features/preview/previewPreferences.ts` — two new fields, defaults, and normalisation.
- `apps/web/src/app-shell/ShellTopBar.tsx` — a right-hand `titlebar-preview-actions` slot in `PreviewTitlebarToolbar`.
- `apps/web/src/design/overlays/overlayTypes.ts` — add `"preview-render"` to `OverlayRootId`.
- `apps/web/src/app-shell/AppShell.tsx` — the hidden `<svg>` holding the channel matrices.
- `apps/web/src/styles.css` — `.preview-image` rules driven by stage data attributes.
- `tools/checks/validate-web-client.mjs` — guards for the gate and the new preferences.
- `apps/web/tests/previewNavigationGate.test.mts` — extend.

---

### Task 1: Verify the two browser assumptions this design rests on

**Files:**
- Create: `docs/benchmarks/2026-08-02-preview-render-browser-checks.md`
- Create (temporary, deleted in Step 5): `tools/dev/_render_probe.mjs`

**Interfaces:**
- Consumes: nothing.
- Produces: two recorded findings that later tasks depend on — `HIGH_QUALITY_DISTINCT` (boolean, decides whether Task 4's filter list has two or three entries) and `FECOLORMATRIX_UNPREMULTIPLIES` (boolean, decides whether Task 4's channel matrices are viable at all).

**Do this task FIRST and do not skip it.** Both assumptions are load-bearing and neither is verified. If the second one is false, the channel feature needs a WebGL paint path and the plan must be re-scoped — that is a conversation to have before building, not after.

- [ ] **Step 1: Launch the app**

```bash
cd /d/Mangoo
export MANGOO_AUTO_ADD_ROOT="C:/Users/84460/Pictures/normal"
export MANGOO_REMOTE_DEBUG=1
nohup npm run dev > /tmp/dev_probe.log 2>&1 &
sleep 85
grep -E "Core ready|renderer ready" /tmp/dev_probe.log | head -3
```

- [ ] **Step 2: Write the probe**

Create `tools/dev/_render_probe.mjs`:

```js
import { WebSocket } from "ws";
const ws = new WebSocket(process.argv[2]);
let id = 0;
const pend = new Map();
const send = (m, p = {}) =>
  new Promise((r, j) => {
    const i = ++id;
    pend.set(i, { r, j });
    ws.send(JSON.stringify({ id: i, method: m, params: p }));
  });
ws.on("message", (d) => {
  const m = JSON.parse(d.toString());
  if (m.id && pend.has(m.id)) {
    const p = pend.get(m.id);
    pend.delete(m.id);
    m.error ? p.j(new Error(JSON.stringify(m.error))) : p.r(m.result);
  }
});
ws.on("open", async () => {
  await send("Runtime.enable");
  const r = await send("Runtime.evaluate", {
    awaitPromise: true,
    returnByValue: true,
    expression: `(async () => {
      // --- Check 1: is image-rendering:high-quality distinct from auto? ---
      const supports = CSS.supports("image-rendering", "high-quality");

      // --- Check 2: does feColorMatrix operate on NON-premultiplied values? ---
      // Build a 1x1 pixel with RGB=(255,0,0) at 50% alpha, then apply a matrix
      // that passes RGB through and forces A=1. If feColorMatrix un-premultiplies,
      // the result is R=255. If it operates on premultiplied data, R is ~128.
      const svgns = "http://www.w3.org/2000/svg";
      const svg = document.createElementNS(svgns, "svg");
      svg.setAttribute("width", "1"); svg.setAttribute("height", "1");
      svg.style.position = "fixed"; svg.style.opacity = "0"; svg.style.pointerEvents = "none";
      const defs = document.createElementNS(svgns, "defs");
      const filter = document.createElementNS(svgns, "filter");
      filter.setAttribute("id", "mangoo-probe-rgb");
      filter.setAttribute("color-interpolation-filters", "sRGB");
      const fe = document.createElementNS(svgns, "feColorMatrix");
      fe.setAttribute("type", "matrix");
      fe.setAttribute("values", "1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 0 1");
      filter.appendChild(fe); defs.appendChild(filter); svg.appendChild(defs);
      document.body.appendChild(svg);

      const src = document.createElement("canvas");
      src.width = 1; src.height = 1;
      const sctx = src.getContext("2d");
      sctx.fillStyle = "rgba(255,0,0,0.5)";
      sctx.fillRect(0, 0, 1, 1);

      const out = document.createElement("canvas");
      out.width = 1; out.height = 1;
      const octx = out.getContext("2d");
      octx.filter = "url(#mangoo-probe-rgb)";
      octx.drawImage(src, 0, 0);
      const px = octx.getImageData(0, 0, 1, 1).data;

      svg.remove();
      return JSON.stringify({
        HIGH_QUALITY_SUPPORTED: supports,
        probeRGBA: [px[0], px[1], px[2], px[3]],
        FECOLORMATRIX_UNPREMULTIPLIES: px[0] > 200
      }, null, 1);
    })()`
  });
  console.log(r.result?.value ?? JSON.stringify(r.exceptionDetails).slice(0, 400));
  ws.close();
  process.exit(0);
});
```

- [ ] **Step 3: Run it**

```bash
cd /d/Mangoo
WS=$(curl -sS http://127.0.0.1:9500/json | python -c "
import json,sys
p=[x for x in json.load(sys.stdin) if x.get('type')=='page' and 'omnibox' not in x.get('url','')]
print(p[0]['webSocketDebuggerUrl'] if p else '')")
node tools/dev/_render_probe.mjs "$WS"
```

Expected shape:
```
{ "HIGH_QUALITY_SUPPORTED": true|false,
  "probeRGBA": [r, g, b, a],
  "FECOLORMATRIX_UNPREMULTIPLIES": true|false }
```

`probeRGBA[0]` near **255** means feColorMatrix un-premultiplies (the design holds).
Near **128** means it does not.

- [ ] **Step 4: Act on the result**

- `FECOLORMATRIX_UNPREMULTIPLIES === false` → **STOP and report to the human.** The channel feature as designed cannot show raw channel values; it needs a WebGL paint path, which is a materially larger change than this plan assumes. Do not silently build a version that darkens semi-transparent pixels.
- `HIGH_QUALITY_SUPPORTED === false` → Task 4 ships **two** filter options. This is the expected outcome.
- `HIGH_QUALITY_SUPPORTED === true` → it is *supported*, which is not the same as *different*. Confirm visually before adding a third row: set `image-rendering: high-quality` vs `auto` on the same upscaled image and compare screenshots. Add the row only if they differ, and name it for the algorithm it actually runs.

- [ ] **Step 5: Record the findings and clean up**

Write `docs/benchmarks/2026-08-02-preview-render-browser-checks.md` containing the raw probe output, the date, the Electron/Chromium version (`process.versions.chrome` via the same CDP eval), and the resulting decision for each check in one sentence.

```bash
cd /d/Mangoo
taskkill //F //IM mangoo-core.exe 2>/dev/null | head -1
taskkill //F //IM electron.exe 2>/dev/null | head -1
rm -f tools/dev/_render_probe.mjs
git add docs/benchmarks/2026-08-02-preview-render-browser-checks.md
git commit -m "docs: record preview render browser capability checks"
```

---

### Task 2: The first-paint navigation gate (pure)

**Files:**
- Modify: `apps/web/src/app/previewNavigationGate.ts`
- Test: `apps/web/tests/previewNavigationGate.test.mts`

**Interfaces:**
- Consumes: the existing `canNavigatePreview` in the same file.
- Produces: `canStartPreviewNavigation(input: { previewOpen: boolean; selectedMediaIndex: number; itemCount: number; direction: "previous" | "next"; selectedMediaId: number | null; paintedMediaId: number | null }): boolean`. Task 3 calls it.

- [ ] **Step 1: Write the failing tests**

Append to `apps/web/tests/previewNavigationGate.test.mts`:

```typescript
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
    // This is the whole feature: selection moved to 101, but the canvas is
    // still showing 100, so a further step would skip 101 entirely.
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

  it("allows the first step when nothing has painted and nothing is selected", () => {
    // Both null means "no frame expected yet" — not a pending frame. Treating
    // this as blocked would wedge navigation before the first paint ever lands.
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
```

Extend the import at the top of the file:

```typescript
const { canNavigatePreview, canStartPreviewNavigation } = await import(
  "../src/app/previewNavigationGate.ts"
);
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run test 2>&1 | grep -E "^# (pass|fail)|not ok" | head -5
```

Expected: FAIL — `canStartPreviewNavigation is not a function`.

- [ ] **Step 3: Write the implementation**

Append to `apps/web/src/app/previewNavigationGate.ts`:

```typescript
/**
 * Whether a prev/next step may START right now.
 *
 * Extends the position-only `canNavigatePreview` with one condition: the
 * currently-selected media must have PAINTED at least once. Not committed, not
 * fully loaded — the moment pixels appear.
 *
 * This is deliberately far weaker than the readiness lock it replaces.
 * `PreviewCanvas` reports a paint on every draw, including the synchronous one
 * that happens in the same tick when a neighbour is already pooled, so whenever
 * prefetch keeps up this gate never engages. It bites only on a cold pool —
 * exactly the case where scrolling used to skip images that never appeared. A
 * single wheel flick emits many `wheel` events, each calling next/previous, so
 * without this one gesture could advance several images.
 *
 * Input arriving while the gate is closed is DROPPED by the caller, not queued:
 * holding a key advances exactly as fast as images can begin painting.
 */
export function canStartPreviewNavigation(input: {
  previewOpen: boolean;
  selectedMediaIndex: number;
  itemCount: number;
  direction: "previous" | "next";
  selectedMediaId: number | null;
  paintedMediaId: number | null;
}): boolean {
  if (
    !canNavigatePreview({
      previewOpen: input.previewOpen,
      selectedMediaIndex: input.selectedMediaIndex,
      itemCount: input.itemCount,
      direction: input.direction
    })
  ) {
    return false;
  }
  // Nothing selected and nothing painted is the "no frame expected" state, not
  // a pending one — blocking it would wedge navigation before the first paint.
  if (input.selectedMediaId === null && input.paintedMediaId === null) {
    return true;
  }
  return input.selectedMediaId === input.paintedMediaId;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run test 2>&1 | grep -E "^# (tests|pass|fail)" | head -4
npm --workspace @mangoo/web run typecheck 2>&1 | tail -2
```

Expected: 7 new assertions pass, 0 failing; typecheck clean.

- [ ] **Step 5: Commit**

```bash
cd /d/Mangoo
git add apps/web/src/app/previewNavigationGate.ts apps/web/tests/previewNavigationGate.test.mts
git commit -m "feat(web): add a first-paint preview navigation gate"
```

---

### Task 3: Wire the gate into App.tsx and guard it

**Files:**
- Modify: `apps/web/src/app/App.tsx`
- Modify: `tools/checks/validate-web-client.mjs`

**Interfaces:**
- Consumes: Task 2's `canStartPreviewNavigation`.
- Produces: nothing new for later tasks; this is the behavioural wiring.

**Where the painted id comes from:** `App.tsx` already has `handlePreviewMediaSettled(mediaId)`, called from `CentralPreviewStage`'s `handleMediaReady`, which `PreviewCanvas` invokes on **every** paint — including the synchronous warm paint and the error path. That is exactly the "first paint" signal, so no new plumbing is needed. It must be a **ref**, not state: `startPreviewNavigation` needs the current value, and an extra render per paint would be waste.

- [ ] **Step 1: Add the painted-id ref**

Next to the existing `previewDisplayedMediaId` state declaration in `App.tsx`:

```tsx
  // The media whose pixels are actually on screen. Written on EVERY paint —
  // including the error path, so a media that fails to decode releases the gate
  // instead of trapping navigation forever.
  const paintedMediaIdRef = useRef<number | null>(null);
```

- [ ] **Step 2: Feed it from the existing settle callback**

```tsx
  const handlePreviewMediaSettled = useCallback(
    (mediaId: number) => {
      paintedMediaIdRef.current = mediaId;
      if (mediaId !== library.selectedMediaId) return;
      setPreviewDisplayedMediaId(mediaId);
    },
    [library.selectedMediaId]
  );
```

The ref is written **before** the early return: a paint landing after the user moved on still records that this media reached the screen.

- [ ] **Step 3: Reset it when the preview opens**

```tsx
  const handleOpenPreview = useCallback(
    (mediaId: number) => {
      paintedMediaIdRef.current = null;
      setPreviewDisplayedMediaId(null);
      library.setSelectedMediaId(mediaId);
      setPreviewViewCommands(null);
      setPreviewOpen(true);
    },
    [library]
  );
```

- [ ] **Step 4: Gate the step**

Replace `startPreviewNavigation`, and add `canStartPreviewNavigation` to the existing import from `./previewNavigationGate`:

```tsx
  function startPreviewNavigation(
    target: MediaRecord | undefined,
    direction: "previous" | "next"
  ) {
    if (!target) return;
    // Drop, do not queue. A wheel flick emits many events and a held arrow key
    // repeats far faster than a cold image can start painting; queueing would
    // bank those steps and skip images the user never saw.
    if (
      !canStartPreviewNavigation({
        previewOpen,
        selectedMediaIndex,
        itemCount: orderedPreviewMedia.length,
        direction,
        selectedMediaId: library.selectedMediaId,
        paintedMediaId: paintedMediaIdRef.current
      })
    ) {
      return;
    }
    library.setSelectedMediaId(target.id);
  }

  const handlePreviewPrevious = useCallback(() => {
    if (selectedMediaIndex <= 0) return;
    startPreviewNavigation(orderedPreviewMedia[selectedMediaIndex - 1], "previous");
  }, [library, orderedPreviewMedia, selectedMediaIndex]);

  const handlePreviewNext = useCallback(() => {
    if (selectedMediaIndex < 0 || selectedMediaIndex >= orderedPreviewMedia.length - 1) return;
    startPreviewNavigation(orderedPreviewMedia[selectedMediaIndex + 1], "next");
  }, [library, orderedPreviewMedia, selectedMediaIndex]);
```

- [ ] **Step 5: Update the structural guards**

`tools/checks/validate-web-client.mjs` already forbids the old readiness lock returning. Keep that — the lock it forbids waited for a *settled* frame — but its message is now misleading, and nothing asserts the new gate exists. Replace that block with:

```javascript
if (/previewNavigationLockedRef/.test(app) || /previewNavigationReady/.test(app)) {
  fail("App preview navigation must not reintroduce a settled-frame readiness lock; the first-paint gate replaced it");
}
if (
  !/canStartPreviewNavigation/.test(previewNavigationGate) ||
  !/canStartPreviewNavigation\(\{/.test(app) ||
  !/paintedMediaIdRef/.test(app)
) {
  fail("App preview navigation must gate next/previous on the first-paint canStartPreviewNavigation");
}
```

- [ ] **Step 6: Verify**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run typecheck 2>&1 | tail -2
npm run check:web 2>&1 | grep -E "^FAIL|^PASS: web" | head -3
```

Expected: typecheck clean, `PASS: web client boundaries`.

- [ ] **Step 7: Commit, THEN mutation-test the guard**

Commit first. `git checkout` on a file carrying uncommitted work destroys it — that mistake cost a full redo twice on the previous plan.

```bash
cd /d/Mangoo
git add apps/web/src/app/App.tsx tools/checks/validate-web-client.mjs
git commit -m "feat(web): gate preview steps on the first paint of the current image"
```

Now prove the guard can fail:

```bash
cd /d/Mangoo
python -c "
import io
p='apps/web/src/app/App.tsx'
s=io.open(p,encoding='utf-8').read()
io.open(p,'w',encoding='utf-8',newline='\n').write(s.replace('paintedMediaIdRef','pmRefBROKEN'))
"
npm run check:web 2>&1 | grep -E "^FAIL" | head -2
git checkout -- apps/web/src/app/App.tsx
npm run check:web 2>&1 | grep -E "^PASS: web" | head -1
```

Expected: the mutated run FAILS with the first-paint-gate message, the restored run passes.

---

### Task 4: `previewRenderSettings` — the pure mapping

**Files:**
- Create: `apps/web/src/features/preview/previewRenderSettings.ts`
- Test: `apps/web/tests/previewRenderSettings.test.mts`

**Interfaces:**
- Consumes: Task 1's `HIGH_QUALITY_DISTINCT` finding.
- Produces:
  - `type PreviewRenderFilter = "bilinear" | "nearest"`
  - `type PreviewChannel = "rgba" | "rgb" | "r" | "g" | "b" | "a"`
  - `DEFAULT_PREVIEW_RENDER_FILTER`, `DEFAULT_PREVIEW_CHANNEL`
  - `PREVIEW_RENDER_FILTERS`, `PREVIEW_CHANNELS` (label lists for the menu)
  - `imageRenderingForFilter(filter: PreviewRenderFilter): string`
  - `channelFilterId(channel: PreviewChannel): string | null`
  - `CHANNEL_MATRICES: ReadonlyArray<{ id: string; values: string }>`

  Tasks 5, 6 and 7 consume all of it.

**If Task 1 found `high-quality` genuinely distinct**, add `"highQuality"` to the union, a third `PREVIEW_RENDER_FILTERS` entry named for the algorithm it actually runs, and a case in `imageRenderingForFilter`. Otherwise ship two.

- [ ] **Step 1: Write the failing test**

Create `apps/web/tests/previewRenderSettings.test.mts`:

```typescript
import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  CHANNEL_MATRICES,
  PREVIEW_CHANNELS,
  PREVIEW_RENDER_FILTERS,
  channelFilterId,
  imageRenderingForFilter
} = await import("../src/features/preview/previewRenderSettings.ts");

describe("preview scaling filter", () => {
  it("maps each filter to the CSS keyword that runs that algorithm", () => {
    assert.equal(imageRenderingForFilter("nearest"), "pixelated");
    assert.equal(imageRenderingForFilter("bilinear"), "auto");
  });

  it("names every option after a real algorithm, not a quality word", () => {
    const labels = PREVIEW_RENDER_FILTERS.map((option) => option.label).join(" ");
    // A row labelled by vibe would claim behaviour we do not control.
    for (const banned of ["Smooth", "High quality", "Fast"]) {
      assert.ok(!labels.includes(banned), `label must not say "${banned}": ${labels}`);
    }
    assert.ok(/Nearest/i.test(labels), labels);
    assert.ok(/Bilinear/i.test(labels), labels);
  });
});

describe("preview channel isolation", () => {
  it("applies NO filter for the default RGBA view", () => {
    assert.equal(channelFilterId("rgba"), null);
  });

  it("returns a distinct filter id for every non-default channel", () => {
    const ids = ["rgb", "r", "g", "b", "a"].map((c) => channelFilterId(c as never));
    assert.ok(ids.every((id) => typeof id === "string" && id.length > 0), String(ids));
    assert.equal(new Set(ids).size, ids.length, "ids must be unique");
  });

  it("defines a matrix for every id it can return", () => {
    const defined = new Set(CHANNEL_MATRICES.map((m) => m.id));
    for (const option of PREVIEW_CHANNELS) {
      const id = channelFilterId(option.value);
      if (id === null) continue;
      assert.ok(defined.has(id), `no matrix defined for ${option.value} -> ${id}`);
    }
  });

  it("forces alpha to 1 in every matrix so nothing is multiplied by alpha", () => {
    // The point of the feature: a TGA's A channel is data, not transparency,
    // and RGB must show as stored rather than darkened by it.
    for (const matrix of CHANNEL_MATRICES) {
      const nums = matrix.values.trim().split(/\s+/).map(Number);
      assert.equal(nums.length, 20, `${matrix.id} must be a 4x5 matrix`);
      assert.deepEqual(
        nums.slice(15),
        [0, 0, 0, 0, 1],
        `${matrix.id} alpha row must force A=1`
      );
    }
  });

  it("broadcasts a single channel to R, G and B so it reads as greyscale", () => {
    const byId = new Map(CHANNEL_MATRICES.map((m) => [m.id, m.values]));
    for (const [channel, sourceIndex] of [["r", 0], ["g", 1], ["b", 2]] as const) {
      const nums = byId.get(channelFilterId(channel) as string)!.trim().split(/\s+/).map(Number);
      for (const outputRow of [0, 1, 2]) {
        assert.equal(
          nums[outputRow * 5 + sourceIndex],
          1,
          `${channel}: output row ${outputRow} must read source ${sourceIndex}`
        );
      }
    }
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run test 2>&1 | grep -E "Cannot find module|^# fail" | head -3
```

Expected: FAIL — `Cannot find module '../src/features/preview/previewRenderSettings.ts'`.

- [ ] **Step 3: Write the implementation**

Create `apps/web/src/features/preview/previewRenderSettings.ts`:

```typescript
// Pure mapping from the two preview render preferences to the CSS that applies
// them. No DOM, no React — the whole contract is testable under node --test.
//
// Both settings are CSS-level on purpose, applied to the shared `.preview-image`
// class that the canvas, <img> and <video> lanes all carry. One rule set covers
// every lane, and nothing repaints or re-decodes — which matters, because the
// preview pipeline exists to decode once ahead of time and a per-pixel
// implementation would undo it.

export type PreviewRenderFilter = "bilinear" | "nearest";
export type PreviewChannel = "rgba" | "rgb" | "r" | "g" | "b" | "a";

export const DEFAULT_PREVIEW_RENDER_FILTER: PreviewRenderFilter = "bilinear";
export const DEFAULT_PREVIEW_CHANNEL: PreviewChannel = "rgba";

// Named after the resampling algorithm the browser actually runs, never after a
// quality adjective. `high-quality` is deliberately absent: Chromium parses it
// but collapses it to `auto`, so a third row would label behaviour that does not
// happen. See docs/benchmarks/2026-08-02-preview-render-browser-checks.md.
export const PREVIEW_RENDER_FILTERS: ReadonlyArray<{
  value: PreviewRenderFilter;
  label: string;
}> = [
  { value: "bilinear", label: "双线性 Bilinear" },
  { value: "nearest", label: "最近邻 Nearest neighbour" }
];

export const PREVIEW_CHANNELS: ReadonlyArray<{ value: PreviewChannel; label: string }> = [
  { value: "rgba", label: "RGBA" },
  { value: "rgb", label: "RGB" },
  { value: "r", label: "R" },
  { value: "g", label: "G" },
  { value: "b", label: "B" },
  { value: "a", label: "A" }
];

export function imageRenderingForFilter(filter: PreviewRenderFilter): string {
  return filter === "nearest" ? "pixelated" : "auto";
}

const CHANNEL_FILTER_IDS: Record<Exclude<PreviewChannel, "rgba">, string> = {
  rgb: "mangoo-channel-rgb",
  r: "mangoo-channel-r",
  g: "mangoo-channel-g",
  b: "mangoo-channel-b",
  a: "mangoo-channel-a"
};

/**
 * The SVG filter id for a channel view, or `null` for the default RGBA view.
 *
 * `null` is load-bearing: the default must apply NO `filter` property at all, so
 * a user who never opens the panel pays nothing.
 */
export function channelFilterId(channel: PreviewChannel): string | null {
  return channel === "rgba" ? null : CHANNEL_FILTER_IDS[channel];
}

// 4x5 feColorMatrix rows: R, G, B, A. Each row is [r g b a offset].
//
// Every matrix ends with the alpha row `0 0 0 0 1`, forcing A=1. That is what
// makes these views show the RAW stored value: SVG defines feColorMatrix to
// operate on NON-premultiplied colour, so passing RGB through with alpha forced
// opaque yields the stored RGB undarkened — the whole point for formats like
// TGA where A carries data rather than transparency.
export const CHANNEL_MATRICES: ReadonlyArray<{ id: string; values: string }> = [
  // Colour as stored, alpha ignored.
  { id: CHANNEL_FILTER_IDS.rgb, values: "1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 0 1" },
  // One channel broadcast to R, G and B so it reads as greyscale.
  { id: CHANNEL_FILTER_IDS.r, values: "1 0 0 0 0  1 0 0 0 0  1 0 0 0 0  0 0 0 0 1" },
  { id: CHANNEL_FILTER_IDS.g, values: "0 1 0 0 0  0 1 0 0 0  0 1 0 0 0  0 0 0 0 1" },
  { id: CHANNEL_FILTER_IDS.b, values: "0 0 1 0 0  0 0 1 0 0  0 0 1 0 0  0 0 0 0 1" },
  { id: CHANNEL_FILTER_IDS.a, values: "0 0 0 1 0  0 0 0 1 0  0 0 0 1 0  0 0 0 0 1" }
];
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run test 2>&1 | grep -E "^# (tests|pass|fail)" | head -4
npm --workspace @mangoo/web run typecheck 2>&1 | tail -2
```

Expected: 6 new assertions pass; typecheck clean.

- [ ] **Step 5: Commit**

```bash
cd /d/Mangoo
git add apps/web/src/features/preview/previewRenderSettings.ts apps/web/tests/previewRenderSettings.test.mts
git commit -m "feat(web): add pure preview render settings mapping"
```

---

### Task 5: Persist the two settings in `PreviewPreferences`

**Files:**
- Modify: `apps/web/src/features/preview/previewPreferences.ts`
- Modify: `tools/checks/validate-web-client.mjs`
- Test: `apps/web/tests/previewRenderSettings.test.mts` (extend)

**Interfaces:**
- Consumes: Task 4's types and defaults.
- Produces: `PreviewPreferences.previewRenderFilter: PreviewRenderFilter` and `PreviewPreferences.previewChannel: PreviewChannel`. Tasks 6 and 7 read and write them.

**Why this is small:** `normalizePreviewPreferences` is already the single merge point for every field, and `readStoredPreviewPreferences` routes through it. Adding two fields there gives forward- and backward-compatibility with no new code — a blob written by an older build simply lacks the keys and falls back. That behaviour needs a test, not an implementation.

- [ ] **Step 1: Write the failing test**

Append to `apps/web/tests/previewRenderSettings.test.mts`:

```typescript
const { DEFAULT_PREVIEW_PREFERENCES, normalizePreviewPreferences } = await import(
  "../src/features/preview/previewPreferences.ts"
);

describe("render settings persistence", () => {
  it("defaults to bilinear and RGBA", () => {
    assert.equal(DEFAULT_PREVIEW_PREFERENCES.previewRenderFilter, "bilinear");
    assert.equal(DEFAULT_PREVIEW_PREFERENCES.previewChannel, "rgba");
  });

  it("falls back per field for a blob written before these keys existed", () => {
    // A user upgrading has a stored object with none of these keys. It must not
    // yield undefined, which would render as `image-rendering: undefined`.
    const normalized = normalizePreviewPreferences({ previewBufferLimitMb: 900 });
    assert.equal(normalized.previewRenderFilter, "bilinear");
    assert.equal(normalized.previewChannel, "rgba");
    assert.equal(normalized.previewBufferLimitMb, 900, "unrelated fields survive");
  });

  it("round-trips valid values", () => {
    const normalized = normalizePreviewPreferences({
      previewRenderFilter: "nearest",
      previewChannel: "a"
    });
    assert.equal(normalized.previewRenderFilter, "nearest");
    assert.equal(normalized.previewChannel, "a");
  });

  it("rejects a garbage value rather than passing it through to CSS", () => {
    const normalized = normalizePreviewPreferences({
      previewRenderFilter: "lanczos" as never,
      previewChannel: "cmyk" as never
    });
    assert.equal(normalized.previewRenderFilter, "bilinear");
    assert.equal(normalized.previewChannel, "rgba");
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run test 2>&1 | grep -E "^# fail|not ok" | head -4
```

Expected: FAIL — the new fields are `undefined`.

- [ ] **Step 3: Add the fields**

In `apps/web/src/features/preview/previewPreferences.ts`:

```typescript
import {
  DEFAULT_PREVIEW_CHANNEL,
  DEFAULT_PREVIEW_RENDER_FILTER,
  PREVIEW_CHANNELS,
  PREVIEW_RENDER_FILTERS,
  type PreviewChannel,
  type PreviewRenderFilter
} from "./previewRenderSettings";
```

Add to the `PreviewPreferences` interface:

```typescript
  // How the preview resamples when zoomed, and which colour channel it shows.
  // Global and persisted; see previewRenderSettings.ts for what each maps to.
  previewRenderFilter: PreviewRenderFilter;
  previewChannel: PreviewChannel;
```

Add to `DEFAULT_PREVIEW_PREFERENCES`:

```typescript
  previewRenderFilter: DEFAULT_PREVIEW_RENDER_FILTER,
  previewChannel: DEFAULT_PREVIEW_CHANNEL,
```

Add to the object `normalizePreviewPreferences` returns:

```typescript
    // Validate against the option lists rather than trusting the stored string:
    // an unknown value would otherwise reach CSS as `image-rendering: lanczos`
    // or a filter id that does not exist.
    previewRenderFilter: PREVIEW_RENDER_FILTERS.some(
      (option) => option.value === value.previewRenderFilter
    )
      ? (value.previewRenderFilter as PreviewRenderFilter)
      : DEFAULT_PREVIEW_RENDER_FILTER,
    previewChannel: PREVIEW_CHANNELS.some((option) => option.value === value.previewChannel)
      ? (value.previewChannel as PreviewChannel)
      : DEFAULT_PREVIEW_CHANNEL,
```

- [ ] **Step 4: Extend the preference guard**

In `tools/checks/validate-web-client.mjs`, inside the existing preview-preferences `if (...)` condition, add:

```javascript
  !/previewRenderFilter/.test(previewPreferences) ||
  !/previewChannel/.test(previewPreferences) ||
```

- [ ] **Step 5: Verify and commit**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run test 2>&1 | grep -E "^# (tests|pass|fail)" | head -4
npm --workspace @mangoo/web run typecheck 2>&1 | tail -2
npm run check:web 2>&1 | grep -E "^FAIL|^PASS: web" | head -3
git add apps/web/src/features/preview/previewPreferences.ts apps/web/tests/previewRenderSettings.test.mts tools/checks/validate-web-client.mjs
git commit -m "feat(web): persist the preview render filter and channel"
```

---

### Task 6: The `PreviewRenderMenu` popover

**Files:**
- Create: `apps/web/src/features/preview/PreviewRenderMenu.tsx`
- Modify: `apps/web/src/design/overlays/overlayTypes.ts`

**Interfaces:**
- Consumes: Task 4's `PREVIEW_RENDER_FILTERS`, `PREVIEW_CHANNELS`, and both types.
- Produces: `PreviewRenderMenu`, a presentational component owning no state:

```typescript
interface PreviewRenderMenuProps {
  open: boolean;
  filter: PreviewRenderFilter;
  channel: PreviewChannel;
  onChangeFilter: (filter: PreviewRenderFilter) => void;
  onChangeChannel: (channel: PreviewChannel) => void;
  onOpenChange: (open: boolean) => void;
}
```

Task 7 renders it.

**Model it on `apps/web/src/features/library/SortMenu.tsx`** — same `LiquidGlassButton` + `AnchoredOverlay` + `OverlayMenuList`/`OverlayMenuItem` shape, same `tone="control"`, same `overlay-titlebar-popover` class. Read that file before writing this one.

- [ ] **Step 1: Register the overlay root**

In `apps/web/src/design/overlays/overlayTypes.ts`, add `"preview-render"` to the `OverlayRootId` union:

```typescript
export type OverlayRootId =
  | "library"
  | "filter"
  | "sort"
  | "layout"
  | "preview-render"
  | "notifications"
  | "context-menu"
  | "file-operation";
```

- [ ] **Step 2: Write the component**

Create `apps/web/src/features/preview/PreviewRenderMenu.tsx`:

```tsx
import { SlidersHorizontal } from "lucide-react";
import { useRef } from "react";

import { LiquidGlassButton } from "../../design/liquid-glass";
import { AnchoredOverlay, OverlayMenuItem, OverlayMenuList } from "../../design/overlays";

import {
  PREVIEW_CHANNELS,
  PREVIEW_RENDER_FILTERS,
  type PreviewChannel,
  type PreviewRenderFilter
} from "./previewRenderSettings";

interface PreviewRenderMenuProps {
  open: boolean;
  filter: PreviewRenderFilter;
  channel: PreviewChannel;
  onChangeFilter: (filter: PreviewRenderFilter) => void;
  onChangeChannel: (channel: PreviewChannel) => void;
  onOpenChange: (open: boolean) => void;
}

// Render controls for the preview surface: which resampling algorithm the
// browser uses when zoomed, and which colour channel is displayed.
//
// Deliberately stateless — App owns the values (they are persisted preferences)
// and the open flag, exactly as it does for SortMenu.
export function PreviewRenderMenu({
  open,
  filter,
  channel,
  onChangeFilter,
  onChangeChannel,
  onOpenChange
}: PreviewRenderMenuProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);

  return (
    <div className="preview-render-menu">
      <LiquidGlassButton
        ref={buttonRef}
        active={open}
        aria-controls="preview-render-root"
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label="Render settings"
        className="titlebar-icon-button"
        data-titlebar-control="preview-render"
        onClick={() => onOpenChange(!open)}
        title="Render settings"
        tone="control"
        type="button"
      >
        <SlidersHorizontal aria-hidden="true" size={15} />
      </LiquidGlassButton>

      <AnchoredOverlay
        anchor={buttonRef.current}
        ariaLabel="Render settings"
        className="overlay-titlebar-popover preview-render-root"
        id="preview-render-root"
        onDismiss={() => onOpenChange(false)}
        open={open}
        placement="bottom-end"
        preferredWidth={220}
        restoreFocusTarget={buttonRef.current}
        role="group"
        rootId="preview-render"
      >
        <div className="preview-render-group">
          <div className="preview-render-group-label" id="preview-render-filter-label">
            缩放过滤
          </div>
          <OverlayMenuList
            ariaLabel="缩放过滤"
            className="overlay-titlebar-menu"
            role="listbox"
          >
            {PREVIEW_RENDER_FILTERS.map((option) => {
              const selected = filter === option.value;
              return (
                <OverlayMenuItem
                  aria-selected={selected}
                  checked={selected}
                  key={option.value}
                  onClick={() => onChangeFilter(option.value)}
                  role="option"
                  selected={selected}
                >
                  {option.label}
                </OverlayMenuItem>
              );
            })}
          </OverlayMenuList>
        </div>

        <div className="preview-render-group">
          <div className="preview-render-group-label" id="preview-render-channel-label">
            通道
          </div>
          <OverlayMenuList ariaLabel="通道" className="overlay-titlebar-menu" role="listbox">
            {PREVIEW_CHANNELS.map((option) => {
              const selected = channel === option.value;
              return (
                <OverlayMenuItem
                  aria-selected={selected}
                  checked={selected}
                  key={option.value}
                  onClick={() => onChangeChannel(option.value)}
                  role="option"
                  selected={selected}
                >
                  {option.label}
                </OverlayMenuItem>
              );
            })}
          </OverlayMenuList>
        </div>
      </AnchoredOverlay>
    </div>
  );
}
```

Note: unlike `SortMenu`, selecting an option does **not** close the popover. These are live render controls — you change one, look at the image, change another.

- [ ] **Step 3: Verify it compiles**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run typecheck 2>&1 | tail -3
```

Expected: clean. (It is not rendered yet; Task 7 mounts it.)

- [ ] **Step 4: Commit**

```bash
cd /d/Mangoo
git add apps/web/src/features/preview/PreviewRenderMenu.tsx apps/web/src/design/overlays/overlayTypes.ts
git commit -m "feat(web): add the preview render settings popover"
```

---

### Task 7: Mount it, and make the settings reach pixels

**Files:**
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/app-shell/AppShell.tsx`
- Modify: `apps/web/src/styles.css`

**Interfaces:**
- Consumes: Task 4's `imageRenderingForFilter`, `channelFilterId`, `CHANNEL_MATRICES`; Task 5's two preference fields; Task 6's `PreviewRenderMenu`.
- Produces: the working feature.

- [ ] **Step 1: Add the right-hand slot to the preview toolbar**

In `apps/web/src/app-shell/ShellTopBar.tsx`, add `renderActions?: ReactNode` to `PreviewTitlebarToolbarProps`, accept it in the parameter list, and render it immediately **before** the existing conditional `shellActions` block:

```tsx
      {renderActions ? (
        <div className="titlebar-preview-actions no-drag" data-no-drag="true">
          {renderActions}
        </div>
      ) : null}
```

This keeps the left-hand control cluster and the centred title untouched.

- [ ] **Step 2: Own the state and pass the menu in**

In `App.tsx`, beside `sortMenuOpen`:

```tsx
  const [previewRenderMenuOpen, setPreviewRenderMenuOpen] = useState(false);
```

In `renderCenterTitlebar`'s preview branch, pass:

```tsx
          renderActions={
            <PreviewRenderMenu
              channel={previewPreferences.previewChannel}
              filter={previewPreferences.previewRenderFilter}
              onChangeChannel={(previewChannel) =>
                setPreviewPreferences((current) => ({ ...current, previewChannel }))
              }
              onChangeFilter={(previewRenderFilter) =>
                setPreviewPreferences((current) => ({ ...current, previewRenderFilter }))
              }
              onOpenChange={setPreviewRenderMenuOpen}
              open={previewRenderMenuOpen}
            />
          }
```

The existing `useEffect` on `previewPreferences` already calls `storePreviewPreferences`, so persistence and cross-window sync come for free.

Import `PreviewRenderMenu` from `../features/preview/PreviewRenderMenu`.

- [ ] **Step 3: Put the settings on the stage**

`CentralPreviewStage` renders `.central-preview-stage`. Rather than thread two more props through it, drive the CSS from attributes on the element App already controls — add them to the preview branch's wrapper in `App.tsx` where `CentralPreviewStage` is rendered, or, if that is not an element App owns, pass the two values into `CentralPreviewStage` as props and put the attributes on `.central-preview-stage` itself:

```tsx
        <div
          className="central-preview-stage"
          data-preview-filter={previewRenderFilter}
          data-preview-channel={previewChannel}
          data-preview-video={isVideo ? "true" : undefined}
        >
```

Read `CentralPreviewStage.tsx` first and pick whichever is the smaller change; both are acceptable. Whichever you choose, the attributes must sit on an **ancestor of `.preview-image`**.

- [ ] **Step 4: Define the matrices once, in the app shell**

In `apps/web/src/app-shell/AppShell.tsx`, inside the root `.app-shell` element, render a hidden SVG holding every channel matrix. Generate it from `CHANNEL_MATRICES` so the ids can never drift from the mapping module:

```tsx
      <svg aria-hidden="true" className="preview-channel-filters" focusable="false">
        <defs>
          {CHANNEL_MATRICES.map((matrix) => (
            <filter
              colorInterpolationFilters="sRGB"
              id={matrix.id}
              key={matrix.id}
            >
              <feColorMatrix type="matrix" values={matrix.values} />
            </filter>
          ))}
        </defs>
      </svg>
```

`colorInterpolationFilters="sRGB"` is required: the default is `linearRGB`, which would gamma-shift every channel view and make the displayed values wrong.

- [ ] **Step 5: The CSS**

In `apps/web/src/styles.css`, next to the existing `.preview-image` rule:

```css
/* The matrices are referenced by CSS `filter`; the SVG itself never renders. */
.preview-channel-filters {
  position: absolute;
  width: 0;
  height: 0;
  overflow: hidden;
}

/* Scaling filter and channel isolation apply to EVERY preview lane — the
   canvas, the <img> used for animated formats, and <video> — because all three
   carry `.preview-image`. Purely CSS, so nothing repaints or re-decodes. */
.central-preview-stage[data-preview-filter="nearest"] .preview-image {
  image-rendering: pixelated;
}
.central-preview-stage[data-preview-filter="bilinear"] .preview-image {
  image-rendering: auto;
}

/* No rule for data-preview-channel="rgba": the default must apply no filter. */
.central-preview-stage[data-preview-channel="rgb"] .preview-image {
  filter: url(#mangoo-channel-rgb);
}
.central-preview-stage[data-preview-channel="r"] .preview-image {
  filter: url(#mangoo-channel-r);
}
.central-preview-stage[data-preview-channel="g"] .preview-image {
  filter: url(#mangoo-channel-g);
}
.central-preview-stage[data-preview-channel="b"] .preview-image {
  filter: url(#mangoo-channel-b);
}
.central-preview-stage[data-preview-channel="a"] .preview-image {
  filter: url(#mangoo-channel-a);
}
```

- [ ] **Step 6: Style the popover groups**

```css
.preview-render-group + .preview-render-group {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--glass-divider, rgba(255, 255, 255, 0.08));
}

.preview-render-group-label {
  padding: 4px 10px 2px;
  font-size: 11px;
  opacity: 0.6;
}
```

- [ ] **Step 7: Verify and commit**

```bash
cd /d/Mangoo
npm --workspace @mangoo/web run typecheck 2>&1 | tail -2
npm --workspace @mangoo/web run test 2>&1 | grep -E "^# (tests|pass|fail)" | head -4
npm run check:web 2>&1 | grep -E "^FAIL|^PASS: web" | head -3
npm run check:ui-design 2>&1 | tail -2
```

If any scripted edit was used, normalise line endings before gating — a CRLF conversion breaks `check:ui-design`:

```bash
cd /d/Mangoo
python -c "
import subprocess, os
for f in subprocess.run(['git','diff','--name-only','HEAD'],capture_output=True,text=True).stdout.split():
    if os.path.isfile(f):
        d=open(f,'rb').read()
        if b'\r\n' in d:
            open(f,'wb').write(d.replace(b'\r\n',b'\n')); print('normalised',f)
"
```

```bash
cd /d/Mangoo
git add apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/app-shell/AppShell.tsx apps/web/src/app/App.tsx apps/web/src/features/preview/CentralPreviewStage.tsx apps/web/src/styles.css
git commit -m "feat(web): apply the preview render settings from the titlebar"
```

---

### Task 8: Full gate and real-app verification

**Files:**
- Modify: `docs/benchmarks/2026-08-02-preview-render-browser-checks.md` (append results)

**This task is not optional, and its findings may not be guessed.** Both features are user-visible behaviour that no unit test can confirm.

- [ ] **Step 1: Full gate**

```bash
cd /d/Mangoo
export PATH="$HOME/.cargo/bin:$PATH"
npm test 2>&1 | grep -E "^(PASS|FAIL)|test result:" | head -16
```

Expected: exit 0, every stage PASS. Fix anything red before continuing.

- [ ] **Step 2: Launch against the benchmark folder**

```bash
cd /d/Mangoo
export MANGOO_AUTO_ADD_ROOT="C:/Users/84460/Pictures/normal"
export MANGOO_REMOTE_DEBUG=1
nohup npm run dev > /tmp/dev_verify.log 2>&1 &
sleep 85
```

- [ ] **Step 3: Verify the navigation gate**

Record each of these:

- **Wheel:** one deliberate trackpad flick / one mouse-wheel notch advances **exactly one** image. Before this change a single flick could skip several.
- **Held arrow key, warm:** walk a folder you have already browsed. It should feel unchanged — every pooled neighbour paints in the same tick, so the gate never engages.
- **Held arrow key, cold:** restart the app so the pool is empty, then hold Right. Every image must actually appear; none may be skipped.
- **Broken image:** drop a file with a valid image extension but garbage content into the root (`printf 'not an image' > "C:/Users/84460/Pictures/normal/broken_test.png"`), let the watcher index it, then navigate onto and past it. **Navigation must not wedge.** Delete the file afterwards.

- [ ] **Step 4: Verify the render settings**

- The button is present in the titlebar for an image **and for a video**.
- Zoom well past 100%, switch 最近邻 / 双线性 — pixel edges must visibly change.
- Switch through RGBA / RGB / R / G / B / A — each must visibly change the image, and R/G/B/A must read as greyscale rather than tinted.
- **The alpha claim:** open a PNG with real transparency, select **RGB**, and confirm the colour in the transparent region shows the stored RGB at full strength rather than a darkened blend. This is the behaviour Task 1 predicted; confirm it held in the real surface, not just the probe.
- Restart the app and confirm both settings survived.

- [ ] **Step 5: Record and clean up**

Append a "Results" section to `docs/benchmarks/2026-08-02-preview-render-browser-checks.md` with what you observed for each bullet above — including anything that did **not** work. Then:

```bash
cd /d/Mangoo
taskkill //F //IM mangoo-core.exe 2>/dev/null | head -1
taskkill //F //IM electron.exe 2>/dev/null | head -1
rm -f "C:/Users/84460/Pictures/normal/broken_test.png"
git status --short
```

`git status` must show only the user's own eight pre-existing files.

```bash
cd /d/Mangoo
git add docs/benchmarks/2026-08-02-preview-render-browser-checks.md
git commit -m "docs: record preview gate and render settings verification"
```

---

## Plan-level self-review notes

- **Spec coverage.** §3a–3d → Tasks 2 and 3 (including the broken-image release, tested in Task 2 and exercised in Task 8). §4a/4a-bis → Task 4. §4b → Task 6. §4c → Task 5. §4d → Tasks 4 and 7, with the two assumptions verified in Task 1 *before* anything is built on them. §5 → the per-task tests plus Task 8.
- **The plan can stop itself.** Task 1 Step 4 halts and escalates if `feColorMatrix` turns out to premultiply, because the channel feature would then need a WebGL paint path — a materially different plan.
- **Third filter option is conditional, not assumed.** Task 4 ships two rows unless Task 1 proves a third is real and distinct. A row that names an algorithm the browser is not running is worse than no row.
- **Two process traps are called out inline**, both learned the hard way on the previous plan: commit before mutation-testing a guard (`git checkout` destroys uncommitted work), and pass `newline="\n"` on every scripted write (CRLF breaks `check:ui-design`).
- **Type consistency:** `PreviewRenderFilter` / `PreviewChannel` are defined once in Task 4 and imported by Tasks 5, 6, 7. `channelFilterId` returns `string | null`, and the `null` case has no CSS rule by design — the RGBA default applies no filter at all.

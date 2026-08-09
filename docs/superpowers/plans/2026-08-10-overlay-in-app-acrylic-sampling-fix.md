# Overlay Regional Flatten-Then-Blur Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Task 4 complete on 2026-08-10; automated verification green and same-reviewer Spec/Quality review passed.

**Goal:** Make renderer popups and Rename/Move To modals blur the final, flattened application scene so independent images, clipped boxes, and text boundaries become uniformly frosted while content outside the overlay stays sharp.

**Architecture:** Keep overlays portal-mounted under `document.body`, but stop using element `backdrop-filter`. `OverlayProvider` owns a regional SVG-filter controller that applies `SourceGraphic -> Gaussian blur -> saturation -> synchronous primitive-built rounded alpha mask -> merge with untouched SourceGraphic` to `#root` and, for nested depth, to lower overlay surfaces. The controller restores prior inline filters on cleanup and disables itself for zero blur or solid fallback; it has no image decode or capture dependency.

**Tech Stack:** React 18, TypeScript, SVG Filter Effects, Electron 43/Chromium, Node test runner, Vite fixture windows, PowerShell `System.Drawing`, existing desktop OS-backdrop harness.

## Global Constraints

- The approved design is `docs/superpowers/specs/2026-08-09-overlay-frosted-material-and-radius-unification-design.md` and governs this plan.
- Dialog blur remains `0px`, `34px`, and `68px` at minimum, default, and maximum; the Gaussian primitive uses that CSS-pixel value as `stdDeviation` and expands work bounds by `3 * stdDeviation`.
- `.overlay-surface` paints the approved 108-degree right-content gradient, border, dither, and shadow, but both element backdrop-filter declarations are `none`.
- `.overlay-surface > .liquid-glass-backdrop` stays transparent and unfiltered.
- `.overlay-modal-backdrop` stays transparent, unfiltered, and free of opacity animation; only the modal surface moves.
- Modal dimming remains outside-only through `0 0 0 100vmax var(--dialog-backdrop-dim)` in the same `box-shadow` list as `var(--glass-shadow)`.
- Root/nested/modal stack ownership, focus, dismissal, portals, z-index, pointer glow, Electron window lifecycle, and all non-overlay behavior remain unchanged.
- Reduced transparency, unsupported SVG filters, forced visual fallback, and `0px` blur remove every generated filter; fallback fill stays `rgba(18, 24, 30, 0.98)` on the overlay surface.
- Stack and direct surface/root mutations update synchronously before paint; resize and captured-scroll work is event-driven and coalesced with animation frames. No continuous polling or screenshot/capture IPC is added.
- The current checkout contains unrelated user work and the earlier interrupted overlay attempt. Work in place because this is a continuation, preserve unrelated changes, and stage only files named by this plan.
- A regional popup mask always uses the complete outer border box and outer radius. Do not inset it for the new real border.
- The frame target is the exact historical `backdrop` probe: gradient and `1px solid rgba(255, 255, 255, 0.3)` border on the same element, with no overlay `::after` border or inset edge shadow.
- The 100% renderer capture is independently mandatory for frame alignment; 125% and 150% remain additional scale coverage, not the reason this defect is detectable.

---

### Task 1: Specify and Test Regional Geometry

**Files:**
- Create: `apps/web/src/design/overlays/overlayRegionalBlurPlan.ts`
- Create: `apps/web/tests/overlayRegionalBlurPlan.test.mts`

**Interfaces:**
- Produces: `buildOverlayRegionalBlurPlans(input): OverlayRegionalBlurPlan[]`.
- Consumes later: viewport-space layer rectangles and target rectangles; each returned mask is target-local.

- [x] **Step 1: Write the failing geometry tests**

Create literal tests for these observable cases:

```ts
const popup = rect(200, 100, 300, 200);
assert.deepEqual(buildOverlayRegionalBlurPlans({
  sigma: 34,
  targets: [{ id: "app-root", layerIndex: -1, rect: rect(0, 0, 1000, 700) }],
  layers: [{ id: "root", kind: "root", radius: 12, rect: popup }]
}), [{
  targetId: "app-root",
  targetRect: rect(0, 0, 1000, 700),
  masks: [{ left: 200, top: 100, width: 300, height: 200, radius: 12 }],
  blurBounds: { left: 98, top: 0, width: 504, height: 402 }
}]);
```

Also assert: a nested layer adds masks to `app-root` and only its intersection to the lower root surface; a modal produces one full-target mask for every lower target; zero sigma returns no plans; invalid/empty intersections are omitted; clipped mask radii never exceed half of their smallest dimension.

- [x] **Step 2: Run the focused test and verify RED**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/overlayRegionalBlurPlan.test.mts
```

Expected: FAIL with module-not-found because `overlayRegionalBlurPlan.ts` does not exist.

- [x] **Step 3: Implement the pure planner**

Use these exact public shapes:

```ts
export interface OverlayRegionalRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface OverlayRegionalLayerGeometry {
  id: string;
  kind: "root" | "nested" | "modal";
  radius: number;
  rect: OverlayRegionalRect;
}

export interface OverlayRegionalTargetGeometry {
  id: string;
  layerIndex: number;
  rect: OverlayRegionalRect;
}

export interface OverlayRegionalBlurPlan {
  targetId: string;
  targetRect: OverlayRegionalRect;
  masks: Array<{ left: number; top: number; width: number; height: number; radius: number }>;
  blurBounds: { left: number; top: number; width: number; height: number };
}
```

For each target, consider only layers whose index is greater than `layerIndex`. If any such layer is modal, emit one mask covering the complete target. Otherwise intersect every higher layer with the target, convert the result to target-local coordinates, and clamp radius to half of the local width/height. Union mask bounds, expand by `3 * sigma`, and clip to the target dimensions.

- [x] **Step 4: Run the focused test GREEN**

Run the Step 2 command. Expected: all regional geometry cases pass.

- [x] **Step 5: Commit the pure unit**

```powershell
git add -- apps/web/src/design/overlays/overlayRegionalBlurPlan.ts apps/web/tests/overlayRegionalBlurPlan.test.mts
git commit -m "test(web): define regional overlay blur geometry"
```

---

### Task 2: Bind the Regional Filter to the Real Overlay Stack

**Files:**
- Create: `apps/web/src/design/overlays/overlayRegionalBlur.ts`
- Create: `apps/web/tests/overlayRegionalBlurLifecycle.test.mts`
- Create: `apps/web/tests/fixtures/overlayRegionalBlurLifecycleFixture.tsx`
- Create: `apps/web/tests/fixtures/overlay-regional-blur-lifecycle.html`
- Create: `apps/web/tests/fixtures/runOverlayRegionalBlurLifecycle.mjs`
- Modify: `apps/web/src/design/overlays/OverlayProvider.tsx`
- Modify: `apps/web/src/design/overlays/overlay.css`
- Modify: `apps/web/tests/overlayContract.test.mts`
- Modify: `tools/checks/validate-ui-design.mjs`

**Interfaces:**
- Consumes: Task 1 planner and ordered `OverlayLayerRegistration[]`.
- Produces: `createOverlayRegionalBlurController(document): { setLayers(layers): void; destroy(): void }`.
- Produces: one generated `svg[data-overlay-regional-blur-defs="true"]`; active targets carry `data-overlay-regional-blur="active"` only while filtered.

- [x] **Step 1: Write the mounted lifecycle and material tests first**

The Electron fixture must drive these real phases through `OverlayProvider`: root popup, nested popup, modal above them, all closed, forced fallback, and zero blur. The result must assert:

```ts
assert.equal(result.root.rootFiltered, true);
assert.equal(result.root.gaussianInput, "SourceGraphic");
assert.equal(result.root.stdDeviation, "34");
assert.equal(result.root.edgeMode, "duplicate");
assert.equal(result.root.roundedMaskPrimitiveCount, 1);
assert.equal(result.nested.lowerSurfaceFiltered, true);
assert.equal(result.modal.rootMaskCoversTarget, true);
assert.equal(result.cleanup.defsPresent, false);
assert.equal(result.cleanup.rootInlineFilter, "brightness(1)");
assert.equal(result.fallback.rootFiltered, false);
assert.equal(result.zeroBlur.rootFiltered, false);
assert.equal(result.atomic.decodeCalls, 0);
assert.equal(result.atomic.modalPoppedByNextFrame, true);
assert.equal(result.atomic.nestedPoppedByNextFrame, true);
assert.equal(result.atomic.geometryChangedByNextFrame, true);
```

Update the source contract and validator so the visible overlay surface must contain the gradient plus both `backdrop-filter: none` declarations, while the generated controller path is required and the direct child remains inert.

- [x] **Step 2: Run the new lifecycle/contract/validator checks and verify RED**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/overlayRegionalBlurLifecycle.test.mts apps/web/tests/overlayContract.test.mts
npm run check:ui-design
```

Expected: FAIL because the controller and lifecycle fixture are absent and `.overlay-surface` still owns `backdrop-filter`.

- [x] **Step 3: Implement the controller and provider integration**

`OverlayProvider` creates one controller in `useLayoutEffect`, forwards every committed `state.layers`, and destroys it on unmount. The controller must:

```ts
export interface OverlayRegionalBlurController {
  setLayers(layers: readonly OverlayLayerRegistration[]): void;
  destroy(): void;
}

export function createOverlayRegionalBlurController(
  ownerDocument: Document
): OverlayRegionalBlurController;
```

On an update, read `--glass-dialog-blur`, `--glass-center-saturation`, target rectangles, and computed border radii. Build targets for `#root` at layer index `-1` and every non-top layer surface at its stack index. Build one SVG `<filter>` per returned plan with `filterUnits="userSpaceOnUse"`, `primitiveUnits="userSpaceOnUse"`, `color-interpolation-filters="sRGB"`, and this exact graph:

```xml
<feGaussianBlur in="SourceGraphic" stdDeviation="34" edgeMode="duplicate" result="blurred" />
<feColorMatrix in="blurred" type="saturate" values="1.55" result="saturated" />
<feFlood x="410" y="190" width="420" height="300" flood-color="white" result="mask-source" />
<feGaussianBlur in="mask-source" stdDeviation="11.88" result="mask-softened" />
<feComponentTransfer in="mask-softened" result="mask-rounded">
  <feFuncA type="linear" slope="16" intercept="-7.5" />
</feComponentTransfer>
<feComposite in="saturated" in2="mask-rounded" operator="in" result="blurred-inside" />
<feComposite in="SourceGraphic" in2="mask-rounded" operator="out" result="clear-outside" />
<feMerge><feMergeNode in="clear-outside"/><feMergeNode in="blurred-inside"/></feMerge>
```

For radius zero, use the hard `feFlood` directly. For a rounded mask, blur the flood with `radius * 0.54`, then sharpen its alpha around `0.5`; merge multiple mask results before the two final composites. This avoids Blink's asynchronous `feImage` resource path and allows stack changes, direct surface geometry changes, fallback, and zero-blur clearing to install synchronously. Coalesce only resize and captured scroll into animation-frame updates. Do not hide a new surface while React assigns initial focus. On final close or destroy, disconnect observers/listeners, cancel frames, remove definitions/attributes, and restore each target's exact prior inline `filter` value.

- [x] **Step 4: Switch CSS ownership to the regional path**

Keep the dialog gradient on `.overlay-surface`, but set both surface filter declarations to `none`:

```css
.overlay-surface {
  background: var(
    --overlay-solid-fallback-fill,
    linear-gradient(
      108deg,
      var(--glass-dialog-pane-light) 0%,
      var(--glass-dialog-pane-mid) 52%,
      var(--glass-dialog-pane-end) 100%
    )
  );
  -webkit-backdrop-filter: none;
  backdrop-filter: none;
  box-shadow: var(--glass-shadow);
}
```

Change unsupported-filter fallback detection to the SVG `filter` capability. Preserve the forced/reduced-transparency fill path and all modal rules from the approved spec.

- [x] **Step 5: Run lifecycle/contract/validator checks GREEN**

Run the Step 2 commands, then:

```powershell
npm run check:web
```

Expected: controller lifecycle, CSS contract, validator, web typecheck, and web tests all pass with no Electron child-process leak.

- [x] **Step 6: Commit the runtime unit**

Stage only the nine Task 2 files and commit:

```powershell
git commit -m "fix(web): flatten content before regional overlay blur"
```

---

### Task 3: Replace the Misleading Pixel Probe with Real Multi-Layer Evidence

**Files:**
- Modify: `apps/web/tests/fixtures/overlayAppearanceVisualFixture.tsx`
- Modify: `apps/web/tests/fixtures/overlayAppearanceVisualFixture.css`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`
- Modify: `docs/superpowers/specs/2026-08-09-overlay-frosted-material-and-radius-unification-design.md`
- Modify: `docs/superpowers/plans/2026-08-10-overlay-in-app-acrylic-sampling-fix.md`

**Interfaces:**
- Consumes: the real registered `AnchoredOverlay` and controller from Task 2.
- Produces: `appearanceMatrix.blurPixels` with zero/default/maximum independent-image measurements and `realPortal` measurements from the populated Library.

- [x] **Step 1: Upgrade the fixture and screenshot analyzer before accepting GREEN**

Replace the checkerboard-only probe with at least six independent data-URI `<img>` elements, separate clipped card boxes, gaps, and DOM labels. Mount a real `AnchoredOverlay` over the scene when the modal is toggled off. Analyze known card/image boundaries separately from interior texture and store:

```js
{
  boundaryEnergy,
  interiorEnergy,
  boundaryInteriorRatio,
  outsideReferencePixels
}
```

The literal hard pass contract is:

```js
default.boundaryEnergy <= zero.boundaryEnergy * 0.82;
default.textEdgeEnergy <= zero.textEdgeEnergy * 0.82;
default.boundaryInteriorRatio <= 3.0;
maximum.boundaryEnergy <= default.boundaryEnergy * 1.05;
maximum.textEdgeEnergy <= default.textEdgeEnergy * 1.05;
referenceDelta(zero, default) <= 1;
referenceDelta(zero, maximum) <= 1;
```

The surface and direct child must both report `backdrop-filter: none`; default/maximum must instead report an active generated filter on `#root`, while zero reports none.

- [x] **Step 2: Add the real Library portal-popover capture**

At each dialog-blur value, open the real notification popup over the populated media grid, fix it to a deterministic `360x220` region through a harness-only `!important` rule, hide its foreground content only for sampling, and measure the intersecting real thumbnail/image/text boundaries with the same analyzer. Reopen the popup after every auxiliary Settings-window slider change because production correctly dismisses a main-window popup when focus moves to another BrowserWindow. Require default and maximum boundary/interior ratios at most `3.0`, outside reference delta at most `1`, and an active regional root filter only when blur is non-zero.

- [x] **Step 3: Run the OS-backdrop harness and inspect RED/GREEN evidence**

Run:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'
node .tmp\visual-check\desktop-ui-regression.mjs
Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
```

Expected GREEN: zero/default/maximum screenshots exist; independent fixture and real Library portal measurements pass; popup, nested popup, modal, fallback, and cleanup topology pass; no interaction timeout, console error, network error, or leftover GUI process is reported.

- [x] **Step 4: Exercise scale and modal coverage**

Use a device-scale-only 1.0/1.25/1.5 matrix with page scale fixed at `1` to verify rounded masks stay in CSS-pixel alignment. Capture the real registered popup at zero/default/maximum and the shared production `ModalSurface` at all three scales. The mounted file-operation suite proves Rename and Move To consume that shared modal primitive, while the lifecycle fixture proves nested lower-surface filtering and modal full-target masks; the scale captures verify the common renderer path behind those consumers.

- [x] **Step 5: Run final verification and update status**

Set the spec status to:

```markdown
**Status:** Implemented; regional-mask edge containment is superseded by `2026-08-10-overlay-regional-mask-edge-alignment-design.md` after physical 100% review exposed a one-pixel rounded-mask alpha tail
```

Run fresh:

```powershell
npm run check:ui-design
npm run check:web
npm run check:desktop
npm test
git diff --check
git status --short
```

Expected: every command exits 0; only plan-named overlay files are staged; unrelated user changes remain unstaged and untouched.

- [x] **Step 6: Commit the visual evidence and documentation**

```powershell
git commit -m "test(ui): verify flattened overlay frost on real content"
```

---

### Task 4: Stabilize the First Frame and Reproduce the Historical Popup Frame

**Files:**
- Modify: `apps/web/src/design/overlays/overlayRegionalBlur.ts`
- Modify: `apps/web/src/design/overlays/overlay.css`
- Modify: `apps/web/tests/fixtures/runOverlayRegionalBlurLifecycle.mjs`
- Modify: `apps/web/tests/overlayRegionalBlurLifecycle.test.mts`
- Modify: `apps/web/tests/overlayContract.test.mts`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `apps/web/tests/fixtures/overlayAppearanceVisualFixture.tsx`
- Modify: `apps/web/tests/fixtures/overlayAppearanceVisualFixture.css`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`
- Modify: this spec and plan

**Historical reference:** `.tmp/regional-filter-probe/main.cjs` captured `baseline`, `backdrop`, and `regional` in that order. The requested second image is `backdrop.png`; its source frame is `border: 1px solid rgba(255,255,255,.3)` and the 108-degree gradient on the same `#surface` element.

- [x] **Step 1: Write and run the first-frame lifecycle regression RED**

Install a mutation/animation-frame trace before opening the real `AnchoredOverlay`. Every active root-filter snapshot must have a visible, finite, positive surface box at its final non-origin position, and its first mask must equal the complete outer surface border box. The current implementation must fail because it registers the hidden `left:0; top:0` pre-layout surface.

- [x] **Step 2: Write and run the frame contract RED**

Require the mounted surface to report a real `1px solid rgba(255, 255, 255, 0.3)` border on the gradient owner, while `::after` reports no border and no inset edge shadow. Add a controlled real-popup renderer capture at device scale `1.0`; its hard pass must confirm a visible one-physical-pixel stroke and no classified fill pixels outside the stroke envelope. Keep the existing 1.25/1.5 matrix as secondary coverage. The current pseudo-plane implementation must fail.

- [x] **Step 3: Reject hidden or invalid geometry without changing mask bounds**

In `collectGeometry`, read each surface style and border box once. Exclude disconnected surfaces, `visibility:hidden`, `display:none`, non-finite boxes, and boxes whose width or height is not positive. Let the existing surface `style` observer synchronously rebuild when the positioned visible style lands. Continue passing the unmodified outer `getBoundingClientRect()` and computed outer radius to the planner; do not subtract the border.

- [x] **Step 4: Move the historical frame onto the real surface**

Paint `border: 1px solid rgba(255, 255, 255, 0.3)` on `.overlay-surface` itself, keep the approved gradient on that same element, and suppress the generic overlay `::after` border and inset edge shadow. Remove or override feature `border:0` rules that would erase the shared frame. Do not add `background-clip: padding-box` or a regional-mask inset; neither exists in the approved historical probe.

- [x] **Step 5: Run focused and full verification GREEN**

Run the lifecycle/contract tests first, then `npm run check:ui-design`, `npm run check:web`, `npm run check:desktop`, the OS-backdrop harness, `npm test`, and `git diff --check`. Verify the 100% frame result separately in the harness summary and ensure no Electron process remains.

- [x] **Step 6: Same-reviewer Spec and Quality review, then commit**

Use the existing overlay reviewer for both verdicts. Stage only Task 4 files and create one focused commit without touching unrelated user work.

## Self-Review

- Spec coverage: regional root/nested masks, modal full-target masks, cleanup/fallback, right-pane material, zero/default/maximum blur, real replaced-image boundaries, and 100/125/150 scale evidence all map to Tasks 1-3.
- Placeholder scan: every step names its implementation, assertion, command, and literal threshold.
- Type consistency: Task 2 consumes the exact planner shapes emitted by Task 1; Task 3 observes the target attributes and filters emitted by Task 2.
- Mutation check: using `backdrop-filter` again fails CSS/runtime evidence; omitting `SourceGraphic` flattening fails real boundary ratios; square masks fail rounded-mask lifecycle evidence; a modal-sized-only mask fails the full-target assertion; missing cleanup/fallback fails lifecycle assertions.

## Recorded Result

- Runtime commit: `03ef5a3 fix(web): flatten content before regional overlay blur`.
- Real Library popup at `34px`: independent image-boundary energy `40.3865 -> 0.3793` (`0.94%` retained), DOM label energy `19.2672 -> 0.3324` (`1.73%` retained), boundary/interior ratio `1.1035`, outside-reference MAD `0`.
- `100%`, `125%`, and `150%` popup captures inferred physical scales `1`, `1.25`, and `1.5`; all image/text, graph, cleanup, and outside-reference checks passed.
- The shared production modal reported `fullTarget="true"`, `stdDeviation="34"`, inert surface/direct-backdrop filters, and a sharp foreground at all three scales.
- Task 4 RED trace: the first graph used a hidden surface at `0,-4,166.78125 x 190`; the final graph was `160,96,280 x 190`. The new gate removes every hidden/invalid active entry without changing outer mask geometry.
- Task 4 100% static outline result: historical reference and production surface have identical `548 x 274` outer rectangles; this proves same-element border/gradient ownership only. It did not register the surface in the regional filter and therefore did not prove filtered-plane containment; real left/right titlebar-popup evidence is added by `2026-08-10-overlay-regional-mask-edge-alignment.md`.
- Task 4 verification: focused lifecycle `9/9`, overlay contract `10/10`, `check:ui-design`, `check:web`, `check:desktop`, OS-backdrop desktop harness, `git diff --check`, and `npm test` all passed; Rust reported `479 passed, 0 failed, 2 ignored`.

# Overlay Frosted Material and Radius Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the two panel/control radius settings with one fresh 0–16 setting and give all shared popups and modals a single-layer, Settings-content-style frosted material.

**Architecture:** `interfaceStyle.ts` remains the single preference-to-CSS boundary. It emits one shared shape value and three semantic dialog pane colors. Shared overlay CSS keeps outer containers transparent and lets each surface's direct glass backdrop own one bounded frost pass; modal dimming is an outside-only spread shadow that cannot darken the dialog interior.

**Tech Stack:** React 19, TypeScript, CSS custom properties/backdrop-filter, Node test runner, Electron visual harness.

## Global Constraints

- `panelControlCornerRadius` has default `8`, range `0..16`, and step `1`.
- `surfaceCornerRadius`, `controlCornerRadius`, `panelRadius`, and `controlRadius` are ignored; there is no radius migration.
- `windowCornerRadius` and `contentCornerRadius` remain independent.
- Shared overlay outer containers stay transparent.
- Root/nested popups have one blur owner on their direct `.liquid-glass-backdrop`.
- Modal viewport containers have no fill or filter; the modal surface's direct backdrop is the one bounded blur owner.
- Modal dimming is an outside-only spread shadow and must not paint beneath the translucent dialog interior.
- Reduced-transparency fallback is opaque on the direct backdrop and disables filters.
- Preserve all pre-existing worktree changes and stage only files named by each task.

---

### Task 1: Unify panel and control radius behavior

**Files:**
- Modify: `apps/web/tests/interfaceStyle.test.mts`
- Modify: `apps/web/src/features/settings/interfaceStyle.ts`
- Modify: `apps/web/src/features/settings/settingsPanels.tsx`
- Modify: `apps/web/src/styles.css`
- Modify: `apps/web/src/design/tokens.ts`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`

**Interfaces:**
- Consumes: `normalizeInterfaceStyle(input: unknown)` and `interfaceStyleToCssVariables(value)`.
- Produces: `InterfaceStylePreference.panelControlCornerRadius: number`, `INTERFACE_STYLE_LIMITS.panelControlCornerRadius`, and the slider id `panel-control-corner-radius`.

- [ ] **Step 1: Write the failing radius behavior tests**

Extend `interfaceStyle.test.mts` imports with `INTERFACE_STYLE_LIMITS` and `normalizeInterfaceStyle`, then add literal behavior checks:

```ts
describe("unified panel and control radius", () => {
  test("starts from 8 and ignores every retired radius key", () => {
    const normalized = normalizeInterfaceStyle({
      surfaceCornerRadius: 16,
      controlCornerRadius: 14,
      panelRadius: 12,
      controlRadius: 10
    });
    assert.equal(normalized.panelControlCornerRadius, 8);
    assert.equal("surfaceCornerRadius" in normalized, false);
    assert.equal("controlCornerRadius" in normalized, false);
  });

  test("clamps the combined setting to 0 through 16", () => {
    assert.deepEqual(INTERFACE_STYLE_LIMITS.panelControlCornerRadius, {
      min: 0,
      max: 16,
      step: 1
    });
    assert.equal(normalizeInterfaceStyle({ panelControlCornerRadius: -1 }).panelControlCornerRadius, 0);
    assert.equal(normalizeInterfaceStyle({ panelControlCornerRadius: 20 }).panelControlCornerRadius, 16);
  });

  test("maps panels controls surfaces and overlays to the same radius", () => {
    for (const radius of [0, 8, 16]) {
      const variables = interfaceStyleToCssVariables({
        ...DEFAULT_INTERFACE_STYLE,
        panelControlCornerRadius: radius
      });
      for (const token of ["--radius-overlay", "--radius-panel", "--radius-surface", "--radius-control"]) {
        assert.equal(variables[token], `${radius}px`);
      }
      assert.equal(variables["--radius-tight"], `${Math.max(radius - 4, 0)}px`);
    }
  });
});
```

Update the UI-design validator so its required model/slider names are `panelControlCornerRadius` and `id="panel-control-corner-radius"`, it requires the label `面板与控件圆角`, and it rejects retired fields, slider ids, and undefined CSS aliases. Add static-default assertions for overlay/panel/surface/control `8px` and tight `4px`, plus representative consumer assertions that `.settings-section` uses `--radius-panel` and `.settings-action` uses `--radius-control`.

- [ ] **Step 2: Run the radius tests and validator to verify RED**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/interfaceStyle.test.mts
npm run check:ui-design
```

Expected: the Node test fails because `panelControlCornerRadius` and its limit do not exist; the validator fails because the new field and slider are absent.

- [ ] **Step 3: Implement the minimal unified preference**

In `interfaceStyle.ts`, replace both retired fields with:

```ts
panelControlCornerRadius: number;
```

Use this default and limit:

```ts
panelControlCornerRadius: 8
panelControlCornerRadius: { min: 0, max: 16, step: 1 }
```

Normalize only `source.panelControlCornerRadius`:

```ts
panelControlCornerRadius: clampNumber(
  source.panelControlCornerRadius,
  INTERFACE_STYLE_LIMITS.panelControlCornerRadius,
  DEFAULT_INTERFACE_STYLE.panelControlCornerRadius
)
```

Emit the same pixel value for overlay, panel, surface, and control; emit `max(value - 4, 0)` for tight. Do not read or copy a retired radius key.

Replace the two Settings sliders with:

```tsx
<StyleSlider
  id="panel-control-corner-radius"
  label="面板与控件圆角"
  max={limits.panelControlCornerRadius.max}
  min={limits.panelControlCornerRadius.min}
  onChange={(panelControlCornerRadius) => setInterfaceStyle({ panelControlCornerRadius })}
  step={limits.panelControlCornerRadius.step}
  unit="px"
  value={value.panelControlCornerRadius}
/>
```

Set the root fallback `--radius-overlay`, `--radius-panel`, `--radius-surface`, and `--radius-control` values to `8px`, with `--radius-tight: 4px`. Replace every `var(--control-corner-radius, ...)` with `var(--radius-control)` and `var(--surface-corner-radius, ...)` with `var(--radius-panel)`. Align `radiusTokens` to window/overlay/panel/surface/control/content values of `8`, tight `4`, and pill `999`.

Extend the tracked Settings evidence in `desktop-ui-regression.mjs` to set `#panel-control-corner-radius` to `0`, `8`, and `16`, wait for the preference CSS variable to synchronize, and record `getComputedStyle(document.querySelector(".settings-section")).borderRadius` plus `getComputedStyle(document.querySelector(".settings-action")).borderRadius`. The hard gate requires both consumers to report the selected pixel value at all three samples, then restores the original slider value.

- [ ] **Step 4: Run the focused checks to verify GREEN**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/interfaceStyle.test.mts
npm run check:ui-design
npm run check:web
```

Expected: all commands exit `0` and no retired radius identifier remains in `apps/web/src` or the UI-design validator.

- [ ] **Step 5: Commit the radius unit**

```powershell
git add apps/web/tests/interfaceStyle.test.mts apps/web/src/features/settings/interfaceStyle.ts apps/web/src/features/settings/settingsPanels.tsx apps/web/src/styles.css apps/web/src/design/tokens.ts tools/checks/validate-ui-design.mjs .tmp/visual-check/desktop-ui-regression.mjs
git commit -m "fix(web): unify panel and control radius"
```

---

### Task 2: Replace duplicate dark overlay paint with one content-pane material

**Files:**
- Modify: `apps/web/tests/interfaceStyle.test.mts`
- Modify: `apps/web/tests/overlayContract.test.mts`
- Modify: `apps/web/src/features/settings/interfaceStyle.ts`
- Modify: `apps/web/src/styles.css`
- Modify: `apps/web/src/design/overlays/overlay.css`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`

**Interfaces:**
- Consumes: center color plus existing dialog opacity, overlay-strength, blur, and backdrop-dim preferences.
- Produces: `--glass-dialog-pane-light`, `--glass-dialog-pane-mid`, `--glass-dialog-pane-end`, and the one-blur overlay CSS contract.

- [ ] **Step 1: Write failing dialog-token behavior tests**

Add literal expectations to `interfaceStyle.test.mts`:

```ts
describe("dialog content pane colors", () => {
  test("derives the Settings-content gradient from the center color", () => {
    const variables = interfaceStyleToCssVariables({ ...DEFAULT_INTERFACE_STYLE });
    assert.equal(variables["--glass-dialog-pane-light"], "rgb(52 56 59 / 0.2)");
    assert.equal(variables["--glass-dialog-pane-mid"], "rgb(8 12 16 / 0.38)");
    assert.equal(variables["--glass-dialog-pane-end"], "rgb(7 11 14 / 0.48)");
    assert.equal(variables["--glass-dialog-fill"], variables["--glass-dialog-pane-mid"]);
  });

  test("makes every dialog pane transparent when dialog opacity is zero", () => {
    const variables = interfaceStyleToCssVariables({
      ...DEFAULT_INTERFACE_STYLE,
      dialogOpacity: 0
    });
    for (const token of ["--glass-dialog-pane-light", "--glass-dialog-pane-mid", "--glass-dialog-pane-end"]) {
      assert.match(variables[token], / \/ 0\)$/);
    }
  });
});
```

- [ ] **Step 2: Write failing overlay ownership tests and validator rules**

Add a small CSS declaration-block helper to `overlayContract.test.mts`, then assert behavior of the declarative CSS artifact:

```ts
const cssDeclarationsForExactSelector = (source: string, selector: string) =>
  [...stripCssComments(source).matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter((match) => match[1].split(",").some((item) => item.trim() === selector))
    .map((match) => match[2]);
```

The new tests must require:

```ts
assert.match(directBackdrop, /--glass-dialog-pane-light/);
assert.match(directBackdrop, /blur\(var\(--glass-dialog-blur\)\)/);
assert.match(modalBackdrop, /background:\s*transparent/);
assert.match(modalBackdrop, /backdrop-filter:\s*none/);
assert.match(modalSurface, /100vmax\s+var\(--dialog-backdrop-dim\)/);
assert.match(modalSurfaceBackdrop, /blur\(var\(--glass-dialog-blur\)\)/);
```

For `.overlay-surface`, `.overlay-surface-popover`, `.overlay-surface-nested`, `.library-switcher`, `.library-switcher-header`, `.library-switcher-list`, `.library-switcher-footer`, `#library-remove-confirm`, `.context-menu`, and `.file-operation-dialog`, require a transparent background and reject `--glass-side-fill` or `--glass-dialog-fill` repainting. Update `validate-ui-design.mjs` with the same material ownership requirements, the three new semantic tokens, and removal of `--dialog-blur`.

- [ ] **Step 3: Run the material tests and validator to verify RED**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/interfaceStyle.test.mts apps/web/tests/overlayContract.test.mts
npm run check:ui-design
```

Expected: failures name the missing pane variables, opaque outer surfaces, sidebar-bound library switcher, and second modal-surface blur.

- [ ] **Step 4: Emit semantic dialog pane variables**

In `interfaceStyle.ts`, derive dialog colors with the same mix ratios as Settings:

```ts
const dialogPaneLightColor = mixHexColors(normalized.centerOverlayColor, "#ffffff", 0.18);
const dialogPaneEndColor = mixHexColors(normalized.centerOverlayColor, "#000000", 0.12);
const dialogPaneLight = colorWithAlpha(dialogPaneLightColor, 0.2 * normalized.dialogOverlayStrength * normalized.dialogOpacity);
const dialogPaneMid = colorWithAlpha(normalized.centerOverlayColor, 0.38 * normalized.dialogOverlayStrength * normalized.dialogOpacity);
const dialogPaneEnd = colorWithAlpha(dialogPaneEndColor, 0.48 * normalized.dialogOverlayStrength * normalized.dialogOpacity);
```

Return all three custom properties and map `--glass-dialog-fill` to `dialogPaneMid`. Add matching root fallback values in `styles.css`.

- [ ] **Step 5: Implement one material layer per depth**

In `overlay.css`:

```css
.overlay-surface,
.overlay-surface-popover,
.overlay-surface-nested {
  background: transparent;
}

.overlay-surface > .liquid-glass-backdrop {
  background: var(
    --overlay-solid-fallback-fill,
    linear-gradient(
      108deg,
      var(--glass-dialog-pane-light) 0%,
      var(--glass-dialog-pane-mid) 52%,
      var(--glass-dialog-pane-end) 100%
    )
  );
  -webkit-backdrop-filter: var(
    --overlay-solid-fallback-filter,
    blur(var(--glass-dialog-blur)) saturate(var(--glass-center-saturation))
  );
  backdrop-filter: var(
    --overlay-solid-fallback-filter,
    blur(var(--glass-dialog-blur)) saturate(var(--glass-center-saturation))
  );
}

.overlay-modal-backdrop {
  background: transparent;
  -webkit-backdrop-filter: none;
  backdrop-filter: none;
}

.overlay-modal-surface {
  box-shadow:
    0 0 0 100vmax var(--dialog-backdrop-dim),
    var(--glass-shadow);
}
```

The modal surface uses the same direct-backdrop gradient and blur rule as other overlays; do not add a second modal override. Remove side material overrides and material backgrounds from the feature surface classes listed in Step 2 while preserving borders, separators, layout, selected-state fills, and the new outside-only dim shadow.

Delete `--dialog-blur` from `styles.css`, `interfaceStyle.ts`, the validator, and the tracked appearance harness. In the harness, remove it from `groupVariables.dialog`; make the `dialog-blur` requirement observe `dialog.modal.effects.backdrop.blur` instead of `dialog.modalBackdrop.effects.elementBackdrop.blur`, and make the center-saturation requirement observe the modal surface backdrop rather than the unfiltered viewport container.

- [ ] **Step 6: Run focused tests and UI checks to verify GREEN**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/interfaceStyle.test.mts apps/web/tests/overlayContract.test.mts
npm run check:ui-design
npm run check:web
```

Expected: all commands exit `0`, normal overlays expose one filtered backdrop, and modal surfaces expose no second filter.

- [ ] **Step 7: Commit the material unit**

```powershell
git add apps/web/tests/interfaceStyle.test.mts apps/web/tests/overlayContract.test.mts apps/web/src/features/settings/interfaceStyle.ts apps/web/src/styles.css apps/web/src/design/overlays/overlay.css tools/checks/validate-ui-design.mjs .tmp/visual-check/desktop-ui-regression.mjs
git commit -m "fix(web): restore single-layer overlay frost"
```

---

### Task 3: Verify fallback, desktop composition, and repository health

**Files:**
- Verify: `.tmp/visual-check/desktop-ui-regression.mjs`
- Verify: every file changed in Tasks 1 and 2

**Interfaces:**
- Consumes: the production overlay DOM and CSS custom properties.
- Produces: fresh automated and real-window evidence for normal, modal, and solid-fallback modes.

- [ ] **Step 1: Confirm the existing harness observes the new ownership contract**

Read the tracked appearance-matrix and forced-fallback probes before running them. The existing evidence must expose outer `backgroundColor`/`backgroundImage`, direct-backdrop color/image/blur, modal-backdrop element blur, and forced-fallback outer/direct-backdrop alpha. Do not weaken its current opacity, readability, keyboard, focus, network, or native-acrylic gates. The focused overlay contract and UI-design validator are the durable automated guards for the exact one-blur topology; the harness supplies computed-style and screenshot evidence from production DOM.

- [ ] **Step 2: Run the complete nonvisual verification gate**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/interfaceStyle.test.mts apps/web/tests/overlayContract.test.mts
npm run check:ui-design
npm run check:web
npm run check:desktop
npm test
```

Expected: every command exits `0` with no new warning or failure attributed to this change.

- [ ] **Step 3: Run the explicit OS-backdrop harness**

Run:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'
node .tmp\visual-check\desktop-ui-regression.mjs
$taskHarnessCode = $LASTEXITCODE
Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
exit $taskHarnessCode
```

Expected: the harness exits `0`; captured computed styles show transparent outer popup surfaces, a filtered direct backdrop for normal popups, one filtered full-screen backdrop for modals, no filter on modal surface backdrops, and an opaque/no-filter direct backdrop in forced fallback.

- [ ] **Step 4: Inspect fresh screenshots and the physical window**

Check Settings, a titlebar popover, a context menu, the library switcher, Rename, and Move To. Confirm the popup material resembles the Settings right content pane, background detail remains visible through the frost, the modal interior is not covered by the outside dim, and the blur slider's maximum does not cause an interaction timeout. Treat the user's prior 100%/125%/150% geometry confirmation as pre-change evidence only; repeat normal popup, nested popup, Rename, and Move To checks at all three display scales, asking for the user's physical compositor confirmation where the harness cannot establish it.

- [ ] **Step 5: Request the same reviewer's final quality review**

Give the reviewer the approved spec, exact diff, RED/GREEN test evidence, final commands, and screenshot archive. Resolve every blocking finding, then rerun affected checks.

- [ ] **Step 6: Confirm worktree integrity**

Run:

```powershell
git status --short
git diff --cached --name-only
git diff --check
```

Expected: all pre-existing modified/untracked paths remain present, no unrelated file is staged, and `git diff --check` reports no whitespace errors in this task's diff.

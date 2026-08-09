# Overlay Frosted Material and Radius Unification Design

**Status:** Approved for implementation on 2026-08-09

## 1. Goal

Unify the Settings panel/control radius into one fresh preference and make every shared popup or dialog use the lighter, right-content-area frosted material already established by the Settings window. Rename, Move To, and other modal dialogs must blur the application exactly once instead of stacking dark fills and blur planes.

## 2. Confirmed Product Decisions

- The new combined radius is `panelControlCornerRadius`.
- Its range is `0..16`, its step is `1`, and its default is `8`.
- Existing `surfaceCornerRadius`, `controlCornerRadius`, `panelRadius`, and `controlRadius` values are intentionally ignored. There is no radius migration.
- `windowCornerRadius` and `contentCornerRadius` remain separate settings.
- Shared overlays use the Settings right-content-area material, not the Settings sidebar material.
- Rename, Move To, and other renderer modals remain DOM overlays. They do not become native `BrowserWindow` instances.

This document supersedes the independent `surfaceCornerRadius` and `controlCornerRadius` entries in section 7.1 of `2026-05-23-megle-desktop-workbench-shell-rearchitecture-design.md`.

## 3. Root Cause

The shared `OverlaySurface` already renders a direct `.liquid-glass-backdrop` with a real `backdrop-filter`. The apparent lack of frost comes from painting an opaque-looking dialog fill twice: once on the outer overlay container and again on the inner backdrop. With the default alpha values, those two layers combine into a much darker surface that hides the texture the blur sampled.

Modal dialogs add a second problem. The full-screen modal backdrop currently applies dimming and blur, while the nested dialog surface applies another dark fill and another blur. Rename and Move To therefore render through two blur planes and three dark layers, producing a muddy result instead of the Settings window's clear acrylic hierarchy.

The Settings window is the working reference: a transparent outer shell, one material backdrop, a dark navigation pane, and a lighter 108-degree content gradient.

## 4. Radius Model

`InterfaceStylePreference` and `INTERFACE_STYLE_LIMITS` expose only `panelControlCornerRadius` for panel/control shape. Normalization reads only that new key. If it is absent, normalization returns `8` even when any retired radius key is present. Other interface-style preferences continue to normalize normally.

The combined value drives these CSS variables directly:

- `--radius-overlay`
- `--radius-panel`
- `--radius-surface`
- `--radius-control`

`--radius-tight` remains the only smaller derivative and is `max(panelControlCornerRadius - 4, 0)`. Window, content, and pill radii retain their independent contracts.

The Settings UI contains one slider with id `panel-control-corner-radius` and label `面板与控件圆角`. The retired panel and control sliders disappear. Settings components must consume the formal `--radius-panel` and `--radius-control` tokens; the undefined `--surface-corner-radius` and `--control-corner-radius` aliases are removed.

The static `:root` defaults are part of the same contract. Before JavaScript hydration or when preference application fails, `--radius-overlay`, `--radius-panel`, `--radius-surface`, and `--radius-control` are all `8px`, while `--radius-tight` is `4px`.

## 5. Dialog Material Tokens

The dialog material is derived from `centerOverlayColor` so it follows the central/right content family while retaining the existing dialog opacity, overlay-strength, blur, and backdrop-dim controls.

Three semantic pane tokens mirror the Settings content gradient:

- `--glass-dialog-pane-light`: center color mixed 18% toward white, using base alpha `0.20`.
- `--glass-dialog-pane-mid`: the center color, using base alpha `0.38`.
- `--glass-dialog-pane-end`: center color mixed 12% toward black, using base alpha `0.48`.

Each alpha is multiplied by `dialogOverlayStrength * dialogOpacity` and clamped by the existing color helper. `--glass-dialog-fill` remains as a compatibility alias of the mid token. `--glass-dialog-blur` keeps the current dialog blur setting and is the only dialog blur variable used by overlay CSS. The retired `--dialog-blur` variable is removed from static defaults, runtime emission, validators, and the desktop appearance harness.

The static `:root` defaults include all three pane tokens with the same default colors and alphas as the runtime model, so pre-hydration and fixture rendering use the same material.

The rendered material is:

```css
linear-gradient(
  108deg,
  var(--glass-dialog-pane-light) 0%,
  var(--glass-dialog-pane-mid) 52%,
  var(--glass-dialog-pane-end) 100%
)
```

## 6. One Material Layer per Visual Depth

### 6.1 Root and nested popups

The outer `.overlay-surface`, `.overlay-surface-popover`, and `.overlay-surface-nested` containers are transparent. Their direct `.liquid-glass-backdrop` owns the dialog gradient and `--glass-dialog-blur`. Feature classes such as `.context-menu`, `.library-switcher`, and their headers, lists, footers, or confirmation surfaces may lay out and separate content, but must not repaint sidebar or dialog material behind the shared backdrop.

### 6.2 Modal dialogs

`.overlay-modal-backdrop` remains transparent and has no backdrop filter; it owns only viewport layout, pointer capture, and open/close animation. The modal surface paints a large spread shadow using `--dialog-backdrop-dim`, so the application is dimmed outside the dialog border while the dialog interior remains free of that dark layer.

The modal surface's direct `.liquid-glass-backdrop` owns the same right-content gradient and one `--glass-dialog-blur` pass as a normal popup. This keeps the blur area bounded to the dialog instead of expanding the dialog slider's default 34px and maximum 68px radius across the full viewport. Rename, Move To, library removal confirmation, and all other `ModalSurface` consumers inherit this topology without feature JSX changes.

The outside-only dim is implemented with a spread shadow on `.overlay-modal-surface`, equivalent to `0 0 0 100vmax var(--dialog-backdrop-dim)`, followed by the normal glass elevation shadow. A full-screen filtered pseudo-element, mask, or dark background beneath the translucent dialog is not permitted because it would either reintroduce a second blur plane or contaminate the dialog material.

### 6.3 Reduced-transparency and unsupported-filter fallback

The outer surface and modal viewport container remain transparent in every mode. When transparency is reduced, unsupported, or force-disabled by the visual harness, the direct backdrop becomes `rgba(18, 24, 30, 0.98)` and all overlay filters become `none`. The outside-only modal dim shadow remains available for hierarchy, while no blur is attempted.

## 7. Preserved Behavior

- Overlay stack ownership, focus trapping, Escape/outside-pointer dismissal, portals, z-index levels, and open/close motion remain unchanged.
- The global pointer-highlight system remains the only hover system.
- Settings and Plugins keep native Windows acrylic; no Electron window lifecycle changes are required.
- Media grids, preview canvases, Core APIs, and filesystem operations are outside this change.

## 8. Verification Contract

Automated coverage must prove:

- missing or retired radius keys normalize to the new default `8`;
- values clamp to `0..16` and map all four shared radius variables to the same value;
- static `:root` defaults expose those four shared radii as `8px`, tight radius as `4px`, and all three dialog pane tokens before runtime preference application;
- the Settings UI exposes exactly one combined slider;
- retired preference keys, retired slider ids, and the undefined `--surface-corner-radius` / `--control-corner-radius` aliases are absent from production source;
- representative Settings panel and control elements report `0px`, `8px`, and `16px` computed radii when the combined slider is exercised;
- dialog pane colors follow the declared light/mid/end derivation and become transparent when dialog opacity or strength is zero;
- outer overlay and feature surfaces are transparent;
- a normal popup has one direct-backdrop blur owner;
- a modal viewport has no fill or filter, its surface backdrop is the sole blur owner, and its dim shadow does not paint inside the dialog border;
- the modal center remains within the same luminance family as a normal popup and preserves visibly blurred background texture instead of collapsing to a flat dark fill;
- solid fallback is opaque on the direct backdrop and disables every filter without repainting the outer surface.

Required final checks are the focused interface/overlay tests, `npm run check:ui-design`, `npm run check:web`, `npm run check:desktop`, `npm test`, and the OS-backdrop desktop visual harness. The harness must exercise the dialog blur slider at its maximum and report no interaction timeout or material-topology regression. The new build must be checked at 100%, 125%, and 150% display scale for a normal popup, a nested popup, Rename, and Move To. The real window must be inspected for material layering because CDP screenshots cannot validate physical compositor anti-aliasing.

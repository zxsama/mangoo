# Overlay Frosted Material and Radius Unification Design

**Status:** Revised after physical-window validation on 2026-08-10; written revision pending user review

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

The first implementation removed duplicate dark fills, but physical-window validation showed that its blur owner was still wrong. `OverlaySurface` placed `backdrop-filter` on a direct `.liquid-glass-backdrop` child. That child sits inside a `LiquidGlassSurface`, while root, nested, and modal opening animations temporarily apply `opacity < 1` to the surface or one of its ancestors. Under the CSS Backdrop Root rules, an ancestor with partial opacity bounds the pixels available to descendant backdrop filters. The child can therefore receive an empty transparent intermediate surface even though its computed `backdrop-filter` reports `blur(34px)`.

Computed-style evidence was insufficient because it proved only that Chromium accepted the property, not that the filtered input contained application pixels. The previous fixture also lacked a high-frequency background that would make successful blur measurable.

The Settings window is not evidence that the child-backdrop topology works. Settings is a separate Electron `BrowserWindow` with native Windows acrylic applied to the whole window. Its right content pane only paints the lighter 108-degree translucent gradient over that already-blurred native material. Renderer popups remain DOM elements in the main window, so they need in-app acrylic: the popup element itself must sample already-painted application content with CSS `backdrop-filter`.

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

The portal-mounted `.overlay-surface` element is the only in-app acrylic owner. It paints the dialog gradient and applies `blur(var(--glass-dialog-blur)) saturate(var(--glass-center-saturation))` to its own backdrop. Its direct `.liquid-glass-backdrop` child is transparent and has no backdrop filter, so the generic Liquid Glass DOM structure cannot introduce a nested filtering pass.

Root and nested popup opacity/transform motion may remain on the surface itself. The filtered backdrop is produced by that same element before the element's final opacity is composited; there is no partially opaque ancestor between the portal target and the blur owner. Feature classes such as `.context-menu`, `.library-switcher`, and their headers, lists, footers, or confirmation surfaces may lay out and separate content, but must not repaint sidebar or dialog material.

### 6.2 Modal dialogs

`.overlay-modal-backdrop` remains transparent and has no backdrop filter. It owns only viewport layout and pointer capture; it must not animate opacity because that would create a transient Backdrop Root above the dialog. Modal entrance/exit motion stays on `.overlay-modal-surface` itself.

The modal surface itself owns the same right-content gradient and one `--glass-dialog-blur` pass as a normal popup. Its direct `.liquid-glass-backdrop` is inert. This keeps the blur area bounded to the dialog instead of expanding the dialog slider's default 34px and maximum 68px radius across the full viewport. Rename, Move To, library removal confirmation, and all other `ModalSurface` consumers inherit this topology without feature JSX changes.

The outside-only dim is implemented with a spread shadow on `.overlay-modal-surface`, equivalent to `0 0 0 100vmax var(--dialog-backdrop-dim)`, followed by the normal glass elevation shadow. A full-screen filtered pseudo-element, mask, or dark background beneath the translucent dialog is not permitted because it would either reintroduce a second blur plane or contaminate the dialog material.

### 6.3 Reduced-transparency and unsupported-filter fallback

The modal viewport container remains transparent in every mode. When transparency is reduced, unsupported, or force-disabled by the visual harness, the overlay surface itself becomes `rgba(18, 24, 30, 0.98)` and its backdrop filter becomes `none`; the direct backdrop child remains inert. The outside-only modal dim shadow remains available for hierarchy, while no blur is attempted.

## 7. Preserved Behavior

- Overlay stack ownership, focus trapping, Escape/outside-pointer dismissal, portals, and z-index levels remain unchanged. Root/nested and modal-surface motion remain; only the modal viewport opacity animation is removed.
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
- feature-specific inner surfaces do not repaint dialog or sidebar material;
- a normal popup surface is the sole gradient and blur owner, while its direct backdrop child is transparent and unfiltered;
- a modal viewport has no fill, filter, or opacity animation; its modal surface is the sole blur owner, and its dim shadow does not paint inside the dialog border;
- the modal center remains within the same luminance family as a normal popup and preserves visibly blurred background texture instead of collapsing to a flat dark fill;
- solid fallback is opaque on the overlay surface and disables every filter without reactivating the direct backdrop child;
- a deterministic visual fixture places a high-frequency checkerboard and fine text behind the popup, captures zero-blur and configured-blur states, and verifies that edge energy inside the popup decreases while the outside reference crop remains stable.

Required final checks are the focused interface/overlay tests, `npm run check:ui-design`, `npm run check:web`, `npm run check:desktop`, `npm test`, and the OS-backdrop desktop visual harness. The harness must exercise zero, default, and maximum dialog blur against the patterned fixture, report a real pixel-level blur delta, and show no interaction timeout or material-topology regression. The new build must be checked at 100%, 125%, and 150% display scale for a normal popup, a nested popup, Rename, and Move To. The real window must be inspected for material layering because CDP screenshots cannot validate physical compositor anti-aliasing.

# Overlay Frosted Material and Radius Unification Design

**Status:** Implemented; regional-mask edge containment is superseded by `2026-08-10-overlay-regional-mask-edge-alignment-design.md` after physical 100% review found the filtered plane extending beyond the frame

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

The first revision fixed duplicate fills and moved `backdrop-filter` to the portal-mounted surface. That made the blur property active, but a real-window probe exposed a second, independent compositor failure that the patterned fixture did not model.

The CSS Filter Effects model requires a backdrop to be painted in order, flattened into one 2D screen-space buffer, and only then filtered. Electron 43/Chromium does not produce that unified input for the current real Library scene. Replaced `<img>` elements, clipped thumbnail boxes, and text paint/compositor chunks retain their individual box boundaries. Fine detail inside each box is blurred, but the box edges and text silhouettes are composited back as distinct boundaries. Raising the declared blur from `34px` to `52px` does not correct the missing flattening step.

The real notification-popover crop measured the defect directly. At the default `34px`, mean thumbnail-boundary gradient was `11.194` while interior texture gradient was `0.818`, a boundary/interior ratio of `13.69`. At `52px` the boundary remained `12.433`. In contrast, applying an ordinary `filter: blur(34px)` to the common grid ancestor first flattened its complete `SourceGraphic`; boundary gradient fell to `0.296`, text and rounded image edges became unreadable, and the ratio fell to `1.55`.

Full-viewport `backdrop-filter`, overscan, moving the property between the surface and its child, removing isolation and outer clips, `translateZ(0)`, `will-change`, near-identity filters, and disabling thumbnail fade animation were all falsified in the same real scene. The old fixture looked correct because its checkerboard and repeated text were primarily one CSS background paint layer. Computed style and a single-layer fixture therefore cannot be completion evidence.

Settings remains a separate Electron `BrowserWindow` whose native Windows acrylic operates on a unified window surface. Renderer overlays stay DOM overlays, but they must reproduce the same ordering explicitly: flatten the already-painted renderer scene with an ordinary SVG filter on the common content ancestor, create the Gaussian-blurred result from that `SourceGraphic`, and composite the blurred result only inside active overlay regions. No screenshot capture or native child popup window is required.

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

## 6. Regional Flatten-Then-Blur Material

### 6.1 Root and nested popups

`OverlayProvider` owns one regional-blur controller for the main renderer. It consumes the already-authoritative ordered `OverlayLayerRegistration[]`; feature overlays do not implement material capture themselves.

For every visible, laid-out root or nested overlay, the controller reads the surface border box and computed radius in CSS pixels. A connected surface is not sufficient: `visibility:hidden`, `display:none`, non-finite geometry, and a zero-sized border box are excluded. This prevents an `AnchoredOverlay` from contributing its temporary pre-layout `left:0; top:0` box to the regional graph. The first active filter for a newly opened popup must therefore already reference its final visible geometry. It applies an SVG filter to `#root` with the following fixed pipeline:

1. use the target's complete `SourceGraphic`, which forces the renderer content into one offscreen image;
2. run `feGaussianBlur` with `stdDeviation` equal to `--glass-dialog-blur` and `edgeMode="duplicate"`;
3. apply the current `--glass-center-saturation` to the blurred result;
4. build each rounded alpha mask synchronously inside the filter graph from `feFlood`, a small mask-only Gaussian, and an alpha-threshold `feComponentTransfer`;
5. merge the blurred image inside that mask with the untouched `SourceGraphic` outside it.

The Gaussian primitive subregion is the union of active masks expanded by `3 * stdDeviation` and clipped to the target bounds. The common source still flattens as one image, but the expensive blur kernel does not run across unrelated viewport pixels. Filter and primitive units are `userSpaceOnUse`, so geometry remains in CSS pixels at 100%, 125%, and 150% display scale.

Nested overlays preserve visual depth. `#root` receives masks for every active overlay. Each lower overlay surface receives a separate regional filter only for intersections with layers above it; the top surface remains unfiltered and sharp. This makes a nested menu blur both the application and the root popup beneath it without blurring its own content.

The portal-mounted `.overlay-surface` paints only the approved 108-degree dialog gradient, border, dither, and elevation. Its frame reproduces the exact successful `backdrop` probe from `.tmp/regional-filter-probe`: the gradient and `1px solid rgba(255, 255, 255, 0.3)` border are painted by the same surface element. The generic `.liquid-glass::after` plane must not paint a second border or inset edge shadow on overlay surfaces. The regional rounded mask continues to use the complete outer border box and computed outer radius; it is not inset to the padding box. Both surface backdrop-filter declarations are `none`, and its direct `.liquid-glass-backdrop` child stays transparent and unfiltered. Feature classes such as `.context-menu`, `.library-switcher`, and their headers, lists, footers, or confirmation surfaces may lay out and separate content, but must not repaint sidebar or dialog material.

Stack changes and direct surface/root mutations rebuild the self-contained filter graph synchronously during the same layout or mutation phase, so a removed modal/nested mask cannot survive into the next painted frame. Window resize and captured scroll are event-driven and coalesced into one animation frame. The graph has no `feImage`, data URL, or image-decode dependency. Closing the final layer removes generated SVG definitions and restores every target's prior inline `filter` value.

### 6.2 Modal dialogs

`.overlay-modal-backdrop` remains transparent and unfiltered. It owns only viewport layout and pointer capture; it must not animate opacity. Modal entrance/exit motion stays on `.overlay-modal-surface` itself.

A modal registration changes the regional mask to the full viewport for every lower target. Rename, Move To, library removal confirmation, and all other `ModalSurface` consumers therefore receive one uniform, live blur of the complete renderer scene behind the modal, while the dialog itself stays sharp and paints the same right-content gradient as a normal popup. The modal surface and its direct backdrop child have no backdrop filter.

The outside-only dim remains a spread shadow on `.overlay-modal-surface`, equivalent to `0 0 0 100vmax var(--dialog-backdrop-dim)`, followed by the normal glass elevation shadow. The dim is composited above the one regionally blurred content image and never becomes another blur owner.

### 6.3 Reduced-transparency and unsupported-filter fallback

The modal viewport remains transparent in every mode. When transparency is reduced, the SVG filter path is unavailable, `--glass-dialog-blur` is zero, or fallback is force-enabled by the visual harness, the controller removes all target filters. The overlay surface itself becomes `rgba(18, 24, 30, 0.98)` for fallback modes; the direct backdrop child remains inert. The outside-only modal dim remains available for hierarchy.

## 7. Preserved Behavior

- Overlay stack ownership, focus trapping, Escape/outside-pointer dismissal, portals, and z-index levels remain unchanged. Root/nested and modal-surface motion remain; only the modal viewport opacity animation is removed.
- The global pointer-highlight system remains the only hover system.
- Settings and Plugins keep native Windows acrylic; no Electron window lifecycle or capture IPC is added.
- Media grids and preview canvases are only transient `SourceGraphic` inputs while an overlay is open; their layout, interaction, data, Core APIs, and filesystem operations do not change.

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
- a normal popup surface is the sole gradient owner and has no backdrop filter, while its direct backdrop child is transparent and unfiltered;
- the mounted popup reports a real `1px solid rgba(255, 255, 255, 0.3)` border on the same element as the gradient, while its `::after` border and inset edge shadow are absent;
- the provider builds one root regional filter from the ordered overlay stack, using `SourceGraphic`, Gaussian blur, saturation, rounded masks, and untouched output outside each mask;
- the rounded mask graph is synchronous, does not decode an image resource, follows stack pops and geometry changes before the next painted frame, and never becomes active for a hidden pre-layout surface at the viewport origin;
- every popup mask rectangle remains equal to the surface's complete outer border box; no border-width or padding-box inset is permitted;
- a nested layer adds its rounded mask to the app root and applies only its intersection mask to every lower overlay surface;
- a modal viewport has no fill, filter, or opacity animation; a modal registration expands lower-target masks to the full viewport, and its dim shadow does not paint inside the dialog border;
- the modal center remains within the same luminance family as a normal popup and preserves visibly blurred background texture instead of collapsing to a flat dark fill;
- solid fallback is opaque on the overlay surface and disables every filter without reactivating the direct backdrop child;
- a deterministic visual fixture places independent replaced `<img>` elements, rounded clips, gaps, and fine text behind a real registered popup; zero/default/maximum captures verify that both interior texture and element-boundary gradients decrease, the boundary/interior ratio no longer exposes box edges, and an outside reference crop remains stable;
- a real Library portal-popover crop repeats the same boundary measurement so a single-layer fixture cannot hide a compositor regression;
- generated filter definitions and inline target filters are removed after the final overlay closes, reduced-transparency/fallback disables them, and the maximum blur path does not leak observers or animation frames.

Required final checks are the focused interface/overlay tests, `npm run check:ui-design`, `npm run check:web`, `npm run check:desktop`, `npm test`, and the OS-backdrop desktop visual harness. The harness must exercise zero, default, and maximum dialog blur against the independent-image fixture and the real Library portal crop, report real boundary/interior pixel deltas, and show no interaction timeout or material-topology regression. It must use device-scale-only `1`, `1.25`, and `1.5` captures for a real registered popup and the shared production `ModalSurface`; mounted file-operation coverage must prove Rename and Move To consume that primitive, and the lifecycle fixture must prove nested lower-surface and full-target modal topology. Independently of that scale matrix, the static `1.0` capture is a hard regression only for same-element surface-frame and border ownership. Regional-filter alpha containment for real left/right titlebar popups is governed by `2026-08-10-overlay-regional-mask-edge-alignment-design.md`. The real window must still be inspected by the user for final material layering because CDP screenshots cannot validate physical compositor anti-aliasing.

The recorded real-Library default result is image-boundary energy `40.3865 -> 0.3793` (`0.94%` retained), DOM-label energy `19.2672 -> 0.3324` (`1.73%` retained), boundary/interior ratio `1.1035`, and outside-reference MAD `0`. The three scale captures inferred `1`, `1.25`, and `1.5` physical output and all shared-modal captures reported a full-target `34px` Gaussian behind a sharp dialog surface.

The follow-up first-frame trace recorded the defect before implementation as an active hidden surface at `left:0`, `top:-4`, `166.78125 x 190`, followed by the correct `160,96,280 x 190` graph. After the visibility/geometry gate, every active trace entry is visible, finite, positive, and references a connected filter; every source mask remains equal to the complete un-inset outer border box. The dedicated static 100% frame capture reports identical same-element border and gradient rasterization, but it does not cover the separately rendered regional-filter alpha footprint. Real left/right titlebar-popup containment is specified by `2026-08-10-overlay-regional-mask-edge-alignment-design.md`.

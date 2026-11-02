# Mangoo Desktop Workbench Shell Rearchitecture Design

Updated: 2026-05-23

## 1. Background

Mangoo's desktop shell has reached the point where local CSS and component-level fixes are no longer sufficient. The current issues are structural:

- titlebar-local halo and outline refresh still depends too much on direct hover targets instead of a unified titlebar interaction plane
- left, center, and right regions still partially behave like separately bordered surfaces
- startup still exposes a lower rectangular host layer under the renderer shell
- dialogs can become transparent overlays without a real frosted backdrop/material stack
- settings currently expose mostly global glass tokens instead of the actual desktop shell material model

This design replaces patch-driven shell fixes with a renderer-owned desktop shell architecture. The native desktop window becomes a transparent host only. All visible chrome, glass, radius, outlines, joins, and overlay materials are owned by the renderer and follow one unified contract.

This spec is desktop-first. Browser behavior is not the target of this phase except where shared renderer primitives make that unavoidable.

## 2. Goals

1. Make the desktop window visually owned by one renderer shell with one outer radius, one outer outline, and one material system.
2. Turn the left, center, and right columns into three continuous structural glass surfaces from titlebar through body, with the center column using a distinct material family.
3. Replace direct-hover titlebar behavior with a unified titlebar interaction plane that supports local outline refresh, local halo refresh, dragging, and double-click maximize/restore.
4. Restrict local background halo to interactive surfaces only, rather than letting non-interactive structural backgrounds glow around the pointer.
5. Expose the real desktop shell parameters in Settings, including shared shape, side shell material, center workbench material, shared liquid-glass interaction, and dialog material.
6. Remove titlebar-to-pane seams and double borders at all three column joins.
7. Eliminate the startup rectangular backing plate and make transparency valid from the first visible frame.
8. Make dialogs use an actual frosted material stack with backdrop blur over the underlying UI.

## 3. Non-Goals

- Do not redesign library browsing, plugin execution, indexing, file operations, or task execution logic.
- Do not revert back to a native-looking OS titlebar or depend on Electron `app-region: drag` as the primary interaction model.
- Do not add a global opaque or semi-opaque rectangle to hide transparency defects.
- Do not turn the center preview image into a framed card.
- Do not introduce decorative diagonal gradients from top-left to bottom-right across shell surfaces.
- Do not expose structural rules such as join ownership or separator ownership as user settings.

## 4. Approved Direction

The approved direction for this phase is:

- transparent native root window
- renderer-owned workbench shell
- titlebar halo zone supports both local refresh and window dragging
- center column is also a continuous glass workbench surface, not a non-glass void
- left/right shell material and center workbench material are exposed separately in settings
- search field keeps a single outer stroke only, with no inner stroke and no internal local halo

This spec supersedes older assumptions from earlier shell documents where the center stage was treated as a non-structural glass area or where titlebar controls were still modeled as independently hovered controls.

## 5. Architecture

### 5.1 Native Window Host

The Electron window becomes a transparent carrier only.

Responsibilities:

- create the native window with transparent background behavior
- own lifecycle timing for first show
- bridge window controls such as minimize, maximize/restore, close, bounds, move, and resize

Non-responsibilities:

- no visible gray backing plate
- no visible host-owned glass
- no visible host-owned titlebar styling
- no separate host-level rounded rectangle beneath the renderer shell

### 5.2 Workbench Shell Root

The renderer owns a single top-level `workbench-shell-root`.

This root is the only owner of:

- window-level outer radius
- window-level outer outline
- window-level shell shadow
- global structural clipping

No child pane, titlebar, dialog, or overlay may redefine another window-scale outline/radius that competes with the shell root.

### 5.3 Three Continuous Structural Surfaces

The visible workbench consists of three continuous structural surfaces:

- left side shell surface
- center workbench surface
- right side shell surface

Each surface starts at the top edge of the window and continues through its full column. A titlebar region and the pane beneath it must read as one continuous surface, not two stacked cards.

The center column remains a glass surface, but uses a separate parameter family from the side shells. It should be visually quieter and less bright than the side shells so content remains primary.

### 5.4 Sharp Content Layers

Media content remains sharp and sits above the structural shell surfaces:

- library thumbnails
- folder/file items
- center image viewer
- right preview media

Content can use local content radii where appropriate, but it must not reintroduce structural framing around the entire center workbench.

### 5.5 Overlay Material Stack

All overlays participate in the same shell material system, with three overlay classes:

- compact popovers
- modal dialogs
- heavier full overlays if introduced later

They share one overlay host and one material vocabulary. Individual dialogs or popovers must not invent their own transparency model.

## 6. Titlebar Interaction Plane

### 6.1 Unified Pointer Field

The titlebar owns one shared pointer field rather than letting each control wait for direct hover.

Tracked state includes at least:

- pointer x/y in titlebar space
- inside/outside titlebar
- current nearest-control relationship
- active drag state

Every titlebar-interactive control reads from this shared field to compute local response.

### 6.2 Interaction Zones

The titlebar is divided into three interaction zones:

- control zone: true hit targets such as buttons, search field, menu triggers
- halo zone: non-clickable buffer around controls that still refreshes nearby local response
- drag zone: remaining blank titlebar space used for window dragging and double-click maximize/restore

The missing structural concept in the current implementation is the halo zone. This zone must exist explicitly rather than accidentally falling into either hover-only or drag-only behavior.

### 6.3 Event Routing

`pointermove`

- update titlebar pointer field first
- compute nearby controls from the field
- refresh local outline/highlight for the nearest relevant controls
- if dragging is active, forward movement to window positioning

`pointerdown`

- inside control zone: hand off to the control
- inside halo zone: allow local refresh and permit drag-start behavior if the gesture becomes a drag
- inside drag zone: begin drag immediately

`dblclick`

- if the target is not a true control hit target, toggle maximize/restore

`pointerleave`

- reset the titlebar pointer field only when leaving the titlebar plane itself, not when leaving an individual control

### 6.4 Control Coverage

The same pointer field must drive all titlebar controls:

- primary navigation tabs if they reach the top edge
- library toolbar buttons
- preview toolbar buttons
- search field
- tasks and recent triggers
- minimize, maximize/restore, close

There must not be one system for middle controls and another system for right-side window controls.

### 6.5 Search Field Rule

The search field is a special control, not a generic dual-layer glass button.

Rules:

- one outer outline stroke only
- no inner outline
- no inner local outline sweep
- no inner local halo
- local response is limited to the outer contour driven by the shared titlebar pointer field
- text input activates only on true input hit interaction, not on halo proximity

## 7. Material Parameter Model

Settings must expose stable, global shell parameters grouped by responsibility, not by component accident.

### 7.1 Shared Shape

- `windowCornerRadius`
- `surfaceCornerRadius`
- `controlCornerRadius`
- `contentCornerRadius`

These define the radius hierarchy for the whole desktop shell. Components must consume these layers rather than declaring ad hoc radii.

### 7.2 Side Shell Material

Shared by left and right structural side surfaces by default:

- `blur`
- `opacity`
- `overlayStrength`
- `overlayColor`
- `saturation`
- `strokeOpacity`

Left and right may later diverge if there is a justified product need, but this phase uses one side-material family for both.

### 7.3 Center Workbench Material

Dedicated to the center structural glass workbench surface:

- `blur`
- `opacity`
- `overlayStrength`
- `overlayColor`
- `saturation`
- `strokeOpacity`

This family is independent from side shells and is expected to render darker, calmer, and less attention-seeking than the side shells.

### 7.4 Shared Liquid Glass Interaction

- `edgeHighlightBrightness`
- `edgeHighlightSize`
- `haloBrightness`
- `haloFalloff`
- `pointerResponseRadius`
- `refractionStrength`

This group controls local liquid response behavior, not structural material color.

### 7.5 Dialog Material

- `dialogBlur`
- `dialogOpacity`
- `dialogOverlayStrength`
- `dialogBackdropDim`

Dialogs need a separate material family because their surface/backdrop semantics differ from the structural shell.

## 8. Surface Composition Rules

### 8.1 Join Ownership

Titlebar-to-pane joins must not let both sides draw their own boundary.

For each column:

- top and body can be one DOM surface, or
- top and body can be sublayers under one stroke owner

The visible result must be identical:

- no horizontal seam
- no color break
- no double outline
- no blur discontinuity

### 8.2 Column Boundary Ownership

If a column separator is needed, it can only come from one source.

Forbidden outcomes:

- left surface right border plus center surface left border stacking into a thick line
- top separator and body separator defined independently
- left and right titlebar seams that do not match the body seam rules

All column boundaries must be defined once and shared from top to bottom.

### 8.3 Interactive Background Rule

Local background halo is allowed only on interactive surfaces at their lowest background layer.

Allowed:

- buttons
- input fields
- search field outer contour
- sliders
- titlebar interactive controls
- file/folder items
- thumbnail items
- menu items

Disallowed:

- empty structural pane background
- non-interactive container plates
- center image viewing whitespace
- purely decorative shell surfaces

The user should perceive liquid response from nearby interactive affordances, not from generic background chrome.

### 8.4 Content Framing Rule

The center full-image viewing state must not add a uniform frame around the image.

Rules:

- no stage border
- no stage card
- no inset frame padding whose primary visual effect is framing
- no structural image wrapper that competes with the center workbench surface

The content stage can still manage zoom, pan, and fit behavior, but it cannot solve layout clarity by drawing another card around the media.

### 8.5 Gradient Rule

Shell structural surfaces must not use a diagonal top-left to bottom-right gradient as a default material treatment.

Any tonal modeling must come from the approved liquid-glass material stack, not from a broad decorative directional gradient.

## 9. Startup Transparency Contract

### 9.1 Transparent First Frame

Transparency must be valid before the window becomes visible.

From the first visible frame:

- native host background is transparent
- `html` is transparent
- `body` is transparent
- `#root` is transparent
- renderer mount shell is transparent until the renderer-owned shell surface paints

The first visible filled shape should be the renderer `workbench-shell-root`, not a browser default rectangle or native fallback rectangle.

### 9.2 Show Timing

Window display becomes a two-stage handshake:

1. native host ready
2. renderer shell ready

The desktop window should only be shown after the renderer explicitly signals shell readiness. This prevents the host rectangle from appearing before the renderer shell exists.

### 9.3 Single Radius Contract

Any visible host-level clipping and renderer shell clipping must share the same radius contract.

The user must not perceive:

- an upper rounded shell over a lower square host
- mismatched corner arcs between native host and renderer shell
- an extra lower layer extending beyond the shell root

If Windows composition forces a lower visible carrier layer, that layer must be clipped to the same radius as the shell root.

## 10. Dialog And Overlay Material Stack

### 10.1 Backdrop Layer

Modal dialogs open with a dedicated backdrop layer.

Backdrop responsibilities:

- blur underlying UI
- lightly dim underlying UI
- intercept outside pointer interaction
- isolate scroll/focus ownership

The backdrop is not a transparent empty spacer.

### 10.2 Dialog Surface Layer

Above the backdrop sits the actual dialog surface.

Dialog surface responsibilities:

- dialog-local blur
- dialog-local opacity
- dialog-local overlay color
- dialog-local outline
- dialog-local shadow
- dialog-local radius

This ensures dialogs appear as frosted semi-transparent glass, not free-floating transparent text.

### 10.3 Overlay Discipline

Overlay classes must share one host and one material family:

- compact popovers: tasks, recent, filter, sort
- modal dialogs: rename, move, delete confirm
- future heavier overlay panels if needed

Differences between these overlays should come from parameter presets and interaction semantics, not from separate ad hoc transparency implementations.

## 11. Settings And Persistence

The interface-style settings panel becomes the authoritative control surface for desktop shell tuning.

Requirements:

- group parameters by the five approved families
- apply changes live to the renderer shell
- persist renderer-side for the current user/device
- provide reset behavior per the approved defaults

Settings should not expose internal implementation toggles such as seam suppression or pointer-zone topology.

## 12. Component Boundaries

- `apps/desktop/src/main.ts`
  - owns transparent native window creation, show timing, and shell-ready lifecycle
- `apps/desktop/src/preload.cjs`
  - owns shell-ready and window-control bridge surface
- `apps/web/src/app-shell/AppShell.tsx`
  - owns shell root composition and the three continuous structural surface slots
- `apps/web/src/app-shell/ShellTopBar.tsx`
  - owns titlebar control composition, titlebar interaction plane participation, and search-field special rule
- `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
  - owns reusable liquid-glass material primitives and shared interaction primitives, but must stop over-applying background response to non-interactive surfaces
- `apps/web/src/features/settings/interfaceStyle.ts`
  - owns the parameter model, defaults, persistence, and CSS-variable mapping for the approved families
- `apps/web/src/features/file-ops/*Dialog*.tsx`
  - consume the shared dialog material stack instead of inventing local transparency behavior
- `apps/web/src/styles.css`
  - owns structural shell selectors, seam suppression, column-boundary ownership, overlay material rules, and the no-diagonal-gradient rule
- `tools/checks/validate-ui-design.mjs`
  - must be updated to validate the new shell contract instead of the old partial assumptions

## 13. Risks

- Moving drag behavior into the renderer titlebar plane can regress native-feeling interactions if pointer routing is incomplete. The implementation must centralize titlebar hit semantics instead of repeating them per section.
- A global pointer dispatcher can accidentally continue lighting non-interactive shells. Interactive background response must be explicitly gated by surface role.
- Startup transparency can appear fixed in screenshots but still fail on desktop if window show timing is wrong. Verification must include real desktop startup behavior, not only browser snapshots.
- Dialog blur can regress if the overlay host does not create a real backdrop layer. Transparent dialog panels alone are not acceptable.
- Parameter exposure can become noisy if every one-off value is surfaced. Only stable material families should be user-tunable.

## 14. Verification Contract

Implementation is not complete until all of the following hold on desktop:

1. Titlebar local response refreshes while the pointer is inside the titlebar even when it is only near controls rather than directly over them.
2. Left and right titlebar-to-pane seams are gone, matching the already-fixed center join.
3. Column separators are single-owned and do not double up at titlebar/body joins.
4. The lower rectangular host layer is not visible under the rounded renderer shell.
5. Transparency is already correct on first visible show, without requiring maximize/restore to fix the window.
6. Dialogs render as frosted semi-transparent glass and visibly blur/dim the underlying UI.
7. Search field shows only one outer outline and no internal outline/halo behavior.
8. Interactive background halo appears only on interactive surfaces, not on generic background chrome.
9. Center full-image viewing uses the center workbench continuously and does not add a uniform frame around the image.
10. Shell surfaces do not rely on diagonal top-left to bottom-right gradients.

Validation must include:

- static contract checks in `tools/checks/validate-ui-design.mjs`
- desktop startup verification
- desktop interaction verification for titlebar behavior
- visual comparison against real desktop screenshots rather than only synthetic browser captures

## 15. Implementation Readiness

This work is ready to move into implementation planning after spec review. The implementation plan must treat this as a shell rearchitecture, not as a set of isolated CSS patches.

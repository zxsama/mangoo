# Mangoo Global UI Material Layout Design

Updated: 2026-05-21

## 1. Background

Mangoo has already moved toward a global app shell, floating overlays, and Liquid Glass controls, but the current visual result still shows local fixes fighting the underlying layout model. Recent visual evidence shows these failures:

- `D:\Mangoo\.tmp\visual-check\screenshots\ui-integrated-titlebar-main.png`: visible separation lines between titlebar, left sidebar, and right inspector; heavy middle titlebar buttons; search placed too far left; black middle workspace.
- `D:\Mangoo\.tmp\visual-check\screenshots\ui-central-landscape-fit-long-edge.png`: central image is visually framed and cropped by stage padding and sample image border.
- `D:\Mangoo\.tmp\visual-check\logs\desktop-ui-regression-summary.json`: titlebar and side panels have transparent outer surfaces with borders while Liquid Glass blur is visually weak or missing.

The next phase is a global layout and material refactor, not a patch set for isolated symptoms. The approved direction is a unified Workbench Material Layer: the left titlebar and left sidebar read as one continuous glass surface, the middle titlebar and center content column read as one continuous visual surface, the right titlebar and right inspector read as one continuous glass surface, floating popovers behave consistently, and preview areas stop adding frames around media.

## 2. Goals

1. Establish a unified Workbench Material Layer for titlebars, side panels, inspector, and floating overlays.
2. Visually fuse the left titlebar with the left sidebar, the middle titlebar with the center content column, and the right titlebar with the right inspector, with no double border or visible split between each titlebar and its adjoining region.
3. Restore effective Liquid Glass blur on major glass surfaces and overlays while preserving a transparent root and app shell.
4. Make blur controls visibly change major glass material blur, not only small controls or icons.
5. Apply local pointer and edge highlight consistently to all visible glass borders.
6. Make middle titlebar tool buttons icon-only, unframed, and background-free; keep labels only as `aria-label` and `title`.
7. Keep the middle titlebar search box as the only framed glass input and right-align it within the middle titlebar.
8. Make Filter and Sort popovers visually match Tasks and Recent floating popovers.
9. Make Tasks, Recent, Filter, and Sort auto-dismiss on outside click, on Escape, and when another popup is toggled. The center Task Center may remain modal.
10. Keep the right preview stage at a fixed height around `260px`, with media centered horizontally and vertically.
11. Make the center image preview fully unframed: no border, padding, margin, or stage styling that crops or visually frames the image.
12. Center the opened image title/name in the titlebar.
13. Preserve transparent `html`, `body`, `#root`, and `.app-shell`; do not introduce a global rectangular gray backing plate.
14. Extend verification to include screenshot visual checks and startup warning/error checks.

## 3. Non-Goals

- Do not redesign Library browsing, media indexing, file operations, plugin execution, or task execution.
- Do not replace React, Vite, Electron, CSS variables, or the existing Liquid Glass primitives.
- Do not add a global opaque application background to hide material issues.
- Do not make the media grid, central preview image, or right preview image into glass cards.
- Do not reintroduce text labels inside middle titlebar tool buttons.
- Do not make Filter or Sort modal dialogs.
- Do not force the center Task Center to become non-modal in this phase.

## 4. Design

### 4.1 Workbench Material Layer

The shell should read as a workbench made from a small number of continuous material regions:

- Left material region: left titlebar plus left sidebar.
- Center material region: middle titlebar plus workspace content, with the titlebar/content join suppressed so the center column reads as one continuous visual surface. The workspace itself remains a transparent or stable content surface rather than a global glass plate.
- Right material region: right titlebar plus right inspector and preview panel.
- Floating material region: Tasks, Recent, Filter, Sort, dialogs, and menus.

The left, center, and right material regions must avoid double borders where the titlebar touches the adjoining content. The implementation may use shared parent surfaces, adjacent surfaces with edge suppression, or CSS variables that disable the internal joining edge. The visible result is what matters: there is no line, gap, double highlight, or mismatched blur boundary between the left titlebar and sidebar, between the middle titlebar and center workspace content, or between the right titlebar and inspector. Only the outer outline and necessary column separators should remain visible; titlebar and content surfaces should not each draw their own border at the join.

### 4.2 Transparent Root

The renderer root remains transparent:

- `html`
- `body`
- `#root`
- `.app-shell`

These selectors must not paint a full rectangular gray, black, or blurred plate. Shell depth comes from local materials and content surfaces, not a global backing rectangle. Any background needed for contrast must be scoped to the actual component surface that owns it.

### 4.3 Liquid Glass Blur

Liquid Glass blur must be real backdrop material on major glass surfaces:

- Titlebar regions.
- Left sidebar.
- Right inspector and preview panel.
- Floating overlays and popovers.
- Settings glass sections and major glass controls.

The blur slider must update variables used by these surfaces, such as `--glass-blur`, `--glass-elevated-blur`, and `--glass-control-blur`. Moving the slider should visibly affect the titlebar, side panels, inspector, and floating popovers. It is not enough for the slider to change only icons, tiny buttons, or isolated overlays.

### 4.4 Pointer And Edge Highlight

Pointer tracking and local edge highlight belong to the shared Liquid Glass material model. Every visible glass border should participate in the same highlight behavior:

- Left titlebar and sidebar.
- Middle titlebar search input.
- Right titlebar and inspector.
- Tasks and Recent popovers.
- Filter and Sort popovers.
- Settings glass sections.

The local highlight should respond near the pointer edge on all visible glass borders, not only selected surfaces. Edge brightness remains controlled separately from pointer glow so stronger borders do not wash out the entire surface.

### 4.5 Middle Titlebar Controls

The middle titlebar uses a compact tool strip:

- Tool buttons are icon-only.
- Tool buttons have no visible text.
- Tool buttons have no visible border.
- Tool buttons have no visible background.
- Tool button labels remain available through `aria-label` and `title`.
- Icon buttons use stable hit targets and `no-drag` behavior.
- The search input is the only exception: it remains a visible glass input.
- The search input is right-aligned in the middle titlebar.

Any button text currently rendered through spans, captions, or labels must be visually removed from the middle titlebar tool buttons. Accessible names must remain intact.

### 4.6 Floating Popovers

Filter and Sort popovers should use the same visual language as Tasks and Recent floating popovers:

- Floating glass material.
- Real backdrop blur.
- Consistent border, shadow, radius, and edge highlight.
- No opaque menu card that feels separate from the Workbench Material Layer.
- Correct z-index ordering through the overlay host or a shared floating layer convention.

Tasks, Recent, Filter, and Sort share close behavior:

- Escape closes the active popover.
- Clicking outside closes the active popover.
- Toggling another popover closes the previous popover.
- Toggling the same popover closes it.
- Opening the center Task Center closes compact popovers as needed, but the Task Center itself may remain modal.

### 4.7 Right Preview Stage

The right preview stage keeps its previous effective height around `260px`. It is a stable preview well, not a growing card:

- Height or max-height remains approximately `260px`.
- Media is centered horizontally and vertically.
- Images and videos use `object-fit: contain`.
- No cropping in the successful media state.
- No border, black backing, or decorative frame around successful media.

### 4.8 Center Preview

The center image preview is fully unframed:

- No border on the stage.
- No padding that creates an inset frame.
- No margin around the image that reads as a card.
- No background plate behind the image.
- No stage overflow behavior that crops the image in fit modes.
- The image title/name is centered in the titlebar while the preview is open.

The central preview may still use interaction state for pan, zoom, fit, and actual-size modes. Those interactions must not require adding a visible frame around the image.

## 5. Component And File Boundaries

- `D:\Mangoo\apps\web\src\styles.css`
  - Owns CSS variables, Workbench Material Layer rules, left/right titlebar-panel fusion, center titlebar-content fusion, middle titlebar button presentation, popover material, preview stage sizing, and transparent root rules.
- `D:\Mangoo\apps\web\src\design\liquid-glass\LiquidGlassSurface.tsx`
  - Owns shared Liquid Glass pointer state, edge highlight variables, material attributes, and any API needed to disable internal joining edges.
- `D:\Mangoo\apps\web\src\features\settings\interfaceStyle.ts`
  - Owns interface style persistence and maps blur/highlight controls to CSS variables used by real surfaces.
- `D:\Mangoo\apps\web\src\app-shell\AppShell.tsx`
  - Owns top-level shell layout and material-region slots, including direct left, center, and right titlebar-to-content adjacency. It must preserve transparent app root behavior and must not add a global root/app-shell backing plate.
- `D:\Mangoo\apps\web\src\app-shell\ShellTopBar.tsx`
  - Owns titlebar control composition. Middle titlebar buttons must be icon-only and unframed; search is the only visible glass input.
- `D:\Mangoo\apps\web\src\app-shell\ShellOverlayHost.tsx`
  - Owns overlay composition and shared close coordination for Tasks, Recent, Filter, Sort, and modal Task Center.
- `D:\Mangoo\apps\web\src\features\library\FilterMenu.tsx`
  - Owns Filter trigger and content behavior, wired into shared popover close semantics.
- `D:\Mangoo\apps\web\src\features\library\SortMenu.tsx`
  - Owns Sort trigger and content behavior, wired into shared popover close semantics.
- `D:\Mangoo\apps\web\src\features\preview\PreviewPanel.tsx`
  - Owns right preview stage height and media centering.
- `D:\Mangoo\apps\web\src\features\preview\CentralPreviewStage.tsx`
  - Owns center preview unframed media rendering, fit/zoom behavior, and titlebar state handoff.
- `D:\Mangoo\apps\web\src\features\tasks\TaskOverlay.tsx`
  - Owns Tasks floating popover material and center Task Center modal behavior.
- `D:\Mangoo\tools\checks\validate-ui-design.mjs`
  - Owns static UI contract checks for this design.
- `D:\Mangoo\.tmp\visual-check\desktop-ui-regression.mjs`
  - Owns visual and startup regression checks when present.

## 6. Interaction Rules

- Only one compact floating popover is open at a time among Tasks, Recent, Filter, and Sort.
- Escape closes the active compact popover.
- Outside pointer down closes the active compact popover.
- Triggering a different compact popover closes the current one before opening the next one.
- Triggering the same compact popover closes it.
- The center Task Center may keep modal behavior, but opening it should close compact popovers.
- Titlebar controls remain `no-drag`; empty titlebar regions remain draggable.
- Middle titlebar buttons expose names through `aria-label` and `title`; visible text is not rendered.
- Search remains keyboard focusable and visually distinct as a glass input.
- Center preview title/name is centered when preview is open.

## 7. Visual And Material Rules

- No full rectangular root or app-shell backing plate is allowed.
- Glass blur must use actual `backdrop-filter` on glass surfaces.
- Blur variables must be used by major glass surfaces, not only nested controls.
- Left titlebar and sidebar must visually join without an internal line.
- Middle titlebar and center content column must visually join without an internal line, gap, or double border.
- Right titlebar and inspector must visually join without an internal line.
- Only the outer outline and necessary column separators should remain visible at joined titlebar/content boundaries.
- Middle titlebar buttons are icon-only, unframed, and transparent.
- Filter and Sort popovers use the same floating glass material family as Tasks and Recent.
- Right preview stage remains near `260px` and centers media both ways.
- Center preview image is unframed and uncropped in fit modes.
- Successful media previews do not receive decorative borders, opaque mats, or padding frames.

## 8. Risks

- Suppressing internal edges can accidentally remove the outside window outline. The implementation must distinguish internal joining edges from the outer shell border.
- Applying blur at root or app-shell level would hide material gaps but violates the transparent root rule.
- Making middle titlebar buttons visually transparent can reduce discoverability. Use hover/focus states and tooltips without adding persistent button frames.
- Popover outside-click handling can conflict with trigger clicks. The implementation should use a single active popover state or a shared close coordinator.
- Center preview unframing can expose transparent media edges. Do not add a frame to solve that; use the approved unframed design.
- Visual harness assertions can become brittle if they rely on exact pixels. Prefer box relationships, computed styles, and targeted screenshots.

## 9. Verification Contract

Implementation is not complete until all applicable checks pass:

1. Static UI contract:
   - Middle titlebar buttons are icon-only and unframed, with labels retained as `aria-label` and `title`.
   - Search is a glass input and is right-aligned in the middle titlebar.
   - No global gray backing exists on `html`, `body`, `#root`, or `.app-shell`.
   - Left titlebar/sidebar, middle titlebar/center content, and right titlebar/inspector fusion selectors exist.
   - Glass blur variables are used by major material surfaces.
   - Filter and Sort popovers use shared floating popover material.
   - Right preview stage is fixed around `260px` and centers media.
   - Center preview stage has no border, padding, or visible frame.
2. Web and desktop checks:
   - `npm run check:ui-design`
   - `npm run check:web`
   - `npm run check:desktop`
   - Web build and desktop build.
3. Visual harness checks when `D:\Mangoo\.tmp\visual-check\desktop-ui-regression.mjs` exists:
   - Initial integrated titlebar screenshot shows left, center, and right material fusion.
   - Middle titlebar and center content have no internal seam, gap, or double border at their join.
   - Blur slider visibly changes major glass blur.
   - Pointer/edge highlight appears on all visible glass borders under test.
   - Middle titlebar buttons are icon-only; search is right-aligned.
   - Tasks, Recent, Filter, and Sort auto-dismiss correctly.
   - Right preview stage remains near `260px` with centered media.
   - Center preview has no border, padding, or frame; title is centered.
4. Startup health:
   - Visual harness summary has no fatal error.
   - Browser console has no warnings or errors introduced by the change.
   - Network log has no failed app assets.

Verification failures must be fixed in the implementation phase. They should not be waived as subjective visual differences when they contradict this contract.

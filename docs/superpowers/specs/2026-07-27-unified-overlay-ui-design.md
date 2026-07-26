# Unified Overlay UI Design

Date: 2026-07-27

Status: Approved design specification

## 1. Purpose

This specification defines one visual and interaction system for Megle's transient and secondary-window UI surfaces. It replaces the accumulated independent styling of titlebar popovers, context menus, library management overlays, file-operation dialogs, search controls, and notification surfaces with a shared overlay architecture.

The approved direction is a restrained, high-density desktop product language based on the existing Settings and Plugins window material treatment. Settings and Plugins are visual references only. Their layout, component structure, behavior, and styling are not changed by this work.

The target design characteristics are:

- Layered acrylic and frosted-glass materials on chrome, controls, popovers, and dialogs.
- Darker navigation or metadata zones paired with lighter-to-darker graded content zones.
- No divider line between side and content zones. Tonal contrast defines the boundary.
- A titlebar or header hairline only when the surface has a real title or structural header.
- High information density without nested cards around every content group.
- One pointer-proximity highlight system and one overlay lifecycle model.
- Existing data, business behavior, and performance boundaries remain authoritative.

Design calibration:

- Design variance: 4/10.
- Motion intensity: 3/10.
- Visual density: 7/10.

## 2. Scope

### 2.1 In scope

The following renderer surfaces and their nested surfaces are included:

1. The library switcher opened from the main titlebar.
2. The library icon picker nested inside the library switcher.
3. The remove-folder menu nested inside the library switcher.
4. The remove-folder confirmation alert dialog.
5. The titlebar filter popover.
6. The titlebar sort popover.
7. The titlebar layout popover.
8. The titlebar search field and its clear, focus, active-query, empty-result, and disabled states.
9. Every media, folder, and root context menu rendered through the shared context-menu component.
10. Rename, move, recycle-bin, and permanent-delete dialogs opened from context-menu actions.
11. The compact notification popover opened from the titlebar notification button.
12. The full notification center secondary window, including current status, history, and process logs.
13. Nested menus, confirmation dialogs, filter controls, buttons, progress displays, error surfaces, empty states, and loading states owned by the surfaces above.
14. Shared overlay positioning, stacking, dismissal, focus, keyboard, animation, accessibility, and appearance-token behavior.
15. Design validators and visual fixtures required to prevent regression of this contract.

### 2.2 Out of scope

The following are not changed:

- Settings window layout, behavior, component structure, and styling.
- Plugins window layout, behavior, component structure, and styling.
- Main media-grid layout, thumbnail presentation, preview canvas, inspector layout, or file-tree layout.
- Main titlebar button placement or titlebar height, except state attributes needed to connect triggers to the shared overlay system.
- Core HTTP APIs, SQLite schema, notification contract, notification persistence, or notification grouping rules.
- Notification read or unread state. Megle continues to have no read-state model.
- Search semantics, search indexing, query syntax, or result ordering.
- Native Windows file, folder, open, save, and confirmation dialogs supplied by Electron or the operating system.
- New user-facing appearance controls.
- New third-party component, positioning, animation, or accessibility dependencies.
- A migration to Radix, Fluent, Material, or another external design system.

## 3. Existing Surface Inventory

The implementation must account for every existing component listed below.

| Surface | Current component | Trigger or parent | Nested surfaces |
| --- | --- | --- | --- |
| Library switcher | `apps/web/src/features/library/LibrarySwitcher.tsx` | Main titlebar library button | Icon picker, remove-folder menu, remove confirmation |
| Library icon picker | `apps/web/src/features/library/libraryGlyph.tsx` | Library row icon button | None |
| Filter popover | `apps/web/src/features/library/FilterMenu.tsx` | Main titlebar filter button | Kind, rating, favorite, tag controls |
| Sort popover | `apps/web/src/features/library/SortMenu.tsx` | Main titlebar sort button | None |
| Layout popover | `apps/web/src/features/library/LayoutMenu.tsx` | Main titlebar layout button | None |
| Search field | `apps/web/src/features/library/SearchBar.tsx` | Main titlebar | Clear action; results remain in the media grid |
| Context menu | `apps/web/src/features/file-ops/ContextMenu.tsx` | Media, folder, or root right-click | Rename, move, delete actions |
| Rename dialog | `apps/web/src/features/file-ops/RenameDialog.tsx` | Context-menu action | Inline validation and server error |
| Move dialog | `apps/web/src/features/file-ops/MoveDialog.tsx` | Context-menu action | Destination tree and cross-root error |
| Delete dialog | `apps/web/src/features/file-ops/DeleteConfirm.tsx` | Context-menu action | Permanent-delete text confirmation |
| Compact notification | `apps/web/src/features/notifications/NotificationPopover.tsx` | Main titlebar notification button | Open-full-center action |
| Notification center | `apps/web/src/features/notifications/NotificationWindow.tsx` | Compact popover or direct desktop bridge intent | Status, history, process logs |
| Notification status | `NotificationStatusView.tsx` | Notification center | Failure drill-down to process logs |
| Notification history | `NotificationHistoryView.tsx` | Notification center | Filters, details, cancel and retry |
| Process logs | `ProcessLogView.tsx` | Notification center | Filters, export, clear, media focus |
| Overlay host | `apps/web/src/app-shell/ShellOverlayHost.tsx` | Main App | Context menu and file-operation dialogs |
| Titlebar orchestration | `apps/web/src/app/App.tsx` and `apps/web/src/app-shell/ShellTopBar.tsx` | Main shell | Library, filter, sort, layout, notification, search |
| Positioning helper | `apps/web/src/features/library/anchoredPopover.ts` | Filter, sort, layout | None |
| Appearance model | `apps/web/src/features/settings/interfaceStyle.ts` | Main and secondary windows | CSS variable generation |
| Pointer system | `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx` | All interactive surfaces | Global pointer target selection |

No in-scope surface may retain a separate visual or lifecycle subsystem after migration.

## 4. Design Baseline

Settings and Plugins define the reference material language:

- The window titlebar remains visually separate through its existing bottom hairline.
- Navigation or list zones use a darker side-material tone.
- Content zones use a subtle light-to-dark gradient derived from the same configured material color.
- Adjacent zones do not use a vertical divider.
- The outer window owns the acrylic material. Inner content does not simulate a second opaque window.
- Selection is expressed through fill and text contrast, not an outline ring.
- Controls use restrained density, stable icon sizes, and limited decorative framing.

The overlay redesign applies this language proportionally. Small menus do not copy the two-column Settings layout. They translate the same tone, depth, hierarchy, edge treatment, and control language into compact surfaces.

## 5. Shared Overlay Architecture

### 5.1 Required primitives

The shared system must provide the following primitives with these names and responsibilities.

#### OverlaySurface

Owns the visible material shell.

Contract:

- Accepts one surface role: `popover`, `nested`, `modal`, or `secondary-content`.
- Uses `LiquidGlassSurface` rather than recreating acrylic layers.
- Owns the outer radius, material fill, blur, saturation, outer edge, shadow, and reduced-transparency fallback.
- Does not own feature-specific layout or data.
- Registers interactive descendants with the existing global pointer system.

#### AnchoredOverlay

Owns trigger-relative positioning and lifecycle.

Contract:

- Receives a trigger element, preferred placement, alignment, offset, width constraints, and open state.
- Returns collision-safe fixed positioning.
- Supports top and bottom placement for first-level overlays.
- Supports left and right placement for nested overlays.
- Updates on resize, owning-container scroll, and trigger geometry changes.
- Provides the overlay element and trigger relationship required for focus restoration and ARIA attributes.

#### OverlayStack

Owns ordering and dismissal.

Contract:

- Allows at most one first-level titlebar overlay at a time.
- Allows nested overlays only as descendants of the active first-level overlay.
- Allows a modal to supersede the overlay that opened it without destroying the parent state unless the feature explicitly closes it.
- Escape closes only the topmost dismissible layer.
- Outside pointer dismissal applies only to the topmost dismissible layer.
- Window blur, page hide, and renderer visibility loss close transient layers.
- Restores focus to the correct trigger when the final descendant closes.

#### MenuList and MenuItem

Own menu semantics and row geometry.

Contract:

- Supports action, checkbox, radio, selected, disabled, danger, separator, icon, secondary text, shortcut, and submenu affordances.
- Implements roving focus, Arrow Up, Arrow Down, Home, End, Enter, and Space.
- Uses Arrow Right and Arrow Left for nested menu entry and return.
- Skips disabled items during keyboard navigation.
- Uses fill for selected state and does not draw selection border rings.

#### ModalSurface

Owns modal focus and backdrop behavior.

Contract:

- Uses `aria-modal="true"` with a labelled title.
- Traps focus while open and restores focus on close.
- Supports title, optional description, body, inline status, and action footer.
- The default action order is secondary then primary or danger.
- Destructive confirmation initially focuses the cancel action unless text entry is required.
- Busy destructive operations cannot be dismissed accidentally.

#### OverlayField

Owns search, filter, text-input, and select presentation.

Contract:

- Supports label, accessible name, leading icon, trailing action, helper text, error text, disabled state, and focus state.
- Uses center-material variables and the shared control radius.
- Search fields opt out of pointer glow and use field-level hover and focus feedback.
- Other interactive trailing actions continue to use the global pointer system.

### 5.2 API invariants

- Feature components provide content, state, callbacks, labels, and domain semantics.
- Shared primitives provide material, geometry, focus, keyboard, pointer, motion, and dismissal.
- Shared primitives must not import Core DTOs, notification DTOs, filesystem APIs, or feature hooks.
- Feature components must not calculate z-index values or viewport collision themselves.
- Feature components must not add independent document-level dismissal listeners when the shared stack supplies that behavior.
- Renderer code continues to avoid raw `fetch` and Node filesystem imports.

## 6. Surface Hierarchy

The system uses four explicit visual levels.

| Level | Use | Material behavior | Interaction behavior |
| --- | --- | --- | --- |
| Popover | Filter, sort, layout, compact notification, library switcher | Dialog outer material with center-derived readable interior | Anchored to a titlebar trigger; dismissible |
| Nested | Icon picker, remove-folder list, future context submenu | Parent dialog fill plus a fixed 0.04 white overlay and the shared tight overlay shadow | Anchored to parent item; topmost keyboard layer |
| Modal | Rename, move, delete, remove-folder confirmation | Dialog material plus configured backdrop dim and blur | Focus trap; supersedes transient input |
| Secondary content | Full notification center | Existing secondary window shell; side-material navigation and graded content | Independent Electron window lifecycle |

The implementation must define one semantic layer scale for overlay host, popover, nested overlay, modal backdrop, modal panel, and overlay scrollbar. Individual feature CSS must not invent z-index values.

## 7. Appearance Parameter Mapping

No new appearance preference is introduced. Semantic overlay variables may be derived from existing variables, but they are not new user-facing settings.

### 7.1 Side material

Existing preferences:

- `sideBlur`
- `sideOpacity`
- `sideOverlayStrength`
- `sideOverlayColor`
- `sideSaturation`
- `sideStrokeOpacity`

Required use:

- Notification-center navigation rail.
- Dark navigation or metadata zones inside larger overlay compositions.
- Derived dark and graded pane colors already represented by `--secondary-window-pane-dark`, `--secondary-window-pane-light`, and `--secondary-window-pane-end`.

Side material must not be used as the outer material for every popover.

### 7.2 Center material

Existing preferences:

- `centerBlur`
- `centerOpacity`
- `centerOverlayStrength`
- `centerOverlayColor`
- `centerSaturation`
- `centerStrokeOpacity`

Required use:

- Search fields, selects, text inputs, progress tracks, and readable inner regions.
- Content-side gradients inside the notification center.
- Selected-row fill strength derived from center material and the existing accent palette.
- Compact content blocks that need readability but are not independent elevated surfaces.

### 7.3 Dialog material

Existing preferences:

- `dialogBlur`
- `dialogOpacity`
- `dialogOverlayStrength`
- `dialogBackdropDim`

Required use:

- Popover, nested overlay, context menu, and modal outer shells.
- Modal backdrop dimming and blur.
- Stronger nested-overlay material derived from the same dialog values.

### 7.4 Shared appearance values

The following existing values remain global:

- Window, panel, control, content, surface, overlay, and tight radius variables.
- Pointer sensing distance, response radius, falloff, halo brightness, and edge-highlight brightness.
- Dither and backdrop-gradient strength.
- Existing text, accent, warning, and danger colors.

Hardcoded component radii and hardcoded near-opaque popup backgrounds must be removed from in-scope surfaces.

## 8. Pointer and Selection Contract

Megle continues to have one pointer-proximity highlight system driven by `LiquidGlassSurface.tsx`.

Rules:

1. Buttons, menu items, library rows, notification actions, disclosure rows, icon cells, and swatches use the shared pointer target system.
2. In-scope components must not add a per-element hover border, hover background, or second glow implementation.
3. Hover may brighten icon or text color, but it must not create a competing rectangular surface.
4. Selection and active states use a stable background fill and text contrast.
5. Keyboard focus uses a visible focus outline that is independent of selection and hover.
6. Disabled controls are ignored by the pointer system and remain readable at reduced emphasis.
7. Search and text fields use `data-pointer-glow="off"` because field focus is rectangular and semantic.
8. Danger styling remains restrained until the user reaches a destructive confirmation state.

## 9. Positioning, Stack, Focus, and Keyboard

### 9.1 Positioning

- All anchored overlays maintain a 12 CSS-pixel viewport safety gutter.
- First-level overlays prefer opening below the trigger and flip above when required.
- Nested overlays prefer the side with sufficient room and may flip horizontally.
- Width is clamped to the available viewport. Height is clamped and internal scrolling is enabled when content exceeds available space.
- Position recalculates on resize, relevant scroll, and trigger geometry change.
- The library switcher remains visually associated with the library trigger instead of becoming a centered modal.
- Context menus preserve pointer-origin placement and shift only enough to remain fully visible.

### 9.2 Overlay exclusivity

- Library, filter, sort, layout, and compact notification are mutually exclusive first-level overlays.
- Opening one closes the previous first-level overlay before the new one becomes visible.
- Search focus does not count as a first-level overlay because search does not open a result surface.
- Context menus close any titlebar popover before opening.
- Opening a modal from a context menu closes the menu after capturing focus restoration context.

### 9.3 Focus behavior

- A menu opens with the selected item focused when one exists; otherwise the first enabled item is focused.
- A non-menu popover focuses its first meaningful control only when opened from keyboard. Pointer-opened informational popovers do not steal focus.
- Closing restores focus to the originating trigger when that trigger is still mounted and enabled.
- Modal focus remains trapped until the modal closes.
- Nested overlays return focus to their parent trigger when closed.

### 9.4 Keyboard behavior

- Escape closes the topmost dismissible surface.
- Arrow navigation, Home, End, Enter, and Space follow standard menu, listbox, tab, and tree semantics.
- Tab stays within modals but may leave non-modal popovers, closing them when focus exits the overlay family.
- The notification center retains arrow-key navigation between its three pages.
- All keyboard interactions must work without relying on pointer coordinates.

## 10. Component Migration Rules

### 10.1 Library switcher

- Preserve the existing anchored single-column information architecture.
- Use a compact structural header containing the search field and close action.
- Use a darker list tone and a subtle graded footer/content tone without a vertical divider.
- Each library row contains icon, name, truncated path, active state, and contextual actions.
- Row selection uses fill. Availability uses text and icon emphasis, not a border.
- Add-folder and remove-folder actions remain enabled only for the active library.
- Create and open actions remain in the footer and use text-action treatment rather than framed cards.
- Loading displays library-row skeletons.
- Empty search displays a compact query-specific empty state.
- Registry errors display an inline error region without replacing the entire window.

### 10.2 Library icon picker

- Render as a nested overlay using shared positioning and stack behavior.
- Icon selection uses a filled selected cell, not an outline ring.
- Color selection uses a selected fill or check indicator with accessible color names.
- Support arrow-key grid navigation and Escape return to the icon trigger.
- The same component may remain reusable inside Settings, but Settings presentation must not change. A surface variant is required if shared style changes would otherwise alter Settings.

### 10.3 Remove-folder menu and confirmation

- The folder list is a nested menu with relative names and truncated paths where available.
- Empty roots display a non-interactive empty row.
- Selecting a root opens a modal alert dialog above the switcher.
- The confirmation uses the shared modal surface and danger action hierarchy.
- Cancelling the modal returns to the remove-folder trigger without closing the switcher.

### 10.4 Filter popover

- Preserve kind, rating, favorite, tags, and clear-filter behavior.
- Use compact labelled sections separated by spacing, not cards or full-width border rules.
- Multi-select controls use menuitemcheckbox semantics and `aria-checked`.
- Active filters use stable fill and check state.
- Clear filters remains disabled when no filter is active.
- No new filtering behavior or saved-filter feature is introduced.

### 10.5 Sort and layout popovers

- Share one single-selection menu presentation and keyboard implementation.
- The selected option uses a check indicator and stable fill.
- Icons remain visible in layout options. Sort options remain text-first.
- Both are controlled by the common first-level overlay state.

### 10.6 Search field

- Search remains an inline titlebar field that filters the central media grid.
- No suggestions, recent-search list, command palette, or result popover is introduced.
- The field retains a search icon, placeholder, current value, and clear action.
- Resting state is quiet and readable. Focus increases border contrast and center-material fill without outer glow.
- An active query is indicated by text/value presence, not a badge.
- No-result feedback remains in the media content area and must use the same query wording as the search field.
- Clearing preserves input focus.

### 10.7 Context menus

- Media, folder, and root context menus continue to use one component and feature-supplied item model.
- Row height, icon column, label alignment, disabled state, danger state, separators, and keyboard behavior are shared.
- Separators are sparse and represent semantic action groups only.
- Context menus must open completely inside the viewport at pointer origin.
- Future submenus must use the nested overlay contract rather than feature-specific absolute positioning.

### 10.8 Rename, move, and delete dialogs

- All use the shared modal shell, field, inline-error, and action-footer language.
- Rename focuses and selects the editable filename segment as it does today.
- Move keeps the destination tree and excluded-folder logic. The tree becomes the dominant content region rather than an inset decorative card.
- Recycle-bin and permanent-delete flows remain distinct.
- Permanent deletion retains typed-count confirmation.
- Busy state disables dismissal and shows progress through action copy without changing dialog size.
- Server errors remain inline and do not use transient notifications.

## 11. Notification Product Design

### 11.1 Product invariants

- Notification data remains sourced through `@megle/notification-contract` and the existing preload bridge.
- The notification data model, operation grouping, status values, persistence, history pagination, process-log model, and thumbnail-failure snapshots do not change.
- There is no read or unread state.
- Per-thumbnail notifications remain prohibited.
- Nonessential events remain history-only or status-only according to the existing presentation classification.

### 11.2 Compact notification popover

The compact popover is a background-activity monitor, not a miniature history center.

It displays only:

- Current health when notification synchronization is degraded.
- Active logical operations and their progress.
- Active aggregated thumbnail-cache generation status.
- Active scan paths already supplied by the existing model.
- A single action to open the full notification center.

It does not display:

- Recent completed results.
- Cancel or retry actions.
- History filters.
- Process logs.
- Read or unread markers.

Presentation rules:

- Use the heading “后台活动”.
- Health appears as concise semantic text, not a decorative badge.
- Active operations use compact rows with title, current phase or path, progress, and percentage where available.
- Thumbnail cache uses one aggregated progress section per library summary already present in the compact model.
- Idle state is one compact row stating that no background work is active.
- The footer action remains visible without scrolling.

### 11.3 Full notification center information architecture

The full center keeps three pages:

1. `当前状态`
2. `历史记录`
3. `进程日志`

The left navigation uses the Settings-style dark side material. The right content area uses the Settings-style lighter-to-darker gradient. No vertical divider is drawn between them.

#### Current status

- Shows system health, active work, thumbnail-cache state, and recent failures requiring attention.
- Uses continuous sections and spacing rather than a card around every row.
- Cancel actions are available for cancellable active operations.
- Thumbnail failures continue to open process logs with the existing failure filter and snapshot.
- Empty state explains that no background work needs attention.

#### History

- Owns completed, failed, cancelled, and historical operation records.
- Owns cancel and retry actions where permitted by the existing operation state.
- Uses a single compact toolbar containing search and filters.
- Details expand in place and do not open a second modal.
- Long lists continue to use existing virtualization and pagination boundaries.

#### Process logs

- Owns session process logs, structured thumbnail-failure entries, filtering, export, clear, and media-focus actions.
- Uses stable columns for time, source, level, and message at normal width.
- Collapses metadata into two lines at narrow width without horizontal page overflow.
- The toolbar remains visible while the log list scrolls.
- Clicking a locatable failure continues to focus the corresponding media in the main window through the existing desktop bridge.

### 11.4 Notification typography and controls

- Page title: 18-19px, semibold.
- Section title: 12-13px, semibold.
- Primary row text: 12-13px.
- Metadata: 10-11px with tabular numerals for time and percentage.
- Remove decorative all-caps eyebrow text.
- Buttons, selects, text inputs, disclosures, and status rows use shared overlay controls.
- Severity is communicated through text and icon color with restrained background tint only when needed.

## 12. Dimensions, Spacing, and Shape

Default geometry:

- Basic menu row height: 32px.
- Complex library or notification row height: 40-44px minimum.
- Icon-only action: 28-30px square.
- Text field and standard action: 32px height.
- Menu icon size: 14px.
- Primary title or state icon: 16px.
- Internal spacing scale: 4px, 8px, 12px, and 16px.
- Popover viewport gutter: 12px.
- First-level popover edge offset from trigger: 6px.
- Nested popover edge offset from parent item: 4px.

Shape mapping:

- Outer overlay: `--radius-overlay`.
- Content grouping: `--radius-surface`.
- Standard controls: `--radius-control`.
- Menu rows and compact metadata: `--radius-tight`.
- Pills are reserved for true compact statuses or counts, not ordinary buttons.

## 13. Responsive Behavior

Megle is Windows-first, but all renderer surfaces must remain usable from the minimum supported window size.

- Popover width is `min(preferred width, viewport width - 24px)`.
- Library switcher retains one column and reduces secondary-path width before hiding essential actions.
- Filter sections wrap without making the popover wider than the viewport.
- Notification popover fills available width below narrow titlebars and keeps its footer fixed.
- The full notification center uses its side navigation at normal width.
- At narrow secondary-window width, notification navigation becomes a compact top tab row and the side note is removed.
- Modal width is clamped to the viewport and modal body scrolls independently.
- Move-tree rows preserve indentation while labels ellipsize.
- No responsive state may hide the primary close, cancel, retry, or destructive-confirmation action.

## 14. Accessibility

Acceptance requires:

- WCAG AA contrast for text, controls, placeholders, errors, and focus indicators against both blurred and solid fallback materials.
- Correct menu, menuitem, menuitemcheckbox, listbox, option, dialog, alertdialog, tab, tabpanel, tree, and treeitem semantics.
- `aria-expanded`, `aria-controls`, `aria-haspopup`, `aria-selected`, and `aria-checked` used according to component role.
- Accessible names for icon-only actions and color swatches.
- Visible keyboard focus independent of hover and selection.
- Focus trap and restoration for modal surfaces.
- Complete keyboard operation without pointer input.
- Reduced-transparency fallback with opaque readable surfaces.
- Reduced-motion behavior without delayed or hidden content.
- Error messages associated with the relevant input or action context.

## 15. Motion

Motion communicates hierarchy and state change only.

- First-level popover enter and exit: 140ms opacity plus 4px translation.
- Nested overlay enter and exit: 110ms opacity plus 3px translation.
- Modal backdrop and modal panel: synchronized 160ms opacity transition. The panel translates 4px and does not scale.
- Progress values animate width for 180ms when updates are not excessively frequent.
- Overlay animation uses transform and opacity only.
- `prefers-reduced-motion: reduce` disables translation and all overlay enter and exit animation.
- Content must never wait for a decorative animation before becoming keyboard accessible.

## 16. Performance Constraints

- Opening an overlay must not initiate notification polling, library polling, or media queries that do not already occur for that feature.
- Pointer highlighting continues to use the existing near-cursor target driver and must not scan the full DOM per frame.
- Positioning updates are batched through `requestAnimationFrame` and perform no React state update when geometry is unchanged.
- No permanent animation loop is introduced for idle surfaces.
- Notification compact rendering remains bounded by the existing compact model.
- Notification history and process logs retain pagination, bounded buffers, and virtualization.
- No per-thumbnail React component or notification entry is added to the notification surfaces.
- Acrylic effects remain on chrome and overlays. Media grids and preview canvases remain sharp and stable.
- New shared primitives must not introduce raw document-wide pointermove handlers beyond the existing pointer system.

## 17. Migration and Removal Strategy

Migration is complete only when old visual and lifecycle paths are removed, not merely overridden.

Required strategy:

1. Establish shared overlay primitives and semantic layer variables inside `apps/web/src/design/`.
2. Connect titlebar popovers to one controlled first-level overlay state.
3. Migrate filter, sort, layout, search, and library surfaces without changing domain behavior.
4. Migrate context menu and file-operation dialogs while preserving the existing file-operations controller.
5. Migrate compact notification presentation without changing its source model.
6. Migrate notification-center controls and layout without changing notification APIs or state ownership.
7. Remove component-local positioning, dismissal, focus, z-index, hardcoded material, radius, shadow, and hover implementations made redundant by the shared system.
8. Remove obsolete CSS blocks instead of leaving later overrides in `styles.css`.
9. Preserve Settings and Plugins selectors and output exactly unless a shared primitive requires an explicit nonvisual compatibility change.
10. Add validators that reject reintroduction of feature-local popup materials, duplicate overlay stacks, and competing pointer-hover surfaces.

Allowed temporary state during migration:

- A feature may use an adapter around the shared primitive while its JSX is being converted.
- Old and new visual systems must not render simultaneously for the same surface.
- Temporary adapters must be removed before the final acceptance gate.

## 18. Acceptance Criteria

The redesign is accepted only when all criteria below are satisfied.

### 18.1 Scope and architecture

- Every in-scope surface uses the shared overlay architecture.
- Settings and Plugins are visually and behaviorally unchanged.
- No new appearance parameter or third-party dependency exists.
- No notification contract or data-model change exists.
- No raw fetch or Node filesystem import is added to Web code.

### 18.2 Visual consistency

- Popovers, nested menus, modals, context menus, and notification surfaces clearly belong to one design language.
- Appearance changes in Settings propagate correctly to all in-scope surfaces.
- Side and content zones have no divider line.
- Header hairlines appear only on real structural headers.
- Hardcoded popup backgrounds and radii are absent from migrated feature CSS.
- Selection uses fill, pointer hover uses the global pointer system, and keyboard focus remains independently visible.

### 18.3 Interaction

- Only one first-level titlebar overlay is open at a time.
- Nested overlays flip and clamp correctly near every viewport edge.
- Escape, outside click, window blur, focus restoration, and modal trapping behave consistently.
- All menu and dialog operations are keyboard accessible.
- Search behavior and media results are unchanged.
- File-operation behavior and validation are unchanged.

### 18.4 Notifications

- Compact notification displays only active progress and degraded-health information.
- Compact notification contains no recent-result list, cancel action, retry action, history, or logs.
- Full notification center owns cancel, retry, current status, history, and process logs.
- Failure drill-down and media focus continue to work.
- No read-state UI or model is introduced.
- Existing notification performance limits remain intact.

### 18.5 States and fallback

- Loading, empty, error, disabled, busy, degraded, keyboard-focus, nested-menu, and narrow-window states are visually complete.
- Reduced-motion and reduced-transparency modes remain usable and readable.
- Solid material fallback remains readable when backdrop-filter is unavailable.

## 19. Test Matrix

### 19.1 Automated verification

Required repository gates:

```powershell
npm run check:web
npm run check:desktop
npm run check:ui-design
npm test
```

Required focused coverage:

| Area | Automated coverage |
| --- | --- |
| Overlay stack | Mutual exclusion, nested ownership, Escape order, outside dismissal, blur dismissal |
| Positioning | Top/bottom flip, left/right flip, viewport clamp, narrow viewport, scroll and resize updates |
| Focus | Initial focus, roving focus, modal trap, nested return, trigger restoration |
| Keyboard | Arrow keys, Home, End, Enter, Space, Escape, Tab behavior |
| Appearance | Existing CSS variables used for side, center, dialog, radius, pointer, and fallback mappings |
| Pointer | No migrated per-element hover background or border; disabled targets ignored |
| Library | Search, switch, availability, icon picker, add/remove folder, confirm cancel and confirm |
| Search | Query update, clear-and-refocus, no new popover, no-result copy consistency |
| File operations | Rename validation, move tree, cross-root error, recycle, permanent typed confirmation, busy state |
| Compact notification | Active-only rendering, empty, degraded, thumbnail aggregation, no results/actions |
| Notification center | Page navigation, history filters, cancel/retry, failure logs, export/clear, media focus |
| Responsive | Narrow popover, narrow modal, narrow notification window, no clipped primary action |

`tools/checks/validate-ui-design.mjs` must guard at least these invariants:

- Shared overlay primitives use existing appearance variables.
- Migrated feature files do not define independent acrylic backgrounds or z-index values.
- Migrated interactive rows do not introduce per-element hover borders or hover backgrounds.
- Search remains explicitly opted out of pointer glow.
- Compact notification does not render recent results or cancel/retry actions.
- Settings and Plugins retain their existing window structure.

### 19.2 Real desktop verification

Run the explicit Windows compositor harness:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
```

The real desktop interaction pass must cover:

1. Open each titlebar overlay with pointer and keyboard.
2. Move between library, filter, sort, layout, and notification and verify first-level exclusivity.
3. Open the library switcher near narrow window bounds and exercise icon picker, remove menu, and confirmation.
4. Open media, folder, and root context menus at all four viewport corners.
5. Complete rename, move, recycle-bin, and permanent-delete keyboard flows.
6. Confirm search focus, clear, active query, and no-result presentation.
7. Verify compact notification in active, thumbnail-generation, empty, and degraded states.
8. Verify notification status, history, process logs, failure drill-down, cancel, retry, export, clear, and media focus.
9. Resize the full notification center through normal and narrow widths.
10. Test at Windows scale factors 100%, 125%, and 150%.
11. Test the minimum and maximum supported values for side, center, and dialog appearance settings and confirm every surface responds to the intended parameter group.
12. Test reduced motion, reduced transparency, and backdrop-filter fallback.
13. Confirm media browsing, preview return, layout switching, and subfolder interaction remain responsive while overlays open and close.

## 20. Self-Review Result

This specification has been reviewed against the approved direction and repository rules.

- No unresolved placeholder marker remains.
- Scope and exclusions are explicit.
- Settings and Plugins are references only and are not migration targets.
- Appearance mapping uses existing parameters only.
- Notification compact and full-center responsibilities are unambiguous.
- Search behavior explicitly excludes a result popover.
- Shared primitive responsibilities do not overlap feature data ownership.
- Migration rules require deletion of obsolete code and CSS rather than override accumulation.
- Acceptance criteria are observable and testable.
- The scope is large but coherent as one overlay-system initiative and is sufficiently defined for a later implementation plan.

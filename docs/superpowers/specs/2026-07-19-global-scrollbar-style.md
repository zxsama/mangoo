# Global Scrollbar Style

Status: confirmed specification

## Scope

Mangoo uses one custom overlay scrollbar for every scrollable surface. Native
scrollbars remain hidden, the overlay is fixed and does not reserve layout
width, and the same rule applies to vertical and horizontal scrolling.

## Visual contract

- The visible thumb is `5px` thick in CSS logical pixels. It is not multiplied
  by `devicePixelRatio`.
- The invisible interaction rail may remain `10px` wide. It exists only to
  provide a usable pointer hit area and must not paint a background or occupy
  layout space.
- For a vertical scrollbar, the visible thumb's right edge is `5px` from the
  right edge of its owning scroll container.
- For a horizontal scrollbar, the visible thumb's bottom edge is `5px` from
  the bottom edge of its owning scroll container.
- The thumb is rounded, grayscale (`saturate(0)`), and has no arrows, endcaps,
  rail background, or layout gutter.
- The media grid's invisible overhang matches the outer content inset: `16px`
  on the desktop layout and `10px` in the compact layout. The virtual grid
  subtracts the same padding when measuring columns, so this moves its overlay
  scrollbar to the center-pane edge without changing thumbnail geometry.
- The appearance icon viewport similarly extends `16px` into the popup's
  existing right padding, with equal internal content clearance. Icon columns
  retain their width while the entire invisible rail stays beyond the choices.
  This is viewport overhang, not a native scrollbar gutter or another controller.

## Visibility and interaction

- Scrollbars are hidden by default.
- Pointer presence in a scrollable panel keeps its scrollbar visible at 50%
  opacity. Hovering the thumb raises it to 75%; dragging raises it to 100%.
- Scroll activity keeps the relevant scrollbar visible through a short
  requestAnimationFrame-driven idle window. CSS transitions provide roughly
  `140ms` fade-in and fade-out; the controller does not use timers.
- Starting a shell divider drag immediately hides the overlay scrollbars.
  Ending the drag dispatches the same shell-resize state event so geometry is
  recalculated against the new container bounds.
- The scrollbar controller must preserve pointer dragging and keyboard
  accessibility of the shell dividers.
- Both the visible thumb and the full invisible rail start the same pointer
  drag path. Preserve the thumb grab offset and end drag state on pointerup
  or pointercancel; do not install competing thumb and rail handlers.

## Popup ownership

Global rails live under `document.body`, outside their scroll owner's DOM
subtree. Overlay containment must resolve their registered owner before
checking the existing popup surface/trigger family. A popup's own scrollbar
keeps it open; a parent scrollbar dismisses only an external child layer;
an unrelated background scrollbar remains an outside interaction.

`registerOverlayScrollbarOwner(targets, getOwner): () => void` registers exact
rail/thumb elements and returns disposal cleanup. The callback supplies the
controller's current scrollable owner. `resolveOverlayScrollbarOwner(target)`
returns that owner only while target and owner remain connected and the owner
is visible. The controller rejects non-scrollable or divider-resizing owners.
Use the shared WeakMap registration; copied CSS classes or DOM attributes must
not confer popup containment. Disposal removes registration even if an old
rail is later reattached.

The actual mounted regression must hit-test and drag vertical/horizontal
thumbs and rails, assert scrolling without owner dismissal, and cover nested
layers, ordinary outside dismissal, context menus, focus retention, hidden or
detached owners, disposal and unregistered lookalike elements. A plain scroll
container fixture cannot alone verify popup outside-click integration.

## Implementation boundary

- `apps/web/src/app-shell/useOverlayScrollbars.ts` owns hit testing, geometry,
  scroll activity, and divider-resize coordination.
- `apps/web/src/app-shell/AppShell.tsx` publishes the divider resize state.
- `apps/web/src/design/overlays/overlayScrollbarOwner.ts` owns the exact-element
  containment bridge; `OverlayProvider.tsx` applies it before normal containment.
- `apps/web/src/styles.css` owns global dimensions, grayscale appearance,
  opacity states, and transitions.
- `tools/checks/validate-ui-design.mjs` guards the global contract.

## Non-goals

- Do not reserve a scrollbar gutter or change content width to make room for
  the overlay.
- Do not add a second per-component scrollbar implementation.
- Do not use device-pixel scaling for the visual thumb thickness or edge gap.

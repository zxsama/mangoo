# Settings and Plugins Split Background Design

## Goal

Restyle the standalone Settings and Plugins windows so their body uses the visual hierarchy from the approved locator mockup:

- the left pane is dark;
- the right pane starts lighter beside the left pane and fades smoothly to dark toward the right edge;
- no vertical divider is drawn between the panes;
- the existing titlebar appearance and its bottom divider remain unchanged.

The result must remain part of Mangoo's shared Liquid Glass system rather than becoming two unrelated component-specific themes.

## Shared Window Surface

`SecondaryWindowFrame` remains the shared frameless BrowserWindow structure. Its titlebar layout, titlebar background, bottom border, reveal lifecycle, drag behavior, close button, and native acrylic initialization are out of scope.

The split background begins below the titlebar. It consists of two visual roles:

1. A dark left-pane material derived from the existing side-material variables.
2. A right-pane material whose brightest region begins at its left edge and fades continuously into the darker right edge.

The transition is expressed by background layers, not by a border, pseudo-divider, shadow seam, or additional DOM separator.

## Settings Mapping

The Settings window already has a stable two-column layout:

- `.settings-dialog-nav` is the dark left pane.
- `.settings-dialog-main` is the gradient right pane.

The existing 208px column width and all navigation/content layout behavior stay unchanged. The current `border-right` on the navigation pane is removed. Existing navigation selection and hover states remain readable over the darker pane.

## Plugins Mapping

The Plugins window maps its existing workspace to the same two roles:

- the plugin list side of `.plugins-layout` is the dark left pane;
- `.plugins-detail-pane` is the right content pane over the light-to-dark gradient.

The toolbar/status area above the plugin columns remains part of the window body and should visually join the right-side material rather than introducing a third framed surface. Plugin list and detail functionality, sizing constraints, scrolling, cards, controls, and empty/error states remain unchanged.

## Material Integration

The split surface must continue to respond to the existing interface-style controls. Its opacity, tint, blur, and saturation are derived from the established side-material CSS variables, including:

- `--glass-side-fill`
- `--glass-side-fill-strong`
- `--glass-side-blur`
- `--glass-side-saturation`

The implementation may add semantic derived variables for the left-pane darkness and right-pane gradient stops, but it must not introduce fixed opaque colors that bypass interface-style synchronization. The right-side highlight is restrained and neutral so content remains readable and the surface does not become a decorative glow.

## Non-Goals

- Do not change titlebar height, color, bottom divider, controls, or drag behavior.
- Do not change native BrowserWindow acrylic, reveal timing, positioning, or lifecycle.
- Do not add a vertical divider between left and right panes.
- Do not redesign settings controls, plugin cards, typography, spacing, or navigation behavior.
- Do not introduce a second material preference system.

## Verification

Automated verification:

- `npm run check:web`
- `npm run check:desktop`
- `npm run check:ui-design`

Desktop visual verification:

- Open Settings and Plugins as standalone windows.
- Confirm the titlebar remains visually unchanged and retains its bottom line.
- Confirm the body has a dark left pane and a light-to-dark right pane.
- Confirm there is no vertical divider or visible seam between the panes.
- Adjust the side-material opacity, blur, tint, and saturation settings and confirm both windows update consistently.
- Confirm list/content scrolling, pointer highlights, selection states, and window reveal remain unchanged.

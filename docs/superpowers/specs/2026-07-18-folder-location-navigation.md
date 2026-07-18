# Folder Location Navigation

## Goal

Provide one reliable folder-location flow for the titlebar and media thumbnails.
The flow reveals a folder in the lazy tree without changing the active folder,
media selection, or an open preview.

## User-visible behavior

- The expanded-sidebar titlebar keeps the existing utility order and adds a
  folder-location button immediately after Settings. A vertical separator is
  placed between Settings and the location button. The location icon is the
  same icon used by thumbnail location controls.
- When sidebars are collapsed, the left titlebar continues to show only the
  existing sidebar toggle. The titlebar location button is not added to the
  collapsed titlebar.
- In the library view, titlebar location targets the current folder. In an
  open preview, it targets the previewed media's `folderId`. If no target is
  available, clicking is a no-op.
- A thumbnail location control is rendered for every displayed media tile. It
  fades in after 500 ms of tile hover, uses the shared interactive-pointer
  system, and remains inside the thumbnail clipping radius. Its inner corner
  radius inherits the corresponding thumbnail corner radius.
- Clicking a thumbnail location control stops the tile selection/open-preview
  action. If sidebars are collapsed, it reveals them first, then locates the
  folder. It never activates the folder or closes/changes the preview.
- Location reveals the complete ancestor path, expands the required nodes,
  smoothly scrolls the target row toward the viewport center, and briefly
  highlights that row.
- Toggling sidebar visibility reflows the media grid. After the new layout is
  measured, the selected thumbnail is centered in the current viewport where
  scrolling is possible.

## Architecture

The current Core contract exposes folder children but not a way to resolve an
arbitrary media `folderId` to its unloaded ancestors. Add a contract-first
`GET /folders/{folderId}/ancestors` endpoint returning `FolderRecord[]` ordered
from the root folder to the requested folder. The Core implementation should
use a bounded recursive query and return 404 for a missing folder. Add the
matching client method and focused Core/client tests.

`useLibraryData` owns the reveal operation. It should fetch/cache the ancestor
path, merge path records into `folderChildrenByParent` without duplicates,
expand every ancestor, and expose the operation through `LibraryState`. It
must not call any selection setter. The sidebar owns only DOM scrolling and
highlight lifetime, using the existing tree row attributes and sticky-occlusion
layout rather than a second tree implementation.

App owns the target request shared by the titlebar and grid. A request carries
the folder id and a monotonically increasing request id so repeated requests
for the same folder still reveal and highlight. The request remains valid
while the sidebar is being shown after a thumbnail click.

The media tile root must not contain a nested interactive button. Convert the
tile shell to a focusable `div[role=button]` with equivalent Enter/Space,
double-click, context-menu, and selected-state behavior, then render the
location control as a real button inside the thumbnail surface. Preserve the
existing Liquid Glass pointer affordance and do not introduce per-tile glow
logic.

## Failure and boundary rules

- Missing target, stale media, or a deleted folder is a no-op with no selection
  change.
- Ancestor loading failures leave existing tree/selection state intact and do
  not activate a fallback folder.
- A target row that cannot be mounted or a non-scrollable viewport is not forced
  into an invalid scroll position; the highlight is still applied if the row is
  present.
- Repeated or overlapping reveal requests are generation-safe and do not apply
  an old highlight after a newer request.
- The thumbnail control is keyboard accessible and appears immediately for
  focus-visible interaction; the 500 ms delay applies to pointer hover reveal.

## Verification

- Contract and generated client checks pass.
- Core tests cover ancestor ordering, root/leaf boundaries, missing folders,
  and cycle/depth protection.
- Web tests cover path merge/expansion without selection mutation, no-op target
  handling, locate-button event isolation, and selected-thumbnail recentering
  across layout changes.
- Run `npm run check:core-api`, `npm run check:core-client`,
  `npm run check:web`, `npm run check:ui-design`, `npm run check:desktop`, and
  the OS-backdrop visual harness when the implementation is complete.

# Directory appearance and persistent subfolder order

## 1. Scope and user decisions

The reported menu is the top-level RootRecord menu, which previously omitted appearance controls despite ordinary FolderRecord menus supporting them. Both root and ordinary folder menus must expose icon/color editing and subfolder sorting. Roots may be offline or not yet have a rootFolderId; local appearance/sort preferences still work. Provider/reachability state remains visible independently of identity artwork.

The user confirmed that sorting applies to subfolders and must optionally recurse. It changes neither media-file sorting nor the order of the root list. Name ascending/descending uses the existing SQLite BINARY name/id order.

## 2. Typed contract and persistence

RootRecord owns nullable icon/color, updated by `PATCH /roots/{rootId}/appearance` through the same paired value validation as ordinary folders. The ordinary folder appearance endpoint remains separate and does not accept root sentinels. Merge appearance metadata without replacing newer reachability/count state.

`GET/PATCH/DELETE /roots/{rootId}/sort` and `/folders/{folderId}/sort` use typed Core client wrappers. PATCH accepts `{ order: name_asc | name_desc, recursive: boolean }`; DELETE clears the explicit override. DirectorySortState contains target root/folder identity, explicitRule, effectiveOrder, source and an opaque revision. Mutation results also indicate whether loaded descendant branches need invalidation. All mutations require the active context revision and session. Sort writes read the prior rule before changing it, so the existing DbWriter must acquire an IMMEDIATE transaction before that read; a DEFERRED read-to-write upgrade can fail with SQLITE_BUSY under a competing WAL writer. Pure page state/items/counts remain in a read snapshot and must not reserve the writer.

Explicit rules are sparse local catalog metadata keyed by stable root/folder identity. A recursive change is a bounded write, not a rewrite of descendant records. No path rename, operating-system metadata, sync event or file-content mutation is involved. Preferences survive process restart and stable-identity rename/move.

## 3. Inheritance

The directory's explicit rule wins for its own direct children. Otherwise use the nearest applicable recursive ancestor rule, or default name ascending. New folders inherit automatically. A nonrecursive override affects only that directory's direct children; deeper directories can still inherit an older recursive ancestor. Clearing an override restores inherited/default behavior. The menu shows the checked effective order, explicit recursive setting and reset action.

## 4. Pagination and cache ownership

Core orders the complete result before keyset pagination: name/id both ascending or both descending. A page's required sortState records its effective rule. Cursor version, actual parent, includeDescendants mode, order and revision must match; invalid/stale cursors return 400 without mixing orders; scope/revision mismatch uses the explicit stale_folder_cursor code for bounded UI recovery. Ancestry and own/subtree rule revisions distinguish direct, recursive, reset and move changes without invalidating unrelated branches.

The shared folderChildrenByParent cache feeds sidebar and SubfolderStrip. Ancestry hydration must match database Unicode-codepoint/name/id order, never localeCompare or reversal of a single page. Mutations invalidate affected cached pages/cursors and fence old in-flight responses. Reload relevant expanded/selected parents only; do not enumerate all descendants or add a request per rendered node. Keep the flat includeDescendants query independent of recursive preference inheritance. Preserve media selection, layout and scrolling.

## 5. Performance contract

Resolve the effective rule once per parent page with a lightweight bounded ancestry query. Use an index supporting parent/status/name/id filtering and ordering. Count only the page's requested folder subtrees in one bulk query rather than re-counting every descendant of the parent on each page. Counts must remain exact; do not replace them with estimates or add N+1 queries.

Measure the actual production page method with wide, deep and file-populated fixtures, including its count work. Record query plans and reproducible timings with fixture shape and hardware/runtime limits. A standalone handwritten SQL benchmark is insufficient proof of production behavior. Recursive preference updates must remain independent of total descendant count.

## 6. Interaction and regression evidence

Use existing registered menu/submenu and compact anchored appearance popup primitives. Menu behavior must also receive the shared context-menu presentation class; sort selection needs a visible check and fill, not ARIA state alone. The root offline indicator uses its own label-grid column so a custom identity and status retain the standard single-row height. Preserve one thin boundary, the shared pointer highlight, keyboard ownership, draft Apply/Cancel/reset and visible failures. Sort mutations retain the exact failed intent for retry, including reset versus update and requested order. Invalid/stale cursor recovery is bounded and must not mask unrelated failures.

Tests cover actual App root/ordinary context menus, pre-scan/offline root metadata, paired writes, all live-root read fences, >500 siblings with ties, ascending/descending paging without gaps/duplicates, recursive/local/reset/new-child behavior, cross-parent/cross-sort cursor rejection, Library switch and stale responses, persistence and unchanged media behavior. Real isolated Electron/Core and OS menu evidence complement mounted/API/DB checks.

## 7. Correct and incorrect patterns

Correct: a root stores one recursive descending rule; unloaded descendants inherit on demand, one child can override locally, and ordered pages feed both visible folder views.

Incorrect: add only the ordinary-folder menu, reverse each loaded page independently, locale-sort hydrated ancestors against SQL pages, bulk-write every descendant for recursion, or assume a metadata response proves an offline root became reachable.

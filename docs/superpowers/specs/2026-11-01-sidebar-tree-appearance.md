# Sidebar folder selection and colored hierarchy

## 1. Scope

Root and ordinary-folder rows share one content-selection shape that excludes the left expand/collapse slot. Known leaves have no interactive expander. Expanded hierarchy guides communicate nesting using folder identity colors while retaining the current compact acrylic sidebar, sorting, paging and pinned ancestor behavior.

This is renderer presentation/state derivation. Root/folder identity persistence and ordered paging remain governed by directory-preferences and folder-appearance contracts; no new API or filesystem metadata is introduced.

## 2. Expansion evidence

Use existing hasChildren metadata, loaded children, paging cursors, loading/failure state and successful page completion. Positive children, a next-page cursor, loading or a failed/incomplete read keep expansion reachable. Explicit negative metadata or a successful complete empty page can establish a leaf when no positive evidence remains. Missing metadata is unknown, not false. Media-file counts do not describe child folders.

A root without a folder identity has no actionable expansion target and uses a noninteractive spacer; later scan/refresh data can restore expansion. An unloaded eligible directory remains expandable. A failed refresh must neither establish an empty leaf nor reuse a stale completed-empty cache to prevent retry. Retain visible previously loaded data, use existing request/context fences and clear lifecycle bookkeeping on Library changes. Do not probe every node or enumerate a whole subtree merely to decide whether to draw a caret.

When a fresh parent page reports children for a folder whose cached child page was previously complete and empty, retire that contradicted child completion marker. Do not fetch the child immediately; its next expansion uses normal lazy admission. An empty loaded page with a next cursor is not a leaf and must retain expansion/load-more behavior after collapse and reopen.

## 3. Interaction boundaries

Keep 32px rows and 16px depth alignment. A transparent row contains an independent caret/spacer and a content button containing identity, name, offline status and count. Selection, locate emphasis and the global pointer target belong to that content shape. The caret slot is outside its highlight and selection hit area. Keyboard focus remains visible using one thin boundary per control; use the existing pointer-target remapping mechanism rather than another highlight driver.

Caret activation only expands/collapses. Content activation selects and retains its context menu. A known leaf omits the interactive toggle and expansion ARIA state, with a noninteractive alignment spacer where needed. Preserve existing focus, scrolling, locate and provider/offline behavior. Do not invent new sidebar marquee, drag or multi-select semantics.

## 4. Guide colors and geometry

Each caret uses its own folder's identity color, defaulting to white. Child branch guides use the owning parent's identity color; nested folders with other colors own their respective descendant branches. Apply/reset must update carets and guides from current authoritative data without refreshing media.

Use thin 1px decorative vertical segments with muted opacity and pointer-events disabled. Intermediate siblings and ancestor continuations have no horizontal branches. Only the terminal end of a child group gets a short horizontal bend at its final row's midpoint; that parent segment does not continue through the final child's own descendants. A load-more row represents continued siblings, so do not terminate at the last loaded real child; transfer the visible terminal when paging completes. Collapse removes the corresponding descendant guides. Guides are structure marks, not additional control borders or hover effects.

The vertical stroke center must align with the owning parent's actual caret SVG center at every depth. Center the SVG in its fixed disclosure slot with zero native button padding, and account for half the 1px stroke width rather than aligning its left edge to the center. Verify actual element geometry at normal/narrow/125% sizes, not only a shared constant.

Keep segments within the existing row/sticky clipping geometry so pinned ancestors and scrolled children share occlusion behavior. Do not add per-frame measurements or another global controller solely for guides.

## 5. State transitions and failures

Unknown → loading → children/paging or confirmed empty must update the affordance truthfully. Failure preserves retry; adding a child after refresh restores the caret. Cached positive children outrank stale negative hints. Library/context changes must not let an old read or failed-state marker overwrite new folder identity/state. Preserve loaded-window paging and media selection/layout/scroll.

## 6. Verification

Pure and mounted checks cover metadata absent/false/true, empty/full/more pages, loading/failure/retry, new children and root pre-scan/offline states. Verify caret versus content hit areas, selection/locate/focus ownership, no leaf toggle, count placement and alignment. Verify nested parent/child colors, reset, absent intermediate horizontal branches, centered vertical strokes, terminal-only bends, load-more continuation, collapse, sort and sticky clipping.

Actual isolated Electron/Core acceptance uses physical nested folders, independent custom colors and saves/reopen. OS captures validate content-only highlighting and hierarchy geometry. Global scrollbar and popup checks continue separately; no user Library or process is used as a test fixture.

## 7. Wrong versus correct

Wrong: interpret undefined hasChildren or a failed refresh as an empty directory, infer children from a file count, probe every visible node, paint selection across the caret, or draw every branch with the deepest child's color.

Correct: derive expansion from explicit evidence; keep unknown/error recovery usable; select only the content shape; draw a parent-colored branch with separately colored child carets and descendant branches.

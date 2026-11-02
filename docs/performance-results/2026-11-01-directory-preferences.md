# Directory menus, recursive sorting and query performance — 2026-11-01

## Delivered scope

The screenshot matched App.buildRootItems, which had no appearance action. RootRecord now owns icon/color independently of a scan-created sentinel. Both root and ordinary folder context menus expose the existing compact appearance popup and a shared sorting submenu. Offline state remains separate from custom identity artwork and stays on one standard-height row.

Sorting is explicitly limited to subfolders: name ascending/descending, a recursive rule option and reset to inherited/default order. Each directory is independent; new directories inherit the nearest applicable recursive ancestor, while local overrides remain possible. A nonrecursive intermediate rule changes only that directory's direct children. Media-file sorting and the order of the root list are unchanged.

Rules are sparse catalog metadata. One recursive change updates one rule and its target revision, without enumerating descendants. Ordered Core keyset pages feed sidebar and SubfolderStrip. Cursors bind parent, mode, direction and rule/lineage revision. The UI fences old replies, recovers once for explicit stale_folder_cursor errors, retains failed intent for retry, and atomically replaces the previously loaded window so a sort does not shrink it temporarily or move the media viewport.

## Production query measurements

Measured the actual Database::list_folder_children_page, including effective-rule resolution, item query and exact file/media counts. Windows debug build, in-memory SQLite, 200 directories per page, one file per sibling. The same fixtures and 4 first-page plus 4 next-page samples were used before/after; these are query observations, not end-to-end release UI guarantees.

| Fixture / page | Previous median | New median |
| --- | ---: | ---: |
| 10,000 siblings / first | 24.9235 ms | 0.9035 ms |
| 10,000 siblings / next | 23.9825 ms | 0.9915 ms |
| 50,000 siblings / first | 125.5405 ms | 1.0295 ms |
| 50,000 siblings / next | 119.8740 ms | 0.9050 ms |
| 10,000 siblings + depth 256 / first | 24.1135 ms | 1.3525 ms |

With 10,000 siblings plus a 256-level nested branch, previous first-page samples were 23.933–24.387 ms; new samples were 1.053–1.959 ms. The exact count of 257 was preserved.

The new query uses idx_folders_parent_status_name_id for filtering/keyset order, with no item-order temporary sort. Exact counts visit only the current page's folder subtrees in one bulk query. At both 10k and 50k siblings, measured item-query VM steps were 5,442 first/6,262 next, counts used 13,830 steps and both reported zero fullscan steps. Deep selected subtrees still cost work proportional to their actual contents; the optional flat includeDescendants path is separate from the optimized direct-child path.

Evidence: .tmp/folder-menu-sorting/perf-before.log, perf-after.log and core-report.md. The explicit regression also proves a recursive rule update changes exactly two rows in a 10k-sibling catalog.

## Findings fixed during independent review

- Corrected exact retry intent for failed descending/reset actions and retained failed descendant reloads across no-op mutation retries.
- Recovered stale cursors without leaving Load more busy; preserved and atomically replaced loaded rows rather than shortening a 620-row view to its first page.
- Applied the shared context-menu presentation and visible check/fill states; bare menu behavior alone had rendered default inline browser buttons.
- Added a dedicated offline-status grid column so root labels remain one row.
- Replaced a synthetic live-root API fixture with physical folders plus an awaited real scan, and provisioned the second Library before first activation to isolate cross-context behavior. Existing schema-bootstrap expectation was updated to current version39.
- A channel/barrier regression reproduced SQLite error5 on the new DEFERRED read-to-write preference transaction. Sort set/reset now reserve IMMEDIATE writer admission before reading old state, following the existing writer pattern. Root appearance already writes first and passed the same competing-writer coverage. The original intermittent HTTP500 response body was unavailable; the reproduced locking defect is independently proven and fixed. Three key cases passed ten consecutive combined runs.
- An unchanged Desktop health-timeout test could wait forever if its 50ms deadline fired before TCP acceptance under load. The fixture now uses a controlled deadline after real connection acceptance and bounded cleanup; production Desktop behavior was unchanged.

## Final verification

Structure, Core API/schema, UI design, TypeScript/boundary checks and Core client 70 tests passed. Final full Web passed 516/516; Desktop passed 233/233 plus eight development-URL checks. Rust formatting and the complete isolated workspace passed: 876 Core plus 13 other tests, zero failures and 14 ignored. Seven directory preference tests, 33 prior folder regressions and the explicit production benchmark also passed.

Real isolated checks on the final release covered 620 siblings with Unicode names, two UI pages, ascending/descending order, scoped/stale cursor rejection, recursive/local/nonrecursive/reset/new-directory behavior, and three rejected stale-context mutations after switching Libraries. Actual App menus, sidebar/strip agreement and media order/selection/exact scroll preservation in all four layouts passed. Final release to final release process restart confirmed root/offline appearance and local/inherited sorting persistence. OS captures verified root/folder menus and offline appearance in dark/light backdrops.

All required gate components passed separately. The umbrella npm test command was not rerun against the locked default Cargo target. First attempts exposed the recorded fixture problems; decoder tests in the isolated Cargo target also needed copies of existing trusted bundled fixtures. No earlier failed run is counted as a pass. Linux/Docker, packaged release acceptance and physical-display antialiasing were not performed.

## Build and evidence

Final Core release copy: .tmp/folder-menu-sorting/bin/mangoo-core.exe, SHA256 BB97128868BDC4E56860D98D29E92C23D6E351C639BD2BE74C5DE7717643D698; release-build-final.log passed without compiler warnings. The earlier candidate6127a77f build predates the writer transaction fix and is not the final acceptance binary.

Native evidence directory: .tmp/folder-menu-sorting/folder-sort-review-1793527309059/. OS captures include root-sort-popup-os-dark.png, ordinary-folder-sort-popup-os-dark.png and offline-root-appearance-os-dark.png, plus light counterparts. They establish material/layout/state; physical-display antialiasing is not certified.

Baseline: .tmp/task-baselines/20261101-folder-menu-sorting, 358 dirty paths. Unrelated WIP and staging were preserved. No user Library, running user process or source media was changed. The default target/debug Core executable was locked by an existing process, so complete Rust verification used an isolated Cargo target. All owned native processes were closed; protected user processes remained running. Final evidence and delta accounting are recorded in .tmp/folder-menu-sorting/check-report.md and task-delta.json. No staging, commit or publication was performed; the broader all-file task remains in progress.

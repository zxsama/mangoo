# Windows startup diagnostics — 2026-10-31

## Scope

Fix warnings and errors when starting Mangoo through the repository's desktop scripts. Preserve the existing all-file Library, dropdown, badge and hover changes. All runtime verification uses temporary Host/Library/Electron profile directories; no user Library data is used.

## Repairs

- Remove 12 groups of Core unused-code warnings by restricting test-only helpers and removing proven unreferenced code. Preserve production health checks, deadlines, cancellation, stream validation and decoder identity handling.
- Prefer Vite's default port, but select a bindable port in 49152–65535 when the implicit default is occupied or reserved. Windows currently reserves the range containing 5173. Apply the browser-usable port constraint to automatic Core allocation as well. Explicit addresses remain unchanged.
- Verify that a reused server is actually Mangoo, bound readiness waits, and directly own Vite/Electron children so closing Desktop does not leave the development server running.
- Gate Library catalog and thumbnail work on the authoritative active-Library snapshot. First-run HTTP failures fell from eight `409 no_active_library` responses to zero. Preserve switching/revision fences, genuine conflict reporting and refresh recovery after discovery failures.
- Omit an absent BrowserWindow icon instead of passing `icon: undefined`, eliminating Electron's native image conversion warning while preserving packaged icons.
- Share the available-port allocator with mounted Web tests. Supply authoritative active-Library snapshots in affected fixtures; preserve existing assertions.

## Verification

| Check | Result |
| --- | --- |
| Core production build and test compilation | Pass, zero compiler warnings |
| Full Core crate tests | 854 unit and 4 integration passed; 13 ignored |
| Web gate | 495 passed, no skipped/cancelled tests |
| Web production build | Pass |
| Desktop gate | 233 tests passed after the final icon fix |
| Launcher behavior tests | 8 passed |
| UI design / structure | Pass; exact icon-only option parsing also passed positive/negative guard tests |
| Actual development CMD | Port fallback, first-run rendering, zero HTTP/console diagnostics, normal exit and no owned process/listener survivors |
| Actual production BAT | Rebuilt package starts; warm launch skips build and reuses the same instance; normal close and forced-parent exit clean up Core |

Final development verification also enabled Electron's verbose logging before attaching CDP: the native icon warning was absent. An earlier fixture that attached CDP before navigation recorded an Electron sandbox startup-data error; the delayed-attachment run did not. This is consistent with the initial-document debugger mechanism described in [Electron issue 54149](https://github.com/electron/electron/issues/54149), whose upstream reproduction concerns extension/offscreen documents. No Electron upgrade, sandbox weakening or error filtering was used.

## Evidence and limits

- `.tmp/startup-diagnostics/core-report.md`, `web-report.md`, `runtime-report.md` and `check-report.md` contain commands, counts, owned paths and detailed results.
- Preserve the original compiler, reserved-port, unsafe-port and stale-fixture failures separately from final successful runs. They were not relabeled as passing results.
- `.tmp/task-baselines/20261031-startup-diagnostics/` records the initial 271-path dirty tree. `.tmp/startup-diagnostics/task-delta.json` identifies the incremental scope.
- 251 original dirty paths remain byte/deletion-state identical. The accepted CSS, dropdown components and video-hover implementation are unchanged. Release packaging regenerated plugin trust through the normal builder; that generated change is recorded separately in runtime evidence.
- This is scoped Windows startup validation, not full repository `npm test`, Docker/Linux, OS acrylic composition or release publication. No source commit or task archive was requested.

# Overlay Regional Mask Edge Alignment Implementation Plan

> **For Codex:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Status:** Implemented and fully verified in the isolated worktree on 2026-08-10; final integration handoff remains Task 5.

**Goal:** Keep every rounded regional blur mask inside the exact, un-inset popup outer border box and permanently cover both real titlebar popup paths at 100% display scale.

**Architecture:** Preserve the current synchronous app-root regional SVG filter. Clip each thresholded rounded mask back to its own exact source flood with a final `feComposite operator="in"`, then feed only that clipped result into mask unions and the existing inside/outside image composites. Keep the surface border, gradient, geometry, and elevation shadow unchanged.

**Tech Stack:** React 18, TypeScript, SVG filter primitives, Electron 43, Node test runner, CDP desktop visual harness, PowerShell/System.Drawing pixel analysis.

---

## Task 1: Freeze the live graph contract with a failing test

**Files:**

- Modify: `apps/web/tests/fixtures/runOverlayRegionalBlurLifecycle.mjs`
- Modify: `apps/web/tests/overlayRegionalBlurLifecycle.test.mts`

- [x] Extend the mounted fixture snapshot with the rounded alpha result, final source-clip primitive, its `in`, `in2`, `operator`, result id, and downstream mask inputs.
- [x] Assert the source flood still equals the complete surface outer border box and the intermediate Gaussian bounds remain expanded.
- [x] Mount at least two rounded masks and assert every chain clips against its own source before the union; assert the union consumes only clipped results.
- [x] Mount a radius-zero mask and assert it contributes the exact source flood without a rounding or clip chain.
- [x] Run `node --experimental-strip-types --test apps/web/tests/overlayRegionalBlurLifecycle.test.mts` and record the expected RED failure against the current implementation.

## Task 2: Clip rounded alpha support without changing geometry

**Files:**

- Modify: `apps/web/src/design/overlays/overlayRegionalBlur.ts`
- Modify: `tools/checks/validate-ui-design.mjs`

- [x] In `appendRoundedMaskPrimitives`, give the thresholded alpha an explicit temporary result.
- [x] Append `feComposite(in=rounded-unclipped, in2=source, operator=in)` and return its clipped result.
- [x] Keep source `x`, `y`, `width`, `height`, radius, Gaussian sigma, expanded intermediate bounds, radius-zero handling, and union behavior unchanged.
- [x] Add a UI-design guard for the final source-clip topology without weakening existing regional-filter checks.
- [x] Re-run the focused lifecycle test and `npm run check:ui-design`; both must be GREEN.

## Task 3: Replace static-only alignment evidence with real 100% popup evidence

**Files:**

- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`

- [x] Add a 100%-only capture for the real Library switcher and Notifications titlebar triggers.
- [x] Insert a deterministic, pointer-inert high-frequency diagnostic layer inside the real `#root`, beneath the portal and across all four popup edges.
- [x] Wait for open state, no transform, stable natural geometry, and a settled mask; do not override popup position or size.
- [x] Keep each popup mounted and capture `zero-before/default/zero-after` while changing only the regional dialog-blur value; require identical rect and material-style snapshots.
- [x] Analyze straight-edge bands at physical scale `1` with RGB threshold `3`: every 2–5px inside band must have mean delta `>= 4.0` and `>= 60%` classified pixels, every first outside pixel must have `0` classified pixels.
- [x] Require every zero-before/zero-after control band to have mean delta `<= 0.5` and `0` classified pixels, so the retained shadow/time noise cannot masquerade as regional output.
- [x] Mutation-test the pixel oracle on the same mounted popup: temporarily rewire downstream consumers from the clipped mask result to its unclipped rounded input, require at least one classified outside edge for each left/right popup, then restore the clip and require all four outside counts to return to `0`.
- [x] Derive the expected outer box independently from live surface/root rectangles and include source-flood equality in each evidence record, explicitly proving no mask inset.
- [x] Close each popup through the real dismissal path and verify regional graph cleanup.

## Task 4: Correct durable evidence and run full verification

**Files:**

- Modify: `docs/superpowers/specs/2026-08-09-overlay-frosted-material-and-radius-unification-design.md`
- Modify: `docs/superpowers/plans/2026-08-10-overlay-in-app-acrylic-sampling-fix.md`
- Modify: `docs/superpowers/specs/2026-08-10-overlay-regional-mask-edge-alignment-design.md`
- Modify: `docs/superpowers/plans/2026-08-10-overlay-regional-mask-edge-alignment.md`

- [x] Mark the old static outline evidence as border-ownership evidence only and point real regional containment to this follow-up.
- [x] Run the focused lifecycle test, web tests/typecheck, `npm run check:ui-design`, `npm run check:web`, and `npm run check:desktop`.
- [x] Run the OS-backdrop desktop harness with fresh output and confirm the real left/right 100% edge results plus the existing 100%/125%/150% matrix.
- [x] Run `npm test` and `git diff --check` from the isolated exact tree.
- [x] Confirm no harness Electron/Core/Vite process remains.
- [x] Ask the same specification reviewer to perform final quality review; resolve every P0/P1 finding.

### Verification record

- `node --experimental-strip-types --test apps/web/tests/overlayRegionalBlurLifecycle.test.mts` — exit `0`, `12/12` passed.
- `npm --workspace @mangoo/web run test` — exit `0`, `398/398` passed; `npm --workspace @mangoo/web run typecheck` — exit `0`.
- `npm run check:ui-design` — exit `0`; `npm run check:web` — exit `0`, including `398/398`; `npm run check:desktop` — exit `0`, including `222/222`.
- `$env:MANGOO_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs` — exit `0`, run `mangoo-visual-msmfm6a1-66048`, `run.ok=true`, `hardFailures=[]`, popup/modal scale gates passed at `1`, `1.25`, and `1.5`.
- At 100%, Library-left normal/legacy/restored outside counts were `[0,0,0,0]`, `[155,155,427,427]`, `[0,0,0,0]`; Notifications-right were `[0,0,0,0]`, `[151,151,327,327]`, `[0,0,0,0]`. Both source-flood equality, invariant snapshots, configured-blur mutation frames, zero controls, and cleanup gates passed.
- The retained real-content gate passed over six real images and three DOM text rectangles: image boundary energy `40.3865 -> 0.3793 -> 0.0887`, text edge energy `19.2672 -> 0.3324 -> 0.1271`, reference deltas `0/0`.
- `npm test` — exit `0`; Rust `472 passed, 0 failed, 2 ignored`. `git diff --check` — exit `0`.
- Harness startup/finalization recorded child close and closed stdout/stderr streams. Ports `5179` and `9500` had zero listeners afterward; the only matching worktree Node processes were the same three pre-existing editor TypeScript servers, so no Electron/Core/Vite process was terminated.
- Full command logs: `.tmp/task4-verification-20260810-072452/`; harness summary: `.tmp/visual-check/logs/desktop-ui-regression-summary.json`; harness stdout/stderr: `.tmp/visual-check/logs/desktop-ui-regression-mangoo-visual-msmfm6a1-66048.stdout.log` and `.tmp/visual-check/logs/desktop-ui-regression-mangoo-visual-msmfm6a1-66048.stderr.log`.

## Task 5: Commit and hand off safely

- [x] Inspect `git status --short` and stage only the files listed in this plan.
- [x] Preserve the six reviewed implementation/test/documentation commits from the isolated branch instead of squashing their TDD history.
- [x] Verify the user's 29 unrelated dirty entries do not overlap the nine touched paths, then cherry-pick only `e166bb5..53b2399` onto `perf/million-thumbnail-hotloop`.
- [x] Verify the integrated commit tree equals the fully tested isolated tree and re-run the focused overlay lifecycle test (`12/12`); retain physical-monitor inspection as the final anti-aliasing confirmation.

### Handoff record

- Integrated commits: `701cb43`, `59bf9c6`, `fe9f681`, `f8a33b5`, `18bd1ce`, `44387b0`.
- Isolated and integrated committed trees both resolve to `cca106b5452eefe89a03402c8ec2a570f44395ce`.
- The active workspace remains on `perf/million-thumbnail-hotloop`; its pre-existing 29 dirty entries are unchanged and none overlap this change.
- Post-integration focused lifecycle verification: exit `0`, `12/12` passed.

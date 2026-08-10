# Handoff resolution audit: `perf/million-thumbnail-hotloop`

Date: 2026-08-05
Branch: `perf/million-thumbnail-hotloop` (380 commits ahead of `master`, not merged)
Author of this note: the agent that built the media decode/preview pipeline, the
render-settings panel, the preview scrubber, and the navigation-gate fix.

This document began as an unresolved-issue inventory. The resolution summary
below is authoritative for the current worktree; the original issue detail is
retained afterward as historical context and reproduction evidence.

One item is deliberately excluded per the requester's instruction and is tracked
elsewhere: an unrecoverable revert of another agent's uncommitted
`apps/web/src/features/file-ops/MoveDialog.tsx` edits.

---

## Resolution summary (2026-08-05)

| # | Status | Resolution / evidence |
|---|---|---|
| 1 | Fixed | Replaced the literal SOH byte with regex backreference `\1`. A mutation to `requestMediaWindow(index, index + 1)` now makes `check:web` fail with the intended message; restoring the neighbourhood request passes. Control-byte count is zero. |
| 2 | Fixed | All preview-side landings now go through `ensurePreviewMediaAtIndex`, which resolves an absolute slot or requests a clamped 49-item neighbourhood. Step navigation and scrubber jumps share this loader. |
| 3 | Fixed | `resolvePreviewMediaWindow` preserves the absolute `mediaSlots` index and never substitutes `library.media`; the loaded-array index is kept separately for prefetch only. Tests cover a loaded window at slots 124–126 with selection at absolute slot 125. |
| 4 | Fixed | Every grid tile now exposes `data-media-index` in both linear and positioned render paths. `check:web` guards both prop paths and the DOM attribute. |
| 5 | Implemented; real camera sample still needs acceptance | Added the pure-Rust `psd` and `rawler` decoders. Camera extensions `.cr2/.nef/.arw/.dng/.orf/.rw2` are indexed and routed; `.raw` remains supported. A generated valid PSD decodes to RGBA. No redistributable real camera RAW binary was available locally, so real-file colour/output acceptance is not claimed. `rawler 0.7.2` is LGPL-2.1 and needs release-license review before distribution. |
| 6 | Fixed | Migration 26 adds a separate `preview_failures` negative-cache table. Permanent failures are keyed by file/profile/source size/source mtime; changed sources retry. Successful blobs and failures replace one another. Transient network, lock, OOM, Windows sharing-violation, and stat/decode race errors are not cached. Preview decoding was already bounded by the Core semaphore; the original “unbounded” statement was stale. |
| 7 | Automated integration verified | A test now fills the real process-wide `previewBitmapPool`, invokes the production library-switch cache-reset entrypoint, and proves the `ImageBitmap` is closed and pool bytes drop to zero. Removing the production listener registration makes the test fail. A physical GPU-memory observation across two user libraries remains optional manual evidence, not a known code defect. |
| 8 | Manual visual acceptance only | No code defect was found: the renderer state and CSS mode remain covered by the existing browser checks. Chromium screenshots cannot judge realtime resampling on the physical display, so 800% nearest-vs-bilinear appearance remains a human acceptance item. |
| 9 | Far-jump defect fixed; feel remains manual | A missing scrubber target is now recorded and selected when its absolute slot arrives; far jumps request neighbours through the same loader as step navigation. Pure tests cover 20,000-item mapping and slot-125/220 landing. Flattened mode uses the same `mediaTotalCount`/`mediaSlots` contract. Long-drag smoothness remains subjective physical-display acceptance. |

The automated changes are covered by focused Web/Rust/schema tests and the
repository gates recorded in the final branch-state section. Items 5, 8, and 9
retain only the explicitly stated external-sample or human-judgement checks.
The general desktop visual harness starts the isolated application successfully
but is currently blocked before its preview pass by stale selectors from the
`SearchBar`/compact-popover refactors; this is not counted as a visual pass.

## Follow-up hardening (2026-08-10)

A later review found three gaps in the repaired implementation. They are now
closed in the current worktree:

1. The in-process decoder pixel ceiling is shared by `image_crate`, PSD, and
   Rawler. PSD checks parsed dimensions before `document.rgba()`. Rawler performs
   a dummy metadata decode, rejects an unsafe raster before the full raw decode,
   and checks the decoded dimensions again before development.
2. A matching `preview_failures` row suppresses repeated preview work for at
   most five minutes. After 300 seconds the unchanged source is tried again, so
   decoder installation, permission recovery, and other environment changes no
   longer leave the preview permanently poisoned; thumbnail-task retry policy is
   intentionally unchanged.
3. Pending far-jump preview selections now carry the media-page generation from
   `useLibraryData`. Closing preview clears the pending selection immediately,
   while a closed preview or a mismatched media generation discards any late
   arrival instead of changing the current grid selection.

Regression coverage observes each missing behavior before the fix and then
passes with the implementation: decoder budget tests, fresh-versus-expired
preview failure reuse, and closed/mismatched/waiting/arrived preview-selection
states. Focused decoder, preview-route, and Web gates are green. `npm test`,
`cargo fmt --all --check`, and `git diff --check` also exit 0 on the follow-up
worktree; the final branch verification is recorded below.

---

## Historical issue detail

> The sections below preserve the original handoff wording. Treat commands and
> “still broken” statements as historical; current status is the table above.

## 1. Dead structural guard — `tools/checks/validate-web-client.mjs:1997`

**Severity: medium.** A working fix is unprotected against regression.

The guard meant to prevent one-item preview landings can never fire:

```js
if (/requestMediaWindow\(\s*(index|targetIndex)\s*,\s*\x01\s*\+\s*1\s*\)/.test(app)) {
```

That `\x01` is a literal SOH control byte, not a regex backreference. Writing
`\1` through nested shell→Python quoting had it interpreted as a character
escape. The pattern matches nothing, so reintroducing the bug passes `check:web`
silently.

**Verify it is still broken:**

```bash
python -c "import io; s=io.open('tools/checks/validate-web-client.mjs',encoding='utf-8').read(); print('control bytes:', s.count(chr(1)))"
```

**Repair:** replace the byte with `\\1` in the regex literal. Use an editor or a
heredoc'd script with no nested quoting — that is what corrupted it. Then prove
the guard works by mutation:

```bash
# 1. Revert App.tsx's landing to a single item:
#    library.requestMediaWindow(index, index + 1)
# 2. npm run check:web   -> MUST fail with
#    "Preview landings must request a window with neighbours, not a single item"
# 3. git checkout -- apps/web/src/app/App.tsx   (safe: the fix is committed at 979289b)
# 4. npm run check:web   -> MUST pass
```

I stopped mid-repair because another agent was concurrently writing that file
and my edits were being clobbered. Do this when the file is uncontested.

---

## 2. Nothing extends the media window during preview navigation

**Severity: low (latent).** Root cause of the bug fixed in `979289b`.

`MediaGrid` grows the loaded window as you scroll (`onRequestMediaWindow` at
`MediaGrid.tsx:1409`). The preview never did. Stepping now handles it
(`stepPreview` in `App.tsx` requests a window and selects the slot on arrival),
but the asymmetry stands: any *other* preview-side consumer of `mediaSlots` can
still run off the loaded edge with no loader behind it.

Consider making window extension a property of the media-window layer rather
than of each consumer.

---

## 3. `orderedPreviewMedia` silently switches lists

**Severity: low (latent).** `apps/web/src/app/App.tsx`

```ts
const orderedWindowMedia = orderedMediaSlots(library.mediaSlots);
return orderedWindowMedia.length > 0 ? orderedWindowMedia : library.media;
```

When `mediaSlots` is empty this falls back to `library.media`, so the list
navigation indexes into can be a *different* list than the one the title and
scrubber count from (`mediaTotalCount`). Same inconsistency class as the fixed
bug: a correct-looking label over a wrong list.

---

## 4. Grid tiles carry no index attribute

**Severity: low (diagnosability).**

Tiles expose `data-media-id` but no `data-media-index`. Loaded slot indices
therefore cannot be read from the DOM, which is why the 124/220 bug could not be
pinned from instrumentation and needed the user's live numbers. Adding
`data-media-index` to rendered tiles would make this whole bug class observable.

---

## 5. PSD and RAW still do not decode

**Severity: known scope limit, by design.** Deferred in
`docs/superpowers/specs/2026-07-24-media-decode-preview-pipeline-design.md` §2.

- `.psd` is claimed speculatively by `FfmpegDecoder`; ffmpeg handles some
  flattened PSDs and fails the rest.
- `.raw` is indexed but claimed by **no** decoder, so it always reports
  `no decoder handles ".raw"`.
- `.raw` is also not a real camera-RAW extension. The actual ones
  (`.cr2/.nef/.arw/.dng/.orf/.rw2`) are neither indexed nor claimed, so
  CLAUDE.md's "RAW/PSD/HEIC need WebP" rule is vacuous for RAW.

Proper fix is the deferred pure-Rust `psd` / `rawler` decoders. `decode/formats.rs`
is now the single place to register them.

---

## 6. No negative cache for undecodable previews

**Severity: medium on a library with many undecodable files.**

`serve_cached_preview_webp` (`crates/core/src/api/routes.rs`) degrades correctly
on failure — it streams the original instead of 500ing — but records nothing. A
permanently undecodable file re-runs generation on **every** request, which for
`.heic`/`.psd` means an ffmpeg subprocess spawn per request.

Bounded today only by the renderer's `MAX_CORE_INTERACTIVE_REQUESTS = 2`, and
**not bounded at all** in Web/Docker multi-client mode. A `failure_kind` column
on `preview_blobs` would fix it; that is new design, not a review fix.

---

## 7. Library-switch bitmap release never exercised in the running app

**Severity: low-medium.** Unit-tested only.

`releasePreviewBitmaps` (`previewBitmapSource.ts`) is covered by
`apps/web/tests/previewBitmapPool.test.mts`, but no second library was configured
on this machine, so the real teardown path never ran. `ImageBitmap`s do not free
themselves and the next library reuses the same file ids, so a leak here would
show as growing GPU memory plus cross-library image bleed.

**To verify:** configure a second library, open a preview, switch libraries, and
confirm heap does not retain the previous library's frames and no stale image
appears.

---

## 8. Nearest-vs-bilinear not visually confirmed at high zoom

**Severity: low.**

`image-rendering` is provably applied (`docs/benchmarks/2026-08-02-preview-render-browser-checks.md`),
but whether it *looks* right at 800% zoom was not confirmed. Per CLAUDE.md, CDP
`Page.captureScreenshot` does not reflect Chromium's realtime resampling, so this
needs a human on the physical display.

Also recorded there: `CSS.supports("image-rendering", "high-quality")` is
**false** on Chromium 150, which is why only two filter options ship. Re-check if
Electron is upgraded — a third option becomes honest only if it renders
differently from `auto`.

---

## 9. Scrubber untested in two real configurations

**Severity: low-medium.** `docs/benchmarks/2026-08-02-preview-scrubber-verification.md` §"Not verified"

- **Flattened subtree mode.** `showChildFolderContents` was never toggled. The
  scrubber reads the same `mediaTotalCount`/`mediaSlots` pair in both modes and
  should need no special case, but that was reasoned, not observed.
- **Far jumps past the loaded window.** The 31-image benchmark folder loads
  entirely, so the scrubber's `requestMediaWindow` path never fired there. It
  *has* since been exercised indirectly by the 220-image navigation fix, but not
  under a deliberate long drag.
- **Large-folder drag feel.** The ~25-images-per-pixel step is unit-tested at
  20 000 images; whether a long drag feels smooth is a human judgement.

---

## Branch state

- The branch has not been merged, pushed, staged, or committed by this repair.
- `npm test` exits 0 on the repaired worktree. The Rust gate reports 481 passed,
  0 failed, and 2 ignored; structure, contracts, Core client, notification
  contract, Desktop, Web, UI design, schema, formatting, and workspace tests all
  complete successfully.
- `git diff --check` and `cargo fmt --all --check` exit 0. Serena reports no
  error/warning diagnostics in the touched App, MediaGrid, preview-window, Core
  route/DB, PSD, or Rawler files. The SOH-byte count in
  `validate-web-client.mjs` is zero.
- The isolated desktop harness proves Electron, Core, the renderer, and CDP
  start, but it then stops on stale `.search-bar-input` and compact-popover DOM
  selectors before the preview scenario. Its result is **blocked**, not passed;
  800% resampling and large-folder drag remain the manual checks stated above.
- Unrelated pre-existing edits and untracked probe scripts remain untouched.
  Do not use `git add -A` or restore files outside this repair's explicit scope.

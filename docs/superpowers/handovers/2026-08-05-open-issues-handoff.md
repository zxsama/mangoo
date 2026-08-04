# Handoff: open issues on `perf/million-thumbnail-hotloop`

Date: 2026-08-05
Branch: `perf/million-thumbnail-hotloop` (380 commits ahead of `master`, not merged)
Author of this note: the agent that built the media decode/preview pipeline, the
render-settings panel, the preview scrubber, and the navigation-gate fix.

This lists **only unresolved items**. Everything else on the branch is committed,
gated by `npm test`, and recorded in `docs/benchmarks/`.

One item is deliberately excluded per the requester's instruction and is tracked
elsewhere: an unrecoverable revert of another agent's uncommitted
`apps/web/src/features/file-ops/MoveDialog.tsx` edits.

---

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

- `npm test` passes on my work. Two `file-operation overlay` test failures in the
  working tree belong to a concurrent agent's in-progress migration, not to this
  branch's committed code.
- The branch has **not** been merged or pushed. It carries ~327 commits of the
  repo owner's prior work plus this session's; the merge/PR decision is still
  open.
- The repo owner keeps uncommitted probe scripts (`tools/dev/_*.mjs`) and two
  docs in the working tree. **Never `git add -A`**, and never `git checkout` a
  file you did not personally modify — the second rule exists because I broke it.

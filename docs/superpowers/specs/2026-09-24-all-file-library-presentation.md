# All-file library presentation

## Scope and ownership

Ordinary local and synchronized roots collect regular files independently of media decoding. Core owns file identity, queries, reads and generated images; `@mangoo/core-client` owns typed transport; Web owns presentation and interaction. This extends the 2026-09-16 all-file sync specification without changing its source ownership, sync identity or no-follow rules.

No decoder is required to browse, search, count or manage a file. Files without content images use bundled format artwork and open the same static artwork in the preview. Supported original images/videos retain their original preview paths. Format badges omit JPG/JPEG/PNG and otherwise show the normalized extension, including a FILE label for extensionless files. Badge colors and offline artwork are shared across grid, inspector, preview and folder covers.

## Signatures

- `MediaRecord.presentation`: `previewKind` (`original-image`, `original-video`, `static-thumbnail`), `thumbnail` (`available`, `unavailable`), and local numeric capability `generation`.
- `ThumbnailResponse.state = unavailable` is a derived terminal result. It is not inserted into the persisted `thumbs.state` CHECK constraint.
- `RootRecord.fileCount` and `FolderRecord.fileCount` count ordinary files. `mediaCount` remains media-only; `filesSeen` accompanies `mediaFilesSeen` in scan/task progress.
- Migration 0036 updates collection/FTS policy and adds a restartable FTS keyset cursor. Migration 0037 records decoder presentation identity.
- Authenticated POST `/api/media/{fileId}/playback-sessions`, POST `/api/playback-sessions/{sessionId}/renew` and DELETE `/api/playback-sessions/{sessionId}` manage one-source playback grants. GET/HEAD `/api/playback/{capability}` is the only capability-read route.
- Decoder contract: [file-decoder.v1](../../../contracts/plugins/file-decoder-v1.md). Example: [PPM decoder](../../../crates/ppm-decoder/README.md).

## Behavioral contracts

Valid files in enabled roots are the browse/search/position candidate set. `other` includes files without a media row. Scan/watch/import and upgrades preserve IDs, tags, favorites and synchronization identities. Collection upgrades complete only after successful enumeration; FTS backfill uses bounded restartable batches. Source I/O and decoder work do not hold the database writer.

Content capability and cached bytes have different identities. Plugin lifecycle changes refresh media and folder queries, including already selected media. The existing plugin broadcast supplies the fast path; one shared monitor of decoder lifecycle state through `listPlugins` reconciles asynchronous activation while Library consumers are subscribed. It does not invoke Provider or decoder work. Cancellation and request generation prevent an old query from restoring stale records. Verified generated images can survive disabling their decoder; changed source content invalidates those images.

Capability refresh reaches rendered records in both virtualized and fully loaded small grids; changing a page-request callback alone is insufficient if a non-windowed layout never calls it again. Watch-driven changes to a complete unfiltered root list also reconcile the sidebar root count with one bounded roots read, preserving navigation and selection.

Available static-file previews use Core's generated-image preview endpoint through the image render lane. Their byte-cache identity includes source identity and decoder capability generation, but excludes thumbnail task state so polling does not repeatedly reload the same preview. Unavailable static files remain on bundled artwork without requesting source bytes.

Format badges use 80% overall opacity and a uniform 90% visual scale with a top-left transform origin. Their border's accent contribution is 40%, reduced from 70%; text, category colors and the JPG/JPEG/PNG exclusion policy stay unchanged. These values apply to the shared small badge, not to the preview image or full fallback artwork.

The inspector badge anchors 5px inside the actual displayed image or fallback artwork's upper-left corner, not the surrounding square preview stage. Keep the anchor scoped per preview instance. Position follows image loading/source replacement and sidebar resize without adding a separate measurement/polling loop; landscape/portrait letterboxing must not move the badge into empty space. The central preview's existing placement remains governed by its own layout.

Retain the original stage-relative truncation limit before applying the 0.9 transform, and also constrain the badge to the displayed content bounds. In the anchored inspector rule, use the existing size container's `100cqw - 34px` limit instead of a percentage whose resolution can change with anchor sizing. A long badge in a narrow sidebar must shrink from its previous visible width rather than grow because its clipping limit changed. While the image request is pending, the current fallback artwork supplies the anchor.

Eligible pointer entry immediately starts the existing playback-lease setup, without an intentional hover delay; network/metadata/decoding can still determine the first-frame latency. Playback remains muted at 1x. Horizontal movement beyond 2 CSS px of jitter tolerance immediately updates displayed progress to the latest pointer ratio and, when duration is valid, the corresponding time. This presentation is independent of decoded media time. The existing horizontal-hover scrubbing interaction does not require an extra click or drag capture.

Coalesce media seeks through requestAnimationFrame, permit one in-flight seek, and retain only the newest queued target. Older timeupdate/seeked work must not overwrite a newer displayed target; a completion at the wrong submitted time cannot release the latest optimistic presentation. After about 120 ms without effective horizontal movement, resume playback once the pending seek completes. Stop playback sampling while paused/scrubbing and cancel scheduled work on exit.

Before metadata arrives, keep normalized progress responsive without inventing an absolute time. Apply the latest target when valid metadata arrives; invalid duration must not leave idle playback stuck. Progress/time render independently of loadeddata readiness once the playback source exists, while only the video image waits for its first frame. The thin progress rail and time label remain inside the thumbnail. Time opacity is 0.9, and its effective font size matches the format badge after the badge's 0.9 transform: 9px in adaptive/waterfall/grid and 7.2px in list mode.

One owner admits playback. Leaving, scrolling, opening preview, losing visibility/focus, switching Library, changing source or unmounting ends playback and releases its grant. A grant arriving after immediate cancellation must be released without starting the old player. Unsupported playback stays on the cover.

Playback reads stream bounded byte ranges instead of loading the original into a Blob cache. Core permits at most 8 grants, 2 simultaneous streams per grant and 64 KiB read buffers. Grants bind Library/context and source identity, use a renewable 60 s lifetime with a 10 min absolute limit, and hold response-body cancellation ownership until the stream ends. The long-lived Core session credential never appears in a media URL.

## Validation and fallback matrix

| Condition | Result |
| --- | --- |
| Unknown, extensionless, hidden or empty regular file | Collected and visible; bundled artwork if no decoder |
| No decoder and no verified image | Terminal unavailable; no repeated decode/preview requests |
| PPM decoder enabled and granted, no account configured | Content-driven static thumbnail and preview |
| Decoder disabled with valid cached result | Existing verified image remains usable |
| Caller cancels or drops a decode while plugin remains enabled | Join the retired process; restore capacity in the same epoch without persisting a content failure |
| Decoder crashes, violates its protocol or times out | Retire and latch the fault until an explicit lifecycle change; no automatic respawn loop |
| Queued decode executes after a plugin restart | Record failure provenance from the actual executing epoch |
| Source changes while decoder unavailable | Stale image rejected; ordinary file remains visible |
| Old attempt/source/Library/plugin result arrives | Reject publication through identity fences |
| Video Range satisfiable / unsatisfiable | 206 with precise bounds / 416 |
| Playback source invalidated or grant released | Stop current body and reject later reads |

## Examples

Good: a `.ppm` file first shows local artwork, enabling the example in the plugin window updates the existing grid, and preview displays pixels generated by the real decoder process.

Base: a local folder containing photos, code, documents and extensionless files works without configuring synchronization.

Bad: add fake `media` rows for unknown files, derive decoder support from badge categories, load a whole video for hover, or allow an old decoder result to overwrite changed content.

## Required verification

Run `npm test`, PPM real process conformance, populated migration/FTS/query-plan tests, mounted format/capability refresh tests, and real video Range/lifecycle checks. `node tools/checks/file-decoder-integration.mjs` verifies content-derived pixels, larger static preview, negative outcomes and disable/source-change recovery through the actual Core. `node tools/checks/video-hover-integration.mjs` measures real Electron/Core reads for a distant video payload. `node tools/checks/all-file-library-desktop.mjs` uses an isolated mixed-file Library and actual plugin manager controls. Run the Windows OS-backdrop harness for desktop evidence. Platform failures and stale binaries must be reported separately from passing current-tree evidence.

`node tools/bench/file-decoder-resources.mjs` measures Windows process memory with equal source pixels and different input lengths, and checks that disabling joins the child process. Production-task tests pause after encoding and before publication to verify plugin, Library, source and root fencing. These scoped cases do not replace fault testing at every transport stage or operating-system resource exhaustion tests.

`decoder_supervisor_restores_capability_after_cancelled_or_dropped_job` must demonstrate that ordinary cancellation/drop can recover without toggling the plugin. `decoder_task_pending_before_restart_records_current_failure_epoch` must demonstrate that a queued request uses its executing decoder's failure identity. These are distinct from fault-latching and disable/join assertions.

Record the capture mechanism for desktop evidence. `webContents.capturePage` and CDP screenshots establish renderer layout/state, even when a script is named OS-backdrop. Evidence of actual Windows compositing requires an OS capture of the test window; neither capture proves physical-display anti-aliasing quality.

Badge-only presentation checks use the real mounted format fixture plus Web/UI-design gates. Require nonzero displayed image and badge rectangles before checking containment, and cover landscape/portrait/square images, sidebar resize, source replacement, fallback artwork and simultaneous scoped preview instances. A responsive rule hiding the inspector can produce empty rectangles that trivially satisfy containment; explicitly render the inspector in this fixture and reject zero-size geometry. These local checks do not claim a fresh cross-layer release gate.

Hover-response regressions must assert lease intent directly on pointer entry, then deliberately delay media seek completion while checking visible progress/time, rapid reversals and stale-event suppression. Include unknown metadata followed by recovery, invalid duration, immediate-leave late grant cleanup, stationary resume, effective time/badge typography and all four layouts. Combine deterministic delayed-seek ordering checks with the existing real Electron/Core Range/lifecycle integration: eventual native playback alone does not prove immediate admission or optimistic UI feedback.

The real-video test harness records each playback read's scenario phase and a bounded error body. Ordinary playback must return 200/206. Only its deliberate `corrupt-source` case may return 409 with the exact `playback_source_changed` code, because a native read can race the fixture's source rewrite after a grant was issued. Other phases/codes remain failures. Keep this explicit negative scenario separate from positive playback and do not globally allow conflict responses.

Wrong: "File has no decoder, so omit it from search."

Correct: "Include the file in search; decide content image eligibility separately, and stop polling an unavailable image until capability changes."

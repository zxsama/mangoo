# Unified Notification Center Design

Status: confirmed specification

Date: 2026-07-20

## Summary

Megle will replace the separate Tasks and Recent surfaces with one unified
notification system. The system presents current work, important results,
durable operation history, and current-process logs without adding work to the
media browsing hot path.

The titlebar receives one icon-only notification button. It opens a compact,
read-only status popover by default. A secondary-window notification center
provides the complete status view, persistent history, task actions, and
process logs.

Notification history is global across libraries. Core records reliable
per-library operation changes in an Outbox, Electron Desktop mirrors those
changes into a global notification database under `userData`, and Web reads
snapshots and incremental changes through the preload bridge.

## Goals

- Replace the Tasks and Recent titlebar buttons and popovers with one
  notification entry point.
- Show real progress for active logical operations such as root deletion and
  scanning.
- Aggregate thumbnail work into a single cache overview instead of creating a
  row per thumbnail.
- Preserve notification history across app restarts and library switches.
- Show all libraries in one notification center while retaining source-library
  context.
- Route broad workspace errors away from the center-pane error strip and into
  the notification system.
- Provide current-session Electron Main, Core, and Renderer logs in the full
  notification center.
- Keep notification recording, synchronization, and rendering out of media
  browse, scroll, layout, and thumbnail hot paths.
- Preserve the existing Settings/Plugins secondary-window visual language.

## Non-goals

- There is no read/unread state, unread count, or notification badge number.
- Process logs do not persist across app restarts.
- The compact popover does not cancel or retry tasks.
- Megle does not create persistent events for individual thumbnail generation
  attempts.
- The notification center does not replace contextual form validation,
  confirmation dialogs, or blocking library-load recovery UI.
- This work does not introduce a general telemetry or remote logging system.

## Existing Behavior Being Replaced

- `ShellTopBar` currently exposes separate text-and-icon Tasks and Recent
  buttons.
- Tasks are current-library task rows and can be noisy for thumbnail work.
- Recent is a current-library audit of rename, move, recycle-delete, and
  permanent-delete operations rather than a general recent-media feature.
- The full Task Center supports task cancellation and retry but renders raw task
  rows rather than logical-operation groups.
- `LibraryView` renders the broad `library.error` value as an error strip at the
  top of the center workspace.
- Core stdout and stderr are forwarded to the Electron process, but no bounded
  log buffer or UI subscription exists.

The implementation will remove the superseded presentation code and reuse the
underlying task and file-operation capabilities where they remain authoritative.

## Product Model

### Logical operations

One notification operation represents one user intent or one coherent
background operation. Low-level work items update that operation instead of
creating new notification rows.

Required grouping rules:

- Root removal or purge: one operation per removal request, including discovery,
  filesystem work, database cleanup, and completion.
- Manual or background scan: one operation per requested scan run.
- Rename, move, recycle-delete, and permanent-delete: one operation per user
  command or batch command.
- Thumbnail cache: one status aggregate per library. A manual bulk generation
  command may create one final result for that command, but individual media
  items never become notification records.
- Repeated errors: deduplicate by library, source, error code, and target. Update
  occurrence count and last-seen time instead of appending duplicates.

Every background task kind, including future task kinds, must declare its
logical-operation grouping key and presentation class when it is registered.
Creating a new raw-task notification surface is not an allowed integration
path.

Each operation has a stable `operation_id` for its complete lifecycle. Retry
updates the same logical operation and increments its attempt metadata rather
than creating a duplicate compact notification.

### Status and presentation classes

Operations use the existing task lifecycle where applicable: pending, running,
succeeded, failed, and cancelled. Presentation is derived from status and
origin, not from a read flag.

- Active: every pending or running logical operation is visible in the compact
  popover and full status overview.
- Important result: background failures and the success or failure of a
  user-initiated operation are eligible for the compact result list.
- History-only: routine successful background work is persisted but not shown
  as an individual compact result.
- Status-only: the thumbnail cache aggregate appears in current status but does
  not generate per-item history.

The compact result list contains the latest three important results. It is
limited by count, not time, and is independent from active operations.

## Architecture

### 1. Core notification Outbox

Each library database gains a numbered migration for an append-only
`notification_outbox` table. When authoritative operation state is stored in
the library database, Core writes its Outbox event in the same transaction. For
a transition whose source is outside that transaction, the durable Outbox
insert must complete before Core publishes the transition to consumers or
removes transient task state.

An Outbox event contains at least:

- Monotonic sequence.
- Stable operation ID and operation revision.
- Library-local operation kind and category.
- Origin: user, background, recovery, or system.
- Status and phase.
- Progress current value, total value, unit, and computed percentage when known.
- Title, summary, target metadata, and timestamps.
- Error code and error detail when applicable.
- Available capabilities such as cancellable or retryable.
- Presentation class or the inputs needed to derive it.

Progress events are coalesced at the source. Core emits when a phase changes, a
meaningful progress boundary is crossed, or at most once per second while work
is advancing. Terminal success, failure, and cancellation events bypass this
coalescing and are committed immediately.

The Core API contract adds cursor-based Outbox read and acknowledgement
operations. Reads support a bounded long-poll wait. Core wakes the request when
new rows commit, and Desktop acknowledges a high-water sequence only after the
corresponding global transaction succeeds. Acknowledged Outbox rows may then be
pruned. If Core or Desktop stops between read and acknowledgement, replay is
safe because Desktop upserts by library and operation ID with revision checks.

### 2. Desktop notification service

Electron Main owns a `NotificationService` and a global SQLite database under
Electron `userData`. This database is separate from all library databases and
uses WAL plus one serialized write queue.

The durable operation table is keyed by `(library_id, operation_id)` and stores:

- A snapshot of library identity and display name.
- Kind, category, origin, and presentation class.
- Current status, phase, progress, and action capabilities.
- Initial start, latest update, and terminal timestamps.
- Target and result summaries.
- Error details, occurrence count, and retry attempt metadata.

The database also stores the last committed sync cursor for each library. A
batch transaction upserts operations and advances the cursor atomically.

The service maintains one long-poll connection to the active Core. Library
switching aborts the old request, completes any already-read transaction, and
connects to the new Core after restart. Events left in an inactive library's
Outbox are drained the next time that library is activated. Previously mirrored
history remains globally available throughout the switch.

Completed notification operations are retained for 30 days or at most 2,000
records, whichever limit is reached first. Pending and running operations are
never removed by retention. Cleanup runs outside browse-critical work and in a
bounded transaction.

### 3. Renderer bridge and state

Preload exposes typed notification methods; Web does not use raw `fetch`, Node
filesystem APIs, or duplicated DTOs.

The renderer contract provides:

- A compact status snapshot.
- Paged and filtered history queries.
- Incremental current-status updates.
- Operation detail.
- Cancel and retry commands for capable operations.
- Current-process log snapshot and subscription controls.

The shell owns one notification UI state instead of separate task and recent
popover state. Opening the full notification center closes the compact popover.
Opening another compact shell popover closes notifications through the existing
single-popover coordination.

History is queried only when the full history page is active. The compact panel
uses the maintained snapshot and does not execute a history query when opened.

## Thumbnail Aggregation

Thumbnail status reuses the cache-statistics source already used by the
Settings thumbnail panel. The implementation must not add a second aggressive
whole-library statistics loop.

Per library, the aggregate includes:

- Cached count and total candidate count.
- Missing, stale, failed, and pending candidate counts.
- Active bulk-task count.
- Total cache bytes when already available from the shared snapshot.
- Aggregate completion percentage.

The current shared refresh policy remains the upper bound: approximately 8
seconds while bulk work is active and 20 seconds while idle, unless a cheaper
authoritative event updates the same shared snapshot. Opening the compact panel
must not force an immediate expensive recount.

Automatic thumbnail generation only updates the per-library aggregate. A
manual bulk request may persist one logical result when it finishes or fails.
Ten thousand individual thumbnail completions must not produce ten thousand
Outbox or global notification rows.

## User Interface

### Titlebar entry point

The Tasks and Recent buttons are removed. One icon-only notification button
uses the same size, padding, border treatment, and global pointer-highlight
system as ordinary main-titlebar icon buttons.

The button has no text count. Its optional status dot is deliberately subtle:

- No dot when idle and no current actionable failure exists.
- Low-luminance cool dot while one or more operations are active.
- Low-luminance warm dot when a current retryable operation has failed.
- Failure takes precedence over active when both states exist.

The dot represents current operational state, not unread content. Opening the
popover does not clear it. A retryable failure clears when a retry begins, the
operation later succeeds, or a newer authoritative operation supersedes it.

### Compact popover

The default notification interaction is an anchored Liquid Glass popover. It is
read-only and contains, in order:

1. All active logical operations, each with phase, current/total values when
   known, percentage, and a progress bar.
2. One thumbnail cache overview, never per-thumbnail rows.
3. The latest three important results.
4. A button that opens the full notification center.

Unknown totals use phase text and an indeterminate progress treatment rather
than a false percentage. Completed rows are not kept in the active section.
Cancel and retry controls do not appear in the compact panel.

### Full notification center

The complete center is a standalone secondary Electron window using the shared
`SecondaryWindowFrame` and the same material variables as Settings and Plugins.

Visual contract:

- Preserve the current secondary-window titlebar and its bottom outline.
- Use a dark left navigation pane.
- Use a right content surface that transitions from a lighter material tone to
  the darker edge.
- Do not draw a vertical divider between the navigation and content panes.
- Keep the global Liquid Glass pointer-highlight contract; do not add local
  hover borders or a second glow implementation.

The left navigation has three pages:

#### Status overview

- Active operations across all libraries with progress and source-library
  labels.
- Thumbnail cache aggregate by library.
- Latest important results.
- Notification synchronization health when degraded.

#### Notification history

- Persistent all-library operation history.
- Search and filters for library, status, and category.
- Paged retrieval and bounded rendering.
- Expandable operation details with errors, targets, timestamps, and attempts.
- Cancel for currently cancellable operations.
- Retry for retryable failed operations.
- No read markers, unread filters, or badge counts.

#### Process logs

- Current application session only.
- Sources: Electron Main, Core stdout/stderr, and structured Renderer logs.
- Search and filters for source and level.
- Follow-latest mode with pause/resume.
- Copy selected content and export the current bounded buffer.
- No persistence in the notification database.

The log page subscribes only while active. Leaving the page stops renderer log
delivery while Desktop continues maintaining the bounded buffer.

## Process Log Boundary

Desktop owns a current-session ring buffer bounded by approximately 5,000 lines
or 5 MB, whichever is reached first.

- Core process output is captured where Desktop already receives stdout and
  stderr.
- Electron Main uses the shared desktop logger sink.
- Renderer uses a typed, batched preload logging method for meaningful
  application logs.
- The implementation must not monkey-patch all renderer `console` methods.
- Renderer batches must be asynchronous and bounded so logging cannot add
  synchronous work to scrolling, media layout, or thumbnail paint loops.

No sensitive-data redaction requirement is added because Megle is local
software and the user explicitly selected complete current-process logs.

## Error Handling

The center-pane global `error-strip` is removed. Broad failures currently routed
through `LibraryState.error` are classified at their source and reported to the
notification service with a stable deduplication key.

Contextual feedback remains local when the user cannot proceed without it:

- Field validation in rename, move, delete, settings, and plugin dialogs.
- Confirmation prompts.
- Blocking library-load and library-switch recovery UI, including retry and
  switch-library actions.

Those blocking failures are also recorded once in notification history. They do
not create an additional center-pane banner.

If Desktop cannot write the global notification database, it does not
acknowledge Core Outbox rows. It retries with bounded exponential backoff and
keeps a current-session in-memory view so the failure does not block browsing.
Synchronization health is exposed directly to the status UI and process logs;
the system does not recursively create a notification about its inability to
write notifications.

Cancel or retry command failure updates the existing operation detail and error
state. It does not append a duplicate logical operation.

## Upgrade and Migration

On first activation of a library after this feature ships, Desktop imports
eligible existing `file_operations` rows from the previous 30 days. Imported
operation IDs are deterministic, making the import idempotent across restarts.
Libraries that are not activated are imported when next opened.

Existing pending and running tasks seed current notification state during Core
startup or recovery. Existing self-removing task behavior must emit a terminal
Outbox event before task-row cleanup so fast root-purge completion cannot be
missed.

Old UI state, components, CSS, validators, and polling that only support the
separate Tasks or Recent surfaces are removed after the unified center is wired.
Backend task execution and file-operation audit data remain where they are still
authoritative.

## Performance Requirements

- No individual thumbnail notification rows or Outbox events.
- No additional renderer polling loop for active tasks, thumbnail statistics,
  history, or errors.
- One active Core Outbox long-poll connection, woken by committed events rather
  than a busy interval.
- At most one non-terminal persisted progress event per active logical operation
  per second.
- Terminal events are never delayed by progress coalescing.
- Global notification writes are batched and serialized in Desktop's separate
  WAL database.
- The compact popover consumes a maintained snapshot and does no full-history
  query.
- History is paged and rendered in bounded batches.
- Log delivery to Web occurs only while the log page is active.
- Notification work must not run inside media-grid scroll, layout measurement,
  thumbnail priority, decode, or paint callbacks.

## Documentation Deliverables

Implementation includes documentation updates, not only this specification:

- `README.md`: add the unified notification center to the product capability
  overview.
- `CLAUDE.md`: document ownership, cross-library persistence, grouping rules,
  preload boundary, and the prohibition on per-thumbnail notifications or UI
  polling.
- `docs/project-structure.md`: document new directories or durable modules if
  implementation introduces them.
- Relevant architecture or contract documentation: record the Outbox and global
  notification database flow so future task types integrate through the same
  boundary.

## Verification

### Core and schema

- Migration applies to new and existing library databases.
- Outbox event and authoritative operation transition are transactionally
  consistent where they share a database transaction.
- Sequences are monotonic and acknowledgement prunes only acknowledged events.
- Replay after read-before-ack is idempotent.
- Progress coalescing obeys the one-per-second ceiling while terminal events are
  immediate.
- Root purge emits its terminal event before self-cleanup.
- Repeated errors and thumbnail work follow their grouping rules.

### Desktop

- Global database upsert is idempotent by library, operation, and revision.
- Cross-library history remains visible during Core restart and library switch.
- Cursor and operation updates commit atomically.
- Failed persistence leaves Outbox events unacknowledged and retries safely.
- Retention preserves active operations and enforces 30 days or 2,000 completed
  records.
- Legacy file-operation import is deterministic and idempotent.
- The log buffer obeys both line and byte limits.
- Log subscription starts and stops with the log page.

### Web

- Exactly one notification titlebar button replaces Tasks and Recent.
- Dot priority and no-read semantics match this specification.
- Compact status shows every active operation with correct determinate or
  indeterminate progress.
- Compact results are limited to the latest three important results.
- Thumbnail status is one aggregate per library.
- Cancel and retry are absent from compact mode and available only when valid in
  the full center.
- History filters, pagination, search, and all-library labels work.
- The center-pane broad error strip is absent while contextual blocking errors
  remain usable.

### Performance and interaction

- A synthetic run with at least 10,000 thumbnail item completions produces no
  per-item notification rows and only the defined aggregate state.
- A fake-clock progress test proves that large progress bursts are coalesced and
  terminal events are not lost.
- With notification UI closed, no history or log renderer traffic occurs.
- Real desktop interaction covers scanning, root deletion, file operations,
  failure/retry, library switching, notification-window lifecycle, and current
  process logs while browsing and scrolling media.
- The interaction pass checks that notification activity does not introduce
  visible scroll stalls, layout churn, or thumbnail request amplification.

### Required validation gates

- `npm run check:core-api`
- `npm run check:core-client`
- `npm run check:desktop`
- `npm run check:web`
- `npm run check:ui-design`
- `npm run check:schema`
- `npm run check:rust`
- The explicit Electron OS-backdrop visual regression harness.
- Final `npm test` before completion.

## Acceptance Criteria

The feature is complete when one subtle notification titlebar button provides a
compact status-first view, the full secondary window provides durable
all-library history and current-process logs, root deletion and other active
logical operations expose correct progress, thumbnail work remains aggregated,
the old broad center-pane error strip and separate Tasks/Recent presentation are
gone, history survives restart and library switching, required project
documentation is updated, and fresh functional, performance, and visual
verification passes without degrading media browsing behavior.

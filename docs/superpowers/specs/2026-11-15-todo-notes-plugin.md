# Built-in TODO notes plugin (com.mangoo.todo-notes) and generic plugin data

## Goal

A work-item and memo tool in the spirit of Microsoft To Do, TickTick, Todoist,
Things and Windows Sticky Notes, shipped as a Host-engine plugin: its own
window and tray icon, items that can be pinned to the desktop and collapse to
a summary until hovered, and content that synchronizes with the main project.
Disabling the plugin must leave Mangoo exactly as it is without it.

## Survey → feature set

| Capability | Typical apps | Mangoo 便笺 |
| --- | --- | --- |
| Smart views | To Do (My Day, Planned, Important), TickTick, Things | 今天 (+ 已逾期), 计划 (by date), 重要, 收集箱, 全部, 已完成, 最近删除 |
| Lists, tags, search | all | colored lists, `#tags`, search over title / notes / tags / descriptions |
| Subtasks | To Do steps, TickTick, Todoist | nested subtasks (3 levels), progress, reset on recurrence roll-over |
| Priority | Todoist P1–P4, TickTick | none / low / medium / high, colored checkbox + flag, keys 0–3 |
| Dates, reminders, repeat | all | date + time, reminders (at due / before / custom), daily · weekdays · weekly · monthly · yearly · every N |
| Rich notes | Notion, TickTick, Things | headings, emphasis, highlight, lists, checklists, quote, code, links, images |
| Attachments | To Do files, TickTick | link cards, compressed images, text blocks |
| Quick capture | Todoist NLP, To Do | `周报 明天 17:30 !高 #工作`; optional global shortcut; tray “新建事项” |
| Desktop notes | Sticky Notes, Simple Sticky Notes | pin any item; collapsed summary, expand on hover; color, opacity, lock, keep expanded |
| Tray / background | most desktop todo apps | tray with today count, close-to-tray, optional start at login (tray only) |
| Undo / trash | all | undo toast for complete / delete, 最近删除 with restore and permanent clear |
| Ordering | all | manual drag (fractional keys) or sort by date / priority / created / title |

Out of scope: collaboration / sharing, calendar views, location reminders,
import from other apps, file attachments other than images.

## Generic plugin data (all plugins)

Plugin content follows the main project's synchronization, so it must live
where synchronization lives: the **Library Catalog**. Sync is event-sourced and
entity-generic (`sync_events` + `materialize_truth`), so a plugin record is an
ordinary synchronized entity — no new protocol, transport or provider work:

- `plugin_records(record_uuid, plugin_id, collection, fields_json, deleted, revision, sync_captured)` is the projection (migration `0040_plugin_records`); `plugin_record_revisions` is a per-plugin change counter.
- The entity's immutable header field `plugin-record.v1` names owner plugin and collection; every top-level field is its own LWW field `plugin-field.v1:<name>`; delete / restore are canonical tombstones. Concurrent edits of different fields on two devices both survive.
- Local writes update the row and, while a binding captures, emit the events in the same `FULL`-durability transaction. Rows written before capture are backfilled by `capture_uncaptured_plugin_records` on each sync turn.
- Remote apply and checkpoint restore replay through `materialize_truth`, which calls `rebuild_plugin_record` (a pure function of truth; a UUID never changes owner).
- Older builds keep the unknown fields in `sync_field_versions` and project nothing (prefixed names cannot collide with tag / metadata projection).
- Limits keep every field one event: 64 fields, 160 KiB per serialized value, 1 MiB per record, 200 operations per batch. Segment sealing is now byte-bounded as well as count-bounded, so a run of large events can never exceed `MAX_SEGMENT_DECODED_BYTES`.

API (`/api/plugin-data/{pluginId}`): `records` (keyset page per collection), `records/{recordId}`, `batch` (put / delete / restore, all or nothing), `changes` (bounded long-poll that does not hold the Library request gate and re-resolves the active Library on every wake). Every route requires an enabled, granted, trusted Host plugin that requests `library-data` (`HostRegistry::host_plugin_permission_enabled`); one plugin can never touch another's records.

Device-local presentation state (screen positions, per-device preferences) is
not plugin data: Desktop-side plugins keep it in their own `userData` store.

## Revision 2026-11-15: portrait UI and cross-Library sharing

**Interaction.** The notes window is one portrait column (380×640 default,
300 px minimum) without a titlebar: a slim draggable header (view switcher,
search, keep-on-top, ⋯, close), an always-visible capture box (Enter saves;
first line = title with natural-language date / priority / tags, Shift+Enter
continues into the notes), a chip row of quick views and lists, and a stream
of cards. A card answers most questions without a click — title, preview, the
first subtasks (tickable in place), chips — and expands in place for editing:
properties are chips (date, reminder, repeat, priority, tags, list, scope,
pin) that open one small inline panel; the notes editor shows its toolbar only
while focused. There is no detail pane and no sidebar.

**Sharing across Libraries.** Every item and list has a scope: `shared` (all
Libraries — the default for new notes, configurable) or `library`. Lists are
the unit you share (共享到所有资料库 / 改为仅本资料库), single notes can move
too. Shared notes and their desktop pins follow every Library switch and work
with no Library open; Library notes and their pins appear only in their
Library. Storage and sync come from the generic shared scope
(`/api/plugin-data/{pluginId}/shared/*`): Host-held records whose
last-writer-wins fields are reconciled with every active Library's mirror, so
they also sync through any Library. A move between scopes creates new ids
(parents, image references and the pin follow).

## Layout refinement (component by component)

| Component | Decision |
| --- | --- |
| Header | View name (opens all views + lists) · search · ⋯ · close. Search expands in place over the header; keep-on-top lives in ⋯ and shows a header icon only while on. |
| Capture | Quiet fill at rest, accent boundary only once there is text. Parsed date / priority / tags preview on one line with a ↵ hint; the scope badge only when it differs from the default. Hidden while searching. |
| Chips | The one place for quick views; every chip shows its open count; a fade edge marks overflow. |
| Card (collapsed) | Priority colours the checkbox ring; a flag only for high. One fixed slot at the title's end shows status (pin, flag) at rest and actions (pin, ⋯) on hover / selection, so nothing reflows. Scope badge only when both scopes hold notes. A subtask listed alone names its parent. |
| Card (expanded) | Edit time right under the title. One row of primary properties (date, reminder, priority, tags) + ⋯ for repeat / list / scope. Notes read as soft text until focused (toolbar on focus only). Footer: add link / image / text on the left; pin and delete on the right. |
| ⋯ menu | Grouped: sort · display (compact, completed) · desktop notes (one show/hide toggle) and keep-on-top · settings and Mangoo. |
| Pinned note | Expanded height hugs short content (the saved height is the cap). |
| Notices | Deletion counts what the user picked (subtasks travel with their note). |

## Settings, menus and sizing (2026-11-15)

- **Settings follow the image viewer.** A standalone frameless acrylic window (role
  `todo-notes-settings`, 720×560, opened beside the notes column when there is room):
  section nav on the left (记录 · 显示 · 桌面便笺 · 提醒 · 系统 · 快捷键), one scrolling
  column of bordered row groups, switches for on / off and segmented controls for small
  choices, scroll spy, Esc closes. The same form renders in Plugin Manager (without 快捷键).
  It opens from ⋯ → 设置…, the tray and Plugin Manager; it gets no Core session.
- **Menus by intent.** The header view menu is *where to go* (smart views, lists + 新建清单,
  then 已完成 / 最近删除). ⋯ is *how this view looks* (sort, compact), *the desktop*
  (置顶, show / hide desktop notes) and *the app* (设置…, 打开 Mangoo). The item menu runs
  in the order people act: 完成 / 编辑 / 贴到桌面 → one row of dates → one row of
  priorities → 移到清单 (second level) / 保存位置 / 复制 → 删除, set apart.
- **Expanded card.** Only properties that hold a value show; ＋属性 reveals the empty ones.
- **Type scale.** 12 / 13 / 14 / 16 / 20 px (caption · body · label · title · section), matching
  the Host's 14px menus and dialogs; controls are 24 / 28 / 32 px.
- **Default size.** Notes window 400×680 (min 320×440): one property row fits on one line.
- **Highlight and floating placement.** Icon buttons are fixed squares that clip their glow;
  card hover actions swap with status icons inside one reserved slot (opacity only, no reflow);
  the drop indicator uses an outer shadow, not the pseudo-elements the pointer glow owns;
  notices float at the bottom with matching scroll padding so the last card can scroll above
  them; desktop-note actions get their own row that exists only while the note is expanded,
  and the expanded height counts it, so nothing paints over text or is clipped.

## Revision 2026-11-15 (b): no Library affiliation, Markdown, remappable keys

- **Notes belong to no Library.** Every record is written to and read from
  the shared scope only; there is no scope UI anywhere and no per-Library
  pin / reminder bookkeeping on the Desktop side (one flat `pinned` map).
- **Description = Markdown-first rich text.** Storage is unchanged (the
  sanitized HTML subset). Block prefixes convert on the space after them
  (`# ` `## ` `- ` `1. ` `[ ] ` `[x] ` `> ` ```` ``` ````), inline marks on their
  closing delimiter (`**b**` `*i*` `~~s~~` `==m==` `` `c` ``), a typed URL
  followed by a space becomes a link, Enter on an empty quote line leaves the
  quote. Pasting a URL inserts a link (over a selection: links that text);
  pasted Markdown from a plain-text source converts; images paste / drop in
  place, inserted with their pixels (never an empty image waiting for a
  round trip); the store keeps a synchronous URL cache so re-opening a note
  paints its images on the first frame. There are no separate link / image /
  text attachments.
- **Rich titles.** Note and subtask titles are stored as sanitized INLINE
  HTML (b / i / u / s / code / mark / a; `sanitizeTitle`), edited in a
  one-line contenteditable with the same shortcuts and Markdown marks; plain
  text (tray, notifications, search, sort, copy) comes from `titleText`.
- **Sort is its own header button** (↓↑, highlighted when the view's order is
  not its default; remembered per view). ⋯ keeps display / desktop / app.
- **Menus.** Sub-choices are shown inline: the item menu lists the target
  lists under a 移到清单 label instead of a second page. Shortcut hints are
  small, muted and tabular, and come from the user's effective bindings.
- **Remappable shortcuts** (`src/shared/shortcuts.ts`, the image viewer's
  model): an action registry with defaults, per-device overrides in the
  window preferences, chord recorder with conflict-move + undo, per-action
  and global reset. Esc / Enter / Tab stay fixed. Note actions act on the
  selected card and never fire while typing; plain keys never fire inside
  fields. The global quick-add shortcut is recorded the same way (any
  Ctrl/Alt chord or a function key) and reports when another app holds it.
- **Persistent backdrop.** Windows turns a window's acrylic into a flat fill
  when it loses focus; every plugin window re-applies its material
  synchronously inside `blur`. Measured: deferring the call (or resetting to
  "none" first) let the fade run ~80 ms and snap back — a visible flicker.
- **Properties float.** Property chips show only what is set; ＋属性 opens a
  small menu of the unset ones. Each property edits in a popover whose
  top-left corner is the pointer (it slides to fit), never inline. The
  reminder is part of the date (a switch with offsets / custom time); the date
  chip shows a bell instead of the calendar when a reminder is on.
- **Active states.** Every choice / toggle in a menu carries a trailing
  check when on (sort, views, lists, 紧凑显示, 窗口置顶, 显示桌面便笺).
- **Backup.** Settings → 备份: export every list, note, subtask and image to
  one JSON file (`shared/backup.ts`, validated on import), restore writes
  each record back under its own id (restore + put), keeping newer notes.
- **Pinned notes.** Appearance is only colour + keep-expanded; the colour
  choice is a floating popover above the footer.
- **Icon scale.** 14 px in 28 px controls and menus, 13 px in 24 px
  controls, 11–12 px inside chips; editor tools use the 24 px control token.

## Manifest additions

Host capability `todo-notes.v1`; Host activation event `startup`; Host
permissions `library-data { access: plugin-records }`, `system-tray { scope:
current-session }`, `login-item { scope: current-user }`. Process plugins
cannot declare any of them.

## Desktop

`createTodoNotesHost(electron, options)` owns the notes window (role
`todo-notes`), one frameless always-on-top window per pinned item (role
`todo-sticky`, hidden from the taskbar), the tray, reminders, the global
quick-add shortcut, the login item and one change-feed long-poll whose result
is broadcast to every plugin window. main.ts only routes: `window-all-closed`
keeps running while the tray exists, and a `--mangoo-background` login launch
opens no Library window. Disabling the plugin closes all of it and removes the
login item only if this plugin created it.

## Verification

- Rust: projection CRUD / isolation / limits / keyset paging, capture →
  replay into a fresh Catalog (field merge, tombstones, backfill), route gate
  (`403 plugin_data_disabled`), registry lifecycle gate, manifest strictness.
- JS: model (records, ordering keys, recurrence, quick add, views, links),
  desktop state, pinned-note geometry, reminders, preload channel parity;
  structure / contract / web / desktop / UI-design gates.
- Real Electron run: enable → tray + window → create / pin / hover → disable
  → every surface gone and `403`.

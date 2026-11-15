# com.mangoo.todo-notes — Mangoo 便笺

Built-in **Host-engine** plugin for work items and memos: a standalone notes
window, a notification-area (tray) icon, and notes pinned to the desktop.
Notes belong to no Library: the same notes are there whichever Library is
open (or none), and they synchronize through every Library you sync.

Turning the plugin on in Plugin Manager (enable + approve its permissions)
turns the feature on. Turning it off removes every surface — the notes window,
pinned notes, tray icon, global shortcut, reminders and the login item it
created — and Mangoo behaves exactly as without it. **No item is deleted**:
records stay in Core (and keep synchronizing), so re-enabling the
plugin, on this or another device, finds them intact. While off, Core answers
`403 plugin_data_disabled` on `/api/plugin-data/com.mangoo.todo-notes/*`.

## Parts

| Part | Location |
| --- | --- |
| Manifest (`engine: "host"`, `host.capabilities: ["todo-notes.v1"]`, activation `startup`) | `plugin.template.json` (staged as exactly `plugin.json`, pinned in compiled trust) |
| Desktop: notes window, pinned-note windows, tray, reminders, global quick-add, login item, change feed | `src/desktop/` → `dist/desktop`, imported by Electron main |
| Web: notes window, pinned note, plugin-manager panel | `src/web/`, compiled into the Host web bundle; imports only `@mangoo/web/plugin-host` |
| Shared model (records ↔ items, ordering keys, recurrence, quick-add parsing) and IPC contract | `src/shared/` |
| Storage + sync (generic for every Host plugin) | Core `/api/plugin-data/{pluginId}/shared/*` (`crates/core/src/api/plugin_data.rs`, `plugin_shared.rs`, `db/plugin_records.rs`) |

## Permissions

- `{ "kind": "library-data", "access": "plugin-records" }` — keep the plugin's own records through Core's generic plugin-data service (synchronized through the Libraries you sync).
- `{ "kind": "system-tray", "scope": "current-session" }` — a notification-area icon while enabled.
- `{ "kind": "login-item", "scope": "current-user" }` — optional, only when the user turns on “登录时启动” (packaged builds; dev builds never touch the OS).

## Data model

Every record lives in the plugin's **shared scope**
(`/api/plugin-data/com.mangoo.todo-notes/shared/*`): held by the Host,
mirrored into each active Library so it synchronizes through any of them.
Collections (each top-level field merges independently across devices):

- `items` — `title` (sanitized inline rich text), `notes` (sanitized rich-text HTML), `done`, `completedAt`, `priority` (0–3), `dueDate` (`YYYY-MM-DD`), `dueTime`, `remindAt`, `recurrence`, `tags`, `listId`, `parentId` (subtasks), `order` (fractional key: reordering never rewrites siblings).
- `lists` — `name`, `color`, `order`.
- `attachments` — one compressed WebP image per record (≤ 150 KiB base64, one sync event), referenced by `<img data-attachment>` in notes.

Device-local state — window bounds, where each pinned note sits on this machine's screens, note color / keep-expanded, Desktop settings, reminder bookkeeping — lives in `userData/todo-notes.json` and is never synchronized (screens differ per device).

## Features

- **Portrait, no titlebar**: slim draggable header (views, search, sort, ⋯), always-ready capture box, quick-view / list chips, cards that edit in place.
- **Views**: 今天 (with 已逾期), 计划 (grouped by date), 重要, 全部, 已完成, 最近删除; user lists with colors; tags; full-text search over titles, notes and tags.
- **Items**: completion with undo, priority, date + time with an optional reminder (on time / before / custom), repeat (daily, weekdays, weekly, monthly, yearly, every N), subtasks (nested, progress), tags, lists, manual drag ordering or sort by date / priority / created / title. Properties show as chips; ＋属性 and each chip open a small popover at the pointer.
- **Rich text**: titles (inline marks and links) and notes (headings, emphasis, highlight, lists, checklists, quotes, code, links, images), written with the toolbar, shortcuts or Markdown; pasted URLs become links, pasted / dropped images embed. Sanitized on write and on render.
- **Quick add**: `周报 明天 17:30 !高 #工作` sets date, time, priority and tags.
- **Pinned notes**: pin any item to the desktop. Collapsed it shows checkbox, title, date and subtask progress; hovering expands notes and subtasks; per-note color and keep-expanded; notes grow upward at the screen edge and come back after a monitor is removed. Show / hide all from the tray.
- **Tray**: today's count in the tooltip and menu, open, quick add, show / hide pinned notes, settings, open Mangoo, quit. Closing the notes window hides it to the tray (configurable).
- **Reminders**: native notifications; missed reminders from the last 12 hours fire once at start; recurring items roll forward with their reminder.
- **Keyboard**: every window shortcut is remappable in settings (快捷键); a global quick-add shortcut can be recorded too.
- **Backup**: settings → 备份 exports every list, note and image to one file and restores it.

## Checks

`npm run check:plugin-todo-notes` (typecheck both halves + unit tests). Core coverage: `cargo test -p mangoo-core plugin_records plugin_data host_plugin_library_data_gate`.

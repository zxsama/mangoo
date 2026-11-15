/**
 * Web half of the built-in TODO notes plugin. The Host renderer imports this
 * module lazily: `main.tsx` mounts `TodoNotesWindow` for the notes window and
 * `TodoStickyWindow` for each note pinned to the desktop, and the plugin
 * manager renders `TodoNotesPluginPanel` in the plugin's detail pane.
 */
export { TodoNotesWindow } from "./TodoNotesWindow.tsx";
export { TodoStickyWindow } from "./TodoStickyWindow.tsx";
export { TodoNotesPluginPanel } from "./TodoNotesPluginPanel.tsx";
export { TodoSettingsWindow } from "./TodoSettings.tsx";
export { TODO_ROUTE, TODO_STICKY_ROUTE, TODO_STICKY_WINDOW_ROLE, TODO_WINDOW_ROLE } from "../shared/ipc.ts";
export { TODO_PLUGIN_ID } from "../shared/model.ts";

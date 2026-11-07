/**
 * IPC contract between the image viewer's Desktop host (Electron main) and its
 * renderer window. Every channel is owned by this plugin: main.ts only routes
 * them to the host controller, which authorizes the sender per channel.
 */

export const IMAGE_VIEWER_PLUGIN_ID = "com.mangoo.image-viewer";
export const IMAGE_VIEWER_CAPABILITY = "image-viewer.v1";
/** Window role announced through `--mangoo-window=` and preload. */
export const IMAGE_VIEWER_WINDOW_ROLE = "image-viewer";
/** Hash route the shared web bundle renders for the viewer window. */
export const IMAGE_VIEWER_ROUTE = "#/image-viewer";

export const IMAGE_VIEWER_CHANNELS = {
  /** viewer -> main (invoke): renderer painted; returns the pending open request. */
  ready: "mangoo:image-viewer:ready",
  /** main -> viewer (send): open these files (second instance, OS open-file, Library). */
  openRequest: "mangoo:image-viewer:open-request",
  /** viewer -> main (invoke): native multi-select open dialog. */
  pickFiles: "mangoo:image-viewer:pick-files",
  /** viewer -> main (invoke): native folder dialog. */
  pickFolder: "mangoo:image-viewer:pick-folder",
  /** viewer -> main (invoke): admit dropped files/folders into this viewer session. */
  admitPaths: "mangoo:image-viewer:admit-paths",
  /** viewer -> main (invoke): move one admitted image to the recycle bin / trash. */
  trash: "mangoo:image-viewer:trash",
  /** viewer -> main (invoke): rename one admitted image within its folder. */
  rename: "mangoo:image-viewer:rename",
  /** viewer -> main (invoke): reveal one admitted image in the file manager. */
  reveal: "mangoo:image-viewer:reveal",
  /** viewer -> main (invoke): open one admitted image with the system default app. */
  openExternally: "mangoo:image-viewer:open-externally",
  /** viewer -> main (invoke): PNG bytes to the system clipboard. */
  copyImage: "mangoo:image-viewer:copy-image",
  /** viewer -> main (invoke): text (path) to the system clipboard. */
  copyText: "mangoo:image-viewer:copy-text",
  /** viewer -> main (invoke): toggle fullscreen, returns the new state. */
  toggleFullscreen: "mangoo:image-viewer:toggle-fullscreen",
  /** main -> viewer (send): fullscreen state changed. */
  fullscreenChanged: "mangoo:image-viewer:fullscreen-changed",
  /** viewer -> main (invoke): keep the window above others. */
  setAlwaysOnTop: "mangoo:image-viewer:set-always-on-top",
  /** viewer -> main (invoke): bring up the main Mangoo window. */
  openMainWindow: "mangoo:image-viewer:open-main-window",
  /** viewer / plugins / settings -> main (invoke): association status. */
  associationStatus: "mangoo:image-viewer:association-status",
  /** viewer / plugins / settings -> main (invoke): register / unregister. */
  setAssociation: "mangoo:image-viewer:set-association",
  /** viewer / plugins / settings -> main (invoke): OS default-apps settings. */
  openDefaultAppsSettings: "mangoo:image-viewer:open-default-apps-settings",
  /** main / plugins -> main (invoke): open files in the viewer (Library "open with"). */
  openFiles: "mangoo:image-viewer:open-files"
} as const;

/** Channels a privileged Host window (main, settings, plugins) may also use. */
export const IMAGE_VIEWER_PRIVILEGED_CHANNELS: readonly string[] = [
  IMAGE_VIEWER_CHANNELS.associationStatus,
  IMAGE_VIEWER_CHANNELS.setAssociation,
  IMAGE_VIEWER_CHANNELS.openDefaultAppsSettings,
  IMAGE_VIEWER_CHANNELS.openFiles
];

export interface ImageViewerOpenRequest {
  /** Monotonic per Desktop process; the renderer ignores replays. */
  id: number;
  /** Absolute paths. One file opens its folder; several files form the set. */
  paths: string[];
}

export type ImageViewerPlatform = "win32" | "linux" | "darwin" | "other";

export interface ImageViewerAssociationStatus {
  platform: ImageViewerPlatform;
  /** Whether this build can register itself on this platform. */
  supported: boolean;
  /** Whether the OS currently has this executable registered for images. */
  registered: boolean;
  /** The user's persisted choice; re-applied when the plugin is re-enabled. */
  wanted: boolean;
  /** Whether the viewer plugin is enabled (and therefore actionable). */
  pluginEnabled: boolean;
  /** Windows / Linux can open a settings surface to pick the default app. */
  canOpenSystemSettings: boolean;
  /** Human readable explanation (Chinese UI copy). */
  detail: string;
  extensions: string[];
}

export interface ImageViewerActionResult {
  ok: boolean;
  error?: string;
  /** Rename: the new absolute path. */
  path?: string;
}

export interface ImageViewerDesktopBridge {
  ready(): Promise<ImageViewerOpenRequest | null>;
  onOpenRequest(callback: (request: ImageViewerOpenRequest) => void): () => void;
  pickFiles(): Promise<string[] | null>;
  pickFolder(): Promise<string | null>;
  admitPaths(paths: string[]): Promise<string[]>;
  /** Absolute path of a dropped File (Electron webUtils), or null. */
  pathForFile(file: File): string | null;
  trash(path: string): Promise<ImageViewerActionResult>;
  rename(path: string, newName: string): Promise<ImageViewerActionResult>;
  reveal(path: string): Promise<ImageViewerActionResult>;
  openExternally(path: string): Promise<ImageViewerActionResult>;
  copyImage(png: Uint8Array): Promise<ImageViewerActionResult>;
  copyText(text: string): Promise<ImageViewerActionResult>;
  toggleFullscreen(): Promise<boolean>;
  onFullscreenChanged(callback: (fullscreen: boolean) => void): () => void;
  setAlwaysOnTop(value: boolean): Promise<boolean>;
  openMainWindow(): Promise<boolean>;
  associationStatus(): Promise<ImageViewerAssociationStatus>;
  setAssociation(enabled: boolean): Promise<ImageViewerAssociationStatus>;
  openDefaultAppsSettings(): Promise<boolean>;
}

/** Bridge exposed to privileged Host windows (plugin manager, Library). */
export interface ImageViewerHostWindowBridge {
  associationStatus(): Promise<ImageViewerAssociationStatus>;
  setAssociation(enabled: boolean): Promise<ImageViewerAssociationStatus>;
  openDefaultAppsSettings(): Promise<boolean>;
  openFiles(paths: string[]): Promise<boolean>;
}

export const MAX_OPEN_PATHS = 4096;
export const MAX_COPY_IMAGE_BYTES = 512 * 1024 * 1024;

/** Strict shape check for renderer-supplied path lists. */
export function parsePathList(value: unknown): string[] | null {
  if (!Array.isArray(value) || value.length === 0 || value.length > MAX_OPEN_PATHS) return null;
  const paths: string[] = [];
  for (const item of value) {
    if (typeof item !== "string" || item.length === 0 || item.length > 4096 || item.includes("\0")) return null;
    paths.push(item);
  }
  return paths;
}

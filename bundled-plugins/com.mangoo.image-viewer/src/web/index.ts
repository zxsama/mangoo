/**
 * Web half of the built-in image viewer plugin. The Host renderer imports this
 * module lazily: `main.tsx` mounts `ImageViewerWindow` for the viewer OS
 * window, and the plugin manager renders `ImageViewerPluginPanel` in the
 * plugin's detail pane.
 */
export { ImageViewerWindow } from "./ImageViewerWindow.tsx";
export { ImageViewerPluginPanel, AssociationControls } from "./ImageViewerPluginPanel.tsx";
export { viewerHostBridge } from "./bridge.ts";
export { IMAGE_VIEWER_PLUGIN_ID, IMAGE_VIEWER_ROUTE, IMAGE_VIEWER_WINDOW_ROLE } from "../shared/ipc.ts";

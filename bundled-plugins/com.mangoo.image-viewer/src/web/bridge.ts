import { getDesktopBridge } from "@mangoo/web/plugin-host";

import type { ImageViewerDesktopBridge, ImageViewerHostWindowBridge } from "../shared/ipc.ts";

type ViewerBridgeHost = { imageViewer?: ImageViewerDesktopBridge; imageViewerHost?: ImageViewerHostWindowBridge };

/** The viewer window's own Desktop bridge (null in a plain browser build). */
export function viewerBridge(): ImageViewerDesktopBridge | null {
  return (getDesktopBridge() as ViewerBridgeHost | null)?.imageViewer ?? null;
}

/** Association bridge: the viewer window itself, or a privileged Host window. */
export type ImageViewerAssociationBridge = Pick<ImageViewerHostWindowBridge, "associationStatus" | "setAssociation" | "openDefaultAppsSettings">;

export function viewerHostBridge(): ImageViewerAssociationBridge | null {
  const bridge = getDesktopBridge() as ViewerBridgeHost | null;
  return bridge?.imageViewerHost ?? bridge?.imageViewer ?? null;
}

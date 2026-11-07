/**
 * The narrow, explicit surface that Host-engine plugins compiled into the web
 * bundle (`bundled-plugins/<id>/src/web`) may import. A plugin feature must look
 * and behave like the rest of Mangoo, so it reuses the Host's design system,
 * window chrome, Core client and preview decode worker through this module
 * instead of reaching into arbitrary Host internals or duplicating them.
 *
 * Sandboxed process plugins never see this module: their UI runs in the
 * isolated iframe bridge described by the plugin-owned UI contract.
 */
export { CoreApiError, createCoreClient } from "../core/client";
export { getDesktopBridge, getWindowControls } from "../core/desktop";
export type { MangooDesktopBridge } from "../core/desktop";
export { broadcastPreference, subscribePreference } from "../core/preferenceSync";
export {
  LiquidGlassButton,
  LiquidGlassLayer,
  LiquidGlassSurface,
  applyInterfaceStyleVariables,
  readInterfaceStyle
} from "../design/liquid-glass";
export { useSecondaryWindowInterfaceStyle } from "../app-shell/useSecondaryWindowInterfaceStyle";
export * as icons from "../design/icons";
export type { AppIcon } from "../design/icons";
export {
  AnchoredOverlay,
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayField,
  OverlayInlineStatus,
  OverlayMenuItem,
  OverlayMenuList,
  OverlayProvider,
  pointOverlayAnchor
} from "../design/overlays";
export { WindowChrome } from "../features/window-chrome/WindowChrome";
export { MediaLoadingRing } from "../features/loading/MediaLoadingRing";
export {
  createDecodeDispatcher,
  decodeBlobOnMainThread,
  type DecodeTransport
} from "../features/preview/previewBitmapSource";

/**
 * The Library preview's off-main-thread decoder (`createImageBitmap` in a
 * Worker, bitmap transferred back). Returns null where Workers are unavailable;
 * callers fall back to `decodeBlobOnMainThread`.
 */
export function createPreviewDecodeWorker(): Worker | null {
  if (typeof Worker === "undefined") return null;
  try {
    return new Worker(new URL("../features/preview/previewDecodeWorker.ts", import.meta.url), {
      type: "module"
    });
  } catch {
    return null;
  }
}

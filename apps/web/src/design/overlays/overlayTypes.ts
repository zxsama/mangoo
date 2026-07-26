import type { CSSProperties, ReactNode } from "react";

export const OVERLAY_VIEWPORT_GUTTER_PX = 12;
export const ROOT_OVERLAY_OFFSET_PX = 6;
export const NESTED_OVERLAY_OFFSET_PX = 4;

export type OverlayRootId =
  | "library"
  | "filter"
  | "sort"
  | "layout"
  | "notifications"
  | "context-menu"
  | "file-operation";

export type OverlayLayerKind = "root" | "nested" | "modal";
export type OverlaySurfaceRole = "popover" | "nested" | "modal" | "secondary-content";
export type OverlayPlacement =
  | "bottom-start"
  | "bottom-end"
  | "top-start"
  | "top-end"
  | "right-start"
  | "left-start";

export interface OverlayRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface OverlaySize {
  width: number;
  height: number;
}

export interface OverlayViewport {
  width: number;
  height: number;
}

export interface OverlayPositionResult {
  left: number;
  top: number;
  maxWidth: number;
  maxHeight: number;
  placement: OverlayPlacement;
}

export interface OverlayVirtualAnchor {
  getBoundingClientRect(): OverlayRect;
}

export type OverlayAnchor = HTMLElement | OverlayVirtualAnchor;

export interface OverlayLayerRegistration {
  id: string;
  rootId: OverlayRootId;
  parentId?: string;
  kind: OverlayLayerKind;
  trigger: HTMLElement | null;
  surface: HTMLElement | null;
  dismissOnEscape: boolean;
  dismissOnFocusOutside: boolean;
  dismissOnOutsidePointer: boolean;
  dismissOnWindowBlur: boolean;
  restoreFocus: boolean;
  onDismiss: () => void;
}

export interface OverlayStackApi {
  activeRootId: OverlayRootId | null;
  openRoot(rootId: OverlayRootId): void;
  closeRoot(rootId?: OverlayRootId): void;
  isRootOpen(rootId: OverlayRootId): boolean;
  registerLayer(layer: OverlayLayerRegistration): () => void;
  dismissTop(): boolean;
}

export interface AnchoredOverlayStyle extends CSSProperties {
  left: number;
  top: number;
  maxWidth: number;
  maxHeight: number;
}

export interface OverlayHeaderContent {
  description?: ReactNode;
  descriptionId?: string;
  title: ReactNode;
  titleId: string;
}

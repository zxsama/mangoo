import {
  NESTED_OVERLAY_OFFSET_PX,
  ROOT_OVERLAY_OFFSET_PX,
  type OverlayAnchor,
  type OverlayPlacement,
  type OverlayPositionResult,
  type OverlayRect,
  type OverlaySize,
  type OverlayViewport,
  type OverlayVirtualAnchor
} from "./overlayTypes.ts";

const pointOverlayAnchors = new WeakSet<OverlayVirtualAnchor>();

export interface ResolveOverlayPositionInput {
  trigger: OverlayRect;
  overlay: OverlaySize;
  viewport: OverlayViewport;
  preferredPlacement: OverlayPlacement;
  offset: number;
  gutter: number;
}

export interface ResolveOverlayWidthInput {
  measuredWidth: number;
  preferredWidth?: number;
  minWidth?: number;
  maxWidth?: number;
  viewportWidth: number;
  gutter: number;
}

function resolveOverlayMaxWidth({
  maxWidth,
  viewportWidth,
  gutter
}: Pick<ResolveOverlayWidthInput, "maxWidth" | "viewportWidth" | "gutter">): number {
  const safeViewportWidth = Math.max(0, viewportWidth - gutter * 2);
  return Math.min(safeViewportWidth, Math.max(0, maxWidth ?? safeViewportWidth));
}

export function resolveOverlayWidth(input: ResolveOverlayWidthInput): number {
  const maxWidth = resolveOverlayMaxWidth(input);
  const minWidth = Math.min(Math.max(0, input.minWidth ?? 0), maxWidth);
  const requestedWidth = Math.max(0, input.preferredWidth ?? input.measuredWidth);
  return Math.min(Math.max(requestedWidth, minWidth), maxWidth);
}

export function resolveOverlayAnchorOffset(anchor: OverlayAnchor, nested: boolean): number {
  if (nested) return NESTED_OVERLAY_OFFSET_PX;
  return pointOverlayAnchors.has(anchor as OverlayVirtualAnchor)
    ? 0
    : ROOT_OVERLAY_OFFSET_PX;
}

export function pointOverlayAnchor(x: number, y: number): OverlayVirtualAnchor {
  const value: OverlayRect = {
    left: x,
    top: y,
    right: x,
    bottom: y,
    width: 0,
    height: 0
  };
  const anchor = { getBoundingClientRect: () => value };
  pointOverlayAnchors.add(anchor);
  return anchor;
}

export interface ResolveAnchoredOverlayLayoutInput {
  trigger: OverlayRect;
  overlay: { measuredWidth: number; height: number };
  viewport: OverlayViewport;
  preferredPlacement: OverlayPlacement;
  preferredWidth?: number;
  minWidth?: number;
  maxWidth?: number;
  offset: number;
  gutter: number;
}

export interface AnchoredOverlayLayoutResult extends OverlayPositionResult {
  width: number;
}

export function resolveAnchoredOverlayLayout(
  input: ResolveAnchoredOverlayLayoutInput
): AnchoredOverlayLayoutResult {
  const widthInput = {
    measuredWidth: input.overlay.measuredWidth,
    preferredWidth: input.preferredWidth,
    minWidth: input.minWidth,
    maxWidth: input.maxWidth,
    viewportWidth: input.viewport.width,
    gutter: input.gutter
  };
  const width = resolveOverlayWidth(widthInput);
  const position = resolveOverlayPosition({
    trigger: input.trigger,
    overlay: { width, height: input.overlay.height },
    viewport: input.viewport,
    preferredPlacement: input.preferredPlacement,
    offset: input.offset,
    gutter: input.gutter
  });
  return {
    ...position,
    maxWidth: resolveOverlayMaxWidth(widthInput),
    width
  };
}

export function resolveOverlayPosition(
  input: ResolveOverlayPositionInput
): OverlayPositionResult {
  const { trigger, overlay, viewport, offset, gutter } = input;
  const maxWidth = Math.max(0, viewport.width - gutter * 2);
  const availableBelow = viewport.height - gutter - trigger.bottom - offset;
  const availableAbove = trigger.top - gutter - offset;
  const availableRight = viewport.width - gutter - trigger.right - offset;
  const availableLeft = trigger.left - gutter - offset;
  let placement = input.preferredPlacement;

  if (placement.startsWith("bottom") && overlay.height > availableBelow && availableAbove > availableBelow) {
    placement = placement === "bottom-end" ? "top-end" : "top-start";
  } else if (placement.startsWith("top") && overlay.height > availableAbove && availableBelow > availableAbove) {
    placement = placement === "top-end" ? "bottom-end" : "bottom-start";
  } else if (placement === "right-start" && overlay.width > availableRight && availableLeft > availableRight) {
    placement = "left-start";
  } else if (placement === "left-start" && overlay.width > availableLeft && availableRight > availableLeft) {
    placement = "right-start";
  }

  const isTop = placement.startsWith("top");
  const isBottom = placement.startsWith("bottom");
  const isEnd = placement.endsWith("end");
  const rawLeft = placement === "right-start"
    ? trigger.right + offset
    : placement === "left-start"
      ? trigger.left - overlay.width - offset
      : isEnd
        ? trigger.right - overlay.width
        : trigger.left;
  const rawTop = isTop
    ? trigger.top - overlay.height - offset
    : isBottom
      ? trigger.bottom + offset
      : trigger.top;
  const left = Math.min(
    Math.max(gutter, rawLeft),
    Math.max(gutter, viewport.width - gutter - Math.min(overlay.width, maxWidth))
  );
  const isSidePlacement = placement === "left-start" || placement === "right-start";
  const sideOverlayHeight = Math.min(overlay.height, Math.max(0, viewport.height - gutter * 2));
  const top = isSidePlacement
    ? Math.min(
        Math.max(gutter, rawTop),
        Math.max(gutter, viewport.height - gutter - sideOverlayHeight)
      )
    : Math.max(gutter, rawTop);
  const maxHeight = Math.max(
    0,
    placement.startsWith("top")
      ? trigger.top - offset - gutter
      : placement.startsWith("bottom")
        ? viewport.height - trigger.bottom - offset - gutter
        : viewport.height - top - gutter
  );

  return { left, top, maxWidth, maxHeight, placement };
}

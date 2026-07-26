import type {
  OverlayPlacement,
  OverlayPositionResult,
  OverlayRect,
  OverlaySize,
  OverlayViewport,
  OverlayVirtualAnchor
} from "./overlayTypes";

export interface ResolveOverlayPositionInput {
  trigger: OverlayRect;
  overlay: OverlaySize;
  viewport: OverlayViewport;
  preferredPlacement: OverlayPlacement;
  offset: number;
  gutter: number;
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
  return { getBoundingClientRect: () => value };
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
  const top = Math.max(gutter, rawTop);
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

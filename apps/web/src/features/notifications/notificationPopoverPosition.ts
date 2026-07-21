export interface PopoverRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface PopoverSize {
  width: number;
  height: number;
}

export interface PopoverViewport {
  width: number;
  height: number;
}

export interface NotificationPopoverPosition {
  left: number;
  top: number;
  maxHeight: number;
}

export function resolveNotificationPopoverPosition(input: {
  anchor: PopoverRect;
  popover: PopoverSize;
  viewport: PopoverViewport;
  gap: number;
  margin: number;
}): NotificationPopoverPosition {
  const margin = Math.max(0, input.margin);
  const gap = Math.max(0, input.gap);
  const top = Math.max(margin, input.anchor.bottom + gap);
  const maximumLeft = Math.max(margin, input.viewport.width - margin - input.popover.width);
  const preferredLeft = input.anchor.right - input.popover.width;
  return {
    left: clamp(preferredLeft, margin, maximumLeft),
    top,
    maxHeight: Math.max(0, input.viewport.height - top - margin)
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

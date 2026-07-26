export interface OverlayIntrinsicWidthSurface {
  style: {
    width: string;
    maxWidth: string;
  };
  getBoundingClientRect(): { width: number };
}

export function measureOverlayIntrinsicWidth(
  surface: OverlayIntrinsicWidthSurface
): number {
  const previousWidth = surface.style.width;
  const previousMaxWidth = surface.style.maxWidth;
  try {
    surface.style.maxWidth = "none";
    surface.style.width = "max-content";
    return Math.max(0, surface.getBoundingClientRect().width);
  } finally {
    surface.style.width = previousWidth;
    surface.style.maxWidth = previousMaxWidth;
  }
}

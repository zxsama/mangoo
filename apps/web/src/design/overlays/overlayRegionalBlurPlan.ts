import type { OverlayLayerKind } from "./overlayTypes";

export interface OverlayRegionalRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface OverlayRegionalLayerGeometry {
  id: string;
  kind: OverlayLayerKind;
  radius: number;
  rect: OverlayRegionalRect;
}

export interface OverlayRegionalTargetGeometry {
  id: string;
  layerIndex: number;
  rect: OverlayRegionalRect;
}

export interface OverlayRegionalMask {
  left: number;
  top: number;
  width: number;
  height: number;
  radius: number;
}

export interface OverlayRegionalBlurPlan {
  targetId: string;
  targetRect: OverlayRegionalRect;
  masks: OverlayRegionalMask[];
  blurBounds: {
    left: number;
    top: number;
    width: number;
    height: number;
  };
}

export interface OverlayRegionalBlurPlanInput {
  sigma: number;
  targets: readonly OverlayRegionalTargetGeometry[];
  layers: readonly OverlayRegionalLayerGeometry[];
}

function intersectRects(
  left: OverlayRegionalRect,
  right: OverlayRegionalRect
): OverlayRegionalRect | null {
  const intersectionLeft = Math.max(left.left, right.left);
  const intersectionTop = Math.max(left.top, right.top);
  const intersectionRight = Math.min(left.right, right.right);
  const intersectionBottom = Math.min(left.bottom, right.bottom);
  const width = intersectionRight - intersectionLeft;
  const height = intersectionBottom - intersectionTop;
  if (width <= 0 || height <= 0) return null;
  return {
    left: intersectionLeft,
    top: intersectionTop,
    right: intersectionRight,
    bottom: intersectionBottom,
    width,
    height
  };
}

function fullTargetMask(target: OverlayRegionalTargetGeometry): OverlayRegionalMask {
  return {
    left: 0,
    top: 0,
    width: target.rect.width,
    height: target.rect.height,
    radius: 0
  };
}

function layerMask(
  target: OverlayRegionalTargetGeometry,
  layer: OverlayRegionalLayerGeometry
): OverlayRegionalMask | null {
  const intersection = intersectRects(target.rect, layer.rect);
  if (!intersection) return null;
  return {
    left: intersection.left - target.rect.left,
    top: intersection.top - target.rect.top,
    width: intersection.width,
    height: intersection.height,
    radius: Math.max(
      0,
      Math.min(layer.radius, intersection.width / 2, intersection.height / 2)
    )
  };
}

function blurBounds(
  masks: readonly OverlayRegionalMask[],
  target: OverlayRegionalTargetGeometry,
  sigma: number
): OverlayRegionalBlurPlan["blurBounds"] {
  const expansion = sigma * 3;
  const left = Math.max(0, Math.min(...masks.map((mask) => mask.left)) - expansion);
  const top = Math.max(0, Math.min(...masks.map((mask) => mask.top)) - expansion);
  const right = Math.min(
    target.rect.width,
    Math.max(...masks.map((mask) => mask.left + mask.width)) + expansion
  );
  const bottom = Math.min(
    target.rect.height,
    Math.max(...masks.map((mask) => mask.top + mask.height)) + expansion
  );
  return { left, top, width: right - left, height: bottom - top };
}

export function buildOverlayRegionalBlurPlans({
  sigma,
  targets,
  layers
}: OverlayRegionalBlurPlanInput): OverlayRegionalBlurPlan[] {
  if (!Number.isFinite(sigma) || sigma <= 0) return [];

  const plans: OverlayRegionalBlurPlan[] = [];
  for (const target of targets) {
    if (target.rect.width <= 0 || target.rect.height <= 0) continue;
    const higherLayers = layers.slice(target.layerIndex + 1);
    if (higherLayers.length === 0) continue;

    const masks = higherLayers.some((layer) => layer.kind === "modal")
      ? [fullTargetMask(target)]
      : higherLayers
          .map((layer) => layerMask(target, layer))
          .filter((mask): mask is OverlayRegionalMask => mask !== null);
    if (masks.length === 0) continue;

    plans.push({
      targetId: target.id,
      targetRect: target.rect,
      masks,
      blurBounds: blurBounds(masks, target, sigma)
    });
  }
  return plans;
}

function serializeNumber(value: number): string {
  return String(Number(value.toFixed(3)));
}

export function buildOverlayRegionalMaskSvg(plan: OverlayRegionalBlurPlan): string {
  const width = serializeNumber(plan.targetRect.width);
  const height = serializeNumber(plan.targetRect.height);
  const rectangles = plan.masks.map((mask) => {
    const radius = serializeNumber(mask.radius);
    return `<rect x="${serializeNumber(mask.left)}" y="${serializeNumber(mask.top)}" width="${serializeNumber(mask.width)}" height="${serializeNumber(mask.height)}" rx="${radius}" ry="${radius}" fill="white"/>`;
  }).join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}">${rectangles}</svg>`;
}

import {
  buildOverlayRegionalBlurPlans,
  type OverlayRegionalBlurPlan,
  type OverlayRegionalLayerGeometry,
  type OverlayRegionalMask,
  type OverlayRegionalRect,
  type OverlayRegionalTargetGeometry
} from "./overlayRegionalBlurPlan";
import type { OverlayLayerRegistration } from "./overlayTypes";

const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const APP_ROOT_TARGET_ID = "app-root";
const SOLID_FALLBACK_FILL = "--overlay-solid-fallback-fill";
const ROUNDED_MASK_SIGMA_RATIO = 0.54;
const ROUNDED_MASK_ALPHA_SLOPE = 16;
let controllerSequence = 0;

export interface OverlayRegionalBlurController {
  setLayers(layers: readonly OverlayLayerRegistration[]): void;
  destroy(): void;
}

interface RegionalTarget extends OverlayRegionalTargetGeometry {
  element: HTMLElement;
}

interface PreparedRegionalFilter {
  filterId: string;
  plan: OverlayRegionalBlurPlan;
  target: RegionalTarget;
}

function toRegionalRect(rect: DOMRect): OverlayRegionalRect {
  return {
    left: rect.left,
    top: rect.top,
    right: rect.right,
    bottom: rect.bottom,
    width: rect.width,
    height: rect.height
  };
}

function cssNumber(value: string, fallback: number): number {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function setAttributes(element: Element, attributes: Record<string, string | number>): void {
  for (const [name, value] of Object.entries(attributes)) {
    element.setAttribute(name, String(value));
  }
}

function isFullTargetPlan(plan: OverlayRegionalBlurPlan): boolean {
  if (plan.masks.length !== 1) return false;
  const [mask] = plan.masks;
  return (
    mask.left === 0 &&
    mask.top === 0 &&
    mask.width === plan.targetRect.width &&
    mask.height === plan.targetRect.height
  );
}

function svgElement<K extends keyof SVGElementTagNameMap>(
  ownerDocument: Document,
  name: K
): SVGElementTagNameMap[K] {
  return ownerDocument.createElementNS(SVG_NAMESPACE, name);
}

function maskPrimitiveBounds(mask: OverlayRegionalMask, sigma: number) {
  const expansion = sigma * 3;
  return {
    x: mask.left - expansion,
    y: mask.top - expansion,
    width: mask.width + expansion * 2,
    height: mask.height + expansion * 2
  };
}

function appendRoundedMaskPrimitives(
  ownerDocument: Document,
  filter: SVGFilterElement,
  plan: OverlayRegionalBlurPlan
): string {
  const maskResults = plan.masks.map((mask, index) => {
    const sourceResult = `regional-mask-${index}-source`;
    const flood = svgElement(ownerDocument, "feFlood");
    setAttributes(flood, {
      x: mask.left,
      y: mask.top,
      width: mask.width,
      height: mask.height,
      "flood-color": "white",
      "flood-opacity": 1,
      result: sourceResult,
      "data-overlay-regional-mask-primitive": "true"
    });
    filter.append(flood);
    if (mask.radius <= 0) return sourceResult;

    const maskSigma = mask.radius * ROUNDED_MASK_SIGMA_RATIO;
    const bounds = maskPrimitiveBounds(mask, maskSigma);
    const softenedResult = `regional-mask-${index}-softened`;
    const softened = svgElement(ownerDocument, "feGaussianBlur");
    setAttributes(softened, {
      in: sourceResult,
      stdDeviation: maskSigma,
      edgeMode: "none",
      ...bounds,
      result: softenedResult,
      "data-overlay-regional-mask-rounding": "true"
    });

    const roundedUnclippedResult = `regional-mask-${index}-rounded`;
    const rounded = svgElement(ownerDocument, "feComponentTransfer");
    setAttributes(rounded, { in: softenedResult, ...bounds, result: roundedUnclippedResult });
    const alpha = svgElement(ownerDocument, "feFuncA");
    setAttributes(alpha, {
      type: "linear",
      slope: ROUNDED_MASK_ALPHA_SLOPE,
      intercept: -(ROUNDED_MASK_ALPHA_SLOPE - 1) / 2
    });
    rounded.append(alpha);
    const clippedResult = `regional-mask-${index}-clipped`;
    const clipped = svgElement(ownerDocument, "feComposite");
    setAttributes(clipped, {
      in: roundedUnclippedResult,
      in2: sourceResult,
      operator: "in",
      result: clippedResult
    });
    filter.append(softened, rounded, clipped);
    return clippedResult;
  });

  if (maskResults.length === 1) return maskResults[0];
  const mergedResult = "regional-mask-union";
  const union = svgElement(ownerDocument, "feMerge");
  union.setAttribute("result", mergedResult);
  for (const input of maskResults) {
    const node = svgElement(ownerDocument, "feMergeNode");
    node.setAttribute("in", input);
    union.append(node);
  }
  filter.append(union);
  return mergedResult;
}

function buildFilterElement(
  ownerDocument: Document,
  prepared: PreparedRegionalFilter,
  sigma: number,
  saturation: number
): SVGFilterElement {
  const { filterId, plan } = prepared;
  const filter = svgElement(ownerDocument, "filter");
  setAttributes(filter, {
    id: filterId,
    x: 0,
    y: 0,
    width: plan.targetRect.width,
    height: plan.targetRect.height,
    filterUnits: "userSpaceOnUse",
    primitiveUnits: "userSpaceOnUse",
    "color-interpolation-filters": "sRGB",
    "data-overlay-regional-blur-target": plan.targetId,
    "data-overlay-regional-full-target": isFullTargetPlan(plan) ? "true" : "false",
    "data-overlay-regional-mask-count": plan.masks.length
  });

  const blur = svgElement(ownerDocument, "feGaussianBlur");
  setAttributes(blur, {
    in: "SourceGraphic",
    stdDeviation: sigma,
    edgeMode: "duplicate",
    x: plan.blurBounds.left,
    y: plan.blurBounds.top,
    width: plan.blurBounds.width,
    height: plan.blurBounds.height,
    result: "blurred"
  });

  const saturated = svgElement(ownerDocument, "feColorMatrix");
  setAttributes(saturated, {
    in: "blurred",
    type: "saturate",
    values: saturation,
    x: plan.blurBounds.left,
    y: plan.blurBounds.top,
    width: plan.blurBounds.width,
    height: plan.blurBounds.height,
    result: "saturated"
  });

  filter.append(blur, saturated);
  const maskResult = appendRoundedMaskPrimitives(ownerDocument, filter, plan);

  const blurredInside = svgElement(ownerDocument, "feComposite");
  setAttributes(blurredInside, {
    in: "saturated",
    in2: maskResult,
    operator: "in",
    result: "blurred-inside"
  });

  const clearOutside = svgElement(ownerDocument, "feComposite");
  setAttributes(clearOutside, {
    in: "SourceGraphic",
    in2: maskResult,
    operator: "out",
    result: "clear-outside"
  });

  const merge = svgElement(ownerDocument, "feMerge");
  for (const input of ["clear-outside", "blurred-inside"]) {
    const node = svgElement(ownerDocument, "feMergeNode");
    node.setAttribute("in", input);
    merge.append(node);
  }

  filter.append(blurredInside, clearOutside, merge);
  return filter;
}

function stableSurfaceRect(
  element: HTMLElement,
  view: Window,
  style = view.getComputedStyle(element)
): OverlayRegionalRect {
  const rect = element.getBoundingClientRect();
  const transform = style.transform;
  if (!transform || transform === "none") return toRegionalRect(rect);
  try {
    const matrix = new DOMMatrixReadOnly(transform);
    const axisAligned = Math.abs(matrix.m12) < 0.001 && Math.abs(matrix.m21) < 0.001;
    if (axisAligned && matrix.m11 > 0.001 && matrix.m22 > 0.001) {
      const width = rect.width / matrix.m11;
      const height = rect.height / matrix.m22;
      const [originXValue = "0", originYValue = "0"] = style.transformOrigin.split(/\s+/);
      const originX = cssNumber(originXValue, width / 2);
      const originY = cssNumber(originYValue, height / 2);
      const left = rect.left - matrix.m41 - originX * (1 - matrix.m11);
      const top = rect.top - matrix.m42 - originY * (1 - matrix.m22);
      return {
        left,
        top,
        right: left + width,
        bottom: top + height,
        width,
        height
      };
    }
    return {
      left: rect.left - matrix.m41,
      top: rect.top - matrix.m42,
      right: rect.right - matrix.m41,
      bottom: rect.bottom - matrix.m42,
      width: rect.width,
      height: rect.height
    };
  } catch {
    return toRegionalRect(rect);
  }
}

export function createOverlayRegionalBlurController(
  ownerDocument: Document
): OverlayRegionalBlurController {
  const view = ownerDocument.defaultView;
  if (!view) {
    return { setLayers: () => undefined, destroy: () => undefined };
  }
  const runtimeView = view;

  const sequence = ++controllerSequence;
  let currentLayers: readonly OverlayLayerRegistration[] = [];
  let destroyed = false;
  let scheduledFrame = 0;
  let filterRevision = 0;
  let installedSignature: string | null = null;
  let definitions: SVGSVGElement | null = null;
  const previousInlineFilters = new Map<HTMLElement, string>();
  const observedSurfaces = new Set<HTMLElement>();
  const elementIds = new WeakMap<HTMLElement, number>();
  let nextElementId = 0;

  const elementIdentity = (element: HTMLElement): number => {
    const existing = elementIds.get(element);
    if (existing !== undefined) return existing;
    const identity = ++nextElementId;
    elementIds.set(element, identity);
    return identity;
  };

  const restoreTarget = (target: HTMLElement) => {
    const previous = previousInlineFilters.get(target);
    if (previous === undefined) return;
    target.style.filter = previous;
    target.removeAttribute("data-overlay-regional-blur");
    previousInlineFilters.delete(target);
  };

  const clearAppliedFilters = () => {
    for (const target of [...previousInlineFilters.keys()]) restoreTarget(target);
    definitions?.remove();
    definitions = null;
    installedSignature = null;
  };

  const resizeObserver = typeof view.ResizeObserver === "function"
    ? new view.ResizeObserver(() => scheduleRefresh())
    : null;
  const rootMutationObserver = typeof view.MutationObserver === "function"
    ? new view.MutationObserver(() => refreshImmediately())
    : null;
  const surfaceMutationObserver = typeof view.MutationObserver === "function"
    ? new view.MutationObserver(() => refreshImmediately())
    : null;
  const reducedTransparency = view.matchMedia?.("(prefers-reduced-transparency: reduce)") ?? null;

  const handleGeometrySettled = (event?: Event) => {
    if (event?.currentTarget instanceof HTMLElement && event.target !== event.currentTarget) return;
    refreshImmediately();
  };

  const handleWindowResize = () => scheduleRefresh();

  const handleScroll = (event: Event) => {
    if (
      event.target instanceof Node &&
      currentLayers.some((layer) => layer.surface?.contains(event.target as Node))
    ) {
      return;
    }
    scheduleRefresh();
  };

  const cancelScheduledFrame = () => {
    if (scheduledFrame) view.cancelAnimationFrame(scheduledFrame);
    scheduledFrame = 0;
  };

  function scheduleRefresh(): void {
    if (destroyed || scheduledFrame) return;
    scheduledFrame = runtimeView.requestAnimationFrame(() => {
      scheduledFrame = 0;
      refreshImmediately();
    });
  }

  const regionalFilterSupported = () => (
    typeof view.CSS?.supports === "function" &&
    view.CSS.supports("filter", 'url("#megle-overlay-regional-blur-support")')
  );

  const materialSettings = () => {
    const rootStyle = view.getComputedStyle(ownerDocument.documentElement);
    return {
      fallback: rootStyle.getPropertyValue(SOLID_FALLBACK_FILL).trim() !== "",
      saturation: Math.max(
        0,
        cssNumber(rootStyle.getPropertyValue("--glass-center-saturation"), 1)
      ),
      sigma: Math.max(
        0,
        Math.min(128, cssNumber(rootStyle.getPropertyValue("--glass-dialog-blur"), 0))
      )
    };
  };

  const filteringDisabled = (settings = materialSettings()) => (
    settings.sigma <= 0 ||
    settings.fallback ||
    reducedTransparency?.matches === true ||
    !regionalFilterSupported()
  );

  const collectGeometry = () => {
    const appRoot = ownerDocument.getElementById("root");
    if (!(appRoot instanceof view.HTMLElement)) return null;
    const visibleLayers: Array<{
      layer: OverlayLayerRegistration & { surface: HTMLElement };
      rect: OverlayRegionalRect;
      style: CSSStyleDeclaration;
    }> = [];
    for (const layer of currentLayers) {
      const surface = layer.surface;
      if (!(surface instanceof view.HTMLElement) || !surface.isConnected) continue;
      const style = view.getComputedStyle(surface);
      if (style.visibility === "hidden" || style.display === "none") continue;
      const rect = stableSurfaceRect(surface, view, style);
      if (
        !Number.isFinite(rect.left) ||
        !Number.isFinite(rect.top) ||
        !Number.isFinite(rect.right) ||
        !Number.isFinite(rect.bottom) ||
        !Number.isFinite(rect.width) ||
        !Number.isFinite(rect.height) ||
        rect.width <= 0 ||
        rect.height <= 0
      ) {
        continue;
      }
      visibleLayers.push({
        layer: layer as OverlayLayerRegistration & { surface: HTMLElement },
        rect,
        style
      });
    }
    if (visibleLayers.length === 0) return null;

    const appRootRect = toRegionalRect(appRoot.getBoundingClientRect());
    if (
      !Number.isFinite(appRootRect.left) ||
      !Number.isFinite(appRootRect.top) ||
      !Number.isFinite(appRootRect.right) ||
      !Number.isFinite(appRootRect.bottom) ||
      appRootRect.width <= 0 ||
      appRootRect.height <= 0
    ) {
      return null;
    }
    const layerGeometry: OverlayRegionalLayerGeometry[] = visibleLayers.map(({ layer, rect, style }) => ({
      id: layer.id,
      kind: layer.kind,
      radius: Math.max(0, cssNumber(style.borderTopLeftRadius, 0)),
      rect
    }));
    const targets: RegionalTarget[] = [{
      id: APP_ROOT_TARGET_ID,
      layerIndex: -1,
      rect: appRootRect,
      element: appRoot
    }];
    for (let index = 0; index < visibleLayers.length - 1; index += 1) {
      const { layer, rect } = visibleLayers[index];
      targets.push({
        id: `layer:${layer.id}`,
        layerIndex: index,
        rect,
        element: layer.surface
      });
    }
    return { layerGeometry, targets };
  };

  const buildSnapshot = () => {
    const settings = materialSettings();
    if (filteringDisabled(settings)) return null;
    const geometry = collectGeometry();
    if (!geometry) return null;
    const plans = buildOverlayRegionalBlurPlans({
      sigma: settings.sigma,
      targets: geometry.targets,
      layers: geometry.layerGeometry
    });
    if (plans.length === 0) return null;
    const signature = JSON.stringify({
      sigma: settings.sigma,
      saturation: settings.saturation,
      layers: geometry.layerGeometry,
      targets: geometry.targets.map((target) => ({
        id: target.id,
        layerIndex: target.layerIndex,
        rect: target.rect,
        element: elementIdentity(target.element)
      })),
      plans
    }, (_key, value) => (
      typeof value === "number" ? Number(value.toFixed(3)) : value
    ));
    return { geometry, plans, settings, signature };
  };

  const installPreparedFilters = (
    preparedFilters: readonly PreparedRegionalFilter[],
    sigma: number,
    saturation: number
  ) => {
    const nextDefinitions = svgElement(ownerDocument, "svg");
    setAttributes(nextDefinitions, {
      "aria-hidden": "true",
      "data-overlay-regional-blur-defs": "true",
      focusable: "false",
      width: 0,
      height: 0
    });
    nextDefinitions.style.position = "fixed";
    nextDefinitions.style.pointerEvents = "none";
    const defs = svgElement(ownerDocument, "defs");
    for (const prepared of preparedFilters) {
      defs.append(buildFilterElement(ownerDocument, prepared, sigma, saturation));
    }
    nextDefinitions.append(defs);
    ownerDocument.body.prepend(nextDefinitions);

    const nextTargets = new Set(preparedFilters.map((prepared) => prepared.target.element));
    for (const target of [...previousInlineFilters.keys()]) {
      if (!nextTargets.has(target)) restoreTarget(target);
    }
    for (const prepared of preparedFilters) {
      const { element } = prepared.target;
      if (!previousInlineFilters.has(element)) {
        previousInlineFilters.set(element, element.style.filter);
      }
      element.style.filter = `url("#${prepared.filterId}")`;
      element.setAttribute("data-overlay-regional-blur", "active");
    }

    definitions?.remove();
    definitions = nextDefinitions;
  };

  function refreshImmediately(): void {
    if (destroyed) return;
    cancelScheduledFrame();
    try {
      const snapshot = buildSnapshot();
      if (!snapshot) {
        clearAppliedFilters();
        return;
      }
      if (snapshot.signature === installedSignature && definitions?.isConnected) {
        return;
      }

      const targetById = new Map(
        snapshot.geometry.targets.map((target) => [target.id, target])
      );
      const graphRevision = ++filterRevision;
      const preparedFilters = snapshot.plans.map((plan, index): PreparedRegionalFilter => {
        const target = targetById.get(plan.targetId);
        if (!target) throw new Error(`regional blur target is missing: ${plan.targetId}`);
        return {
          filterId: `megle-overlay-regional-blur-${sequence}-${graphRevision}-${index}`,
          plan,
          target
        };
      });
      installPreparedFilters(
        preparedFilters,
        snapshot.settings.sigma,
        snapshot.settings.saturation
      );
      installedSignature = snapshot.signature;
    } catch {
      clearAppliedFilters();
    }
  }

  const resetSurfaceObservers = () => {
    resizeObserver?.disconnect();
    surfaceMutationObserver?.disconnect();
    const appRoot = ownerDocument.getElementById("root");
    if (appRoot instanceof view.HTMLElement) resizeObserver?.observe(appRoot);
    for (const surface of observedSurfaces) {
      surface.removeEventListener("animationend", handleGeometrySettled);
      surface.removeEventListener("transitionend", handleGeometrySettled);
    }
    observedSurfaces.clear();
    for (const layer of currentLayers) {
      const surface = layer.surface;
      if (!(surface instanceof view.HTMLElement)) continue;
      resizeObserver?.observe(surface);
      surface.addEventListener("animationend", handleGeometrySettled);
      surface.addEventListener("transitionend", handleGeometrySettled);
      surfaceMutationObserver?.observe(surface, {
        attributes: true,
        attributeFilter: ["style", "class", "data-overlay-state"]
      });
      observedSurfaces.add(surface);
    }
  };

  rootMutationObserver?.observe(ownerDocument.documentElement, {
    attributes: true,
    attributeFilter: ["style", "data-overlay-solid-fallback"]
  });
  view.addEventListener("resize", handleWindowResize, { passive: true });
  view.addEventListener("scroll", handleScroll, { capture: true, passive: true });
  reducedTransparency?.addEventListener?.("change", handleGeometrySettled);

  return {
    setLayers(layers) {
      if (destroyed) return;
      currentLayers = [...layers];
      resetSurfaceObservers();
      refreshImmediately();
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      cancelScheduledFrame();
      resizeObserver?.disconnect();
      rootMutationObserver?.disconnect();
      surfaceMutationObserver?.disconnect();
      for (const surface of observedSurfaces) {
        surface.removeEventListener("animationend", handleGeometrySettled);
        surface.removeEventListener("transitionend", handleGeometrySettled);
      }
      observedSurfaces.clear();
      view.removeEventListener("resize", handleWindowResize);
      view.removeEventListener("scroll", handleScroll, true);
      reducedTransparency?.removeEventListener?.("change", handleGeometrySettled);
      clearAppliedFilters();
      currentLayers = [];
    }
  };
}

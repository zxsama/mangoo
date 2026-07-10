import {
  createElement,
  forwardRef,
  useEffect,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type HTMLAttributes,
  type PointerEvent as ReactPointerEvent,
  type ReactNode
} from "react";

type LiquidGlassTag =
  | "article"
  | "aside"
  | "div"
  | "header"
  | "main"
  | "nav"
  | "section";

interface LiquidGlassSurfaceProps extends HTMLAttributes<HTMLElement> {
  as?: LiquidGlassTag;
  variant?: "regular" | "clear";
  tone?: "chrome" | "panel" | "elevated" | "control" | "primary" | "danger";
  interactive?: boolean;
  interactiveBackground?: boolean;
  backgroundGlow?: boolean;
  outlineOnly?: boolean;
  pressable?: boolean;
  scrollable?: boolean;
  active?: boolean;
  children?: ReactNode;
}

interface LiquidGlassButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "regular" | "clear";
  tone?: "chrome" | "panel" | "elevated" | "control" | "primary" | "danger";
  backgroundGlow?: boolean;
  active?: boolean;
}

const POINTER_RESET = "50%";
const POINTER_OPACITY_HIDDEN = "0";
// Fallback cursor-proximity radius (px) used only before interfaceStyle writes the
// `--glass-pointer-sensing-distance` var. The live, settings-adjustable radius is
// read from that var each frame in pointerEdgeProximity().
const GLASS_POINTER_EDGE_PROXIMITY_PX = 112;

// Per-element bounding-rect cache for the interactive-affordance hover glow.
// The glow recomputes on every pointer-move frame; without a cache it called
// getBoundingClientRect() on EVERY affordance target (every .tile-thumb — hundreds
// in the media grid) each frame, forcing a synchronous layout each time. During
// thumbnail generation the grid mutates constantly, so those forced reflows were
// never free — measured ~2s of getBoundingClientRect over a 6s span (~48% main-
// thread blocked = the reported jank). Target POSITIONS only change on scroll or
// resize (the grid is virtualized with fixed geometry), NOT when a tile's content
// swaps loading->image, so caching rects and invalidating only on scroll/resize is
// correct and turns the per-frame cost from O(targets) reflows into cheap math.
let affordanceRectCache = new WeakMap<HTMLElement, DOMRect>();

function invalidateAffordanceRectCache() {
  affordanceRectCache = new WeakMap();
}

function cachedBoundingRect(target: HTMLElement): DOMRect {
  const cached = affordanceRectCache.get(target);
  if (cached) return cached;
  const rect = target.getBoundingClientRect();
  affordanceRectCache.set(target, rect);
  return rect;
}
const INTERACTIVE_POINTER_TARGET_SELECTOR_ATTRIBUTE = "data-interactive-pointer-target-selector";
const INTERACTIVE_AFFORDANCE_OWNER_SELECTOR = [
  ".tree-item",
  ".tile-thumb",
  ".settings-style-slider-control",
  // The column-resize divider: drive its edge highlight from the same
  // pointer-distance calc as everything else (was a binary :hover) so its glow
  // fades in with cursor proximity like the rest of the UI.
  ".shell-column-resizer",
  // Clickable list rows / cards. These were previously highlighted by their own
  // rectangular :hover border+background; they now route through the single
  // circular pointer-distance glow like every other interactive element. Disabled
  // move rows opt out via :not(.move-row-disabled).
  ".move-row:not(.move-row-disabled)",
  ".plugin-card",
  ".task-card",
  ".settings-dialog-nav-item",
  ".settings-radio",
  // Glass BUTTONS (LiquidGlassButton renders data-liquid-glass="button"). They
  // are unified into the single circular pointer-distance glow like every other
  // control. Glass SURFACES/panels (data-liquid-glass="surface") are NOT included
  // — panels/background never glow, only clickable elements do.
  '[data-liquid-glass="button"]'
].join(",");
const INTERACTIVE_AFFORDANCE_SELECTOR = [
  INTERACTIVE_AFFORDANCE_OWNER_SELECTOR,
  'button:not([data-liquid-glass])',
  'input:not([type="range"]):not([data-liquid-glass])',
  'select:not([data-liquid-glass])',
  'textarea:not([data-liquid-glass])'
].join(",");

export function LiquidGlassLayer({ children }: { children: ReactNode }) {
  useEffect(() => {
    let animationFrame = 0;
    let latestPointer: { x: number; y: number } | null = null;

    function clearPointerFrame() {
      latestPointer = null;
      if (animationFrame) {
        window.cancelAnimationFrame(animationFrame);
        animationFrame = 0;
      }
    }

    function requestPointerUpdate(x: number, y: number) {
      latestPointer = { x, y };
      if (animationFrame) return;
        animationFrame = window.requestAnimationFrame(() => {
          animationFrame = 0;
          if (latestPointer) {
            updateInteractiveAffordancePointers(latestPointer.x, latestPointer.y);
          }
        });
    }

    function handlePointerMove(event: globalThis.PointerEvent) {
      requestPointerUpdate(event.clientX, event.clientY);
    }

    function handleMouseMove(event: globalThis.MouseEvent) {
      requestPointerUpdate(event.clientX, event.clientY);
    }

    function handlePointerExit(event?: Event) {
      if (event && pointerEventInsideViewport(event)) {
        return;
      }
      clearPointerFrame();
      hideInteractiveAffordancePointers();
    }

    window.addEventListener("pointermove", handlePointerMove, { passive: true });
    window.addEventListener("mousemove", handleMouseMove, { passive: true });
    window.addEventListener("pointerleave", handlePointerExit);
    window.addEventListener("blur", handlePointerExit);
    // Affordance target rects are cached across pointer-move frames; their
    // POSITIONS only change on scroll (capture phase catches inner scrollers like
    // the virtualized media grid) or resize, so invalidate the cache then. This
    // keeps the per-frame hover glow O(1) instead of O(targets) forced reflows.
    window.addEventListener("scroll", invalidateAffordanceRectCache, { capture: true, passive: true });
    window.addEventListener("resize", invalidateAffordanceRectCache, { passive: true });

    return () => {
      clearPointerFrame();
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("pointerleave", handlePointerExit);
      window.removeEventListener("blur", handlePointerExit);
      window.removeEventListener("scroll", invalidateAffordanceRectCache, { capture: true });
      window.removeEventListener("resize", invalidateAffordanceRectCache);
      hideInteractiveAffordancePointers();
    };
  }, []);

  return (
    <>
      <LiquidGlassRefractionDefs />
      {children}
    </>
  );
}

export const LiquidGlassSurface = forwardRef<HTMLElement, LiquidGlassSurfaceProps>(
  function LiquidGlassSurface(
    {
      active = false,
      as = "div",
      children,
      className,
      backgroundGlow,
      interactive = false,
      interactiveBackground = interactive,
      onPointerDown,
      onPointerLeave,
      onPointerMove,
      onPointerUp,
      outlineOnly = false,
      pressable = false,
      scrollable = false,
      style,
      tone = "panel",
      variant = "regular",
      ...props
    },
    ref
  ) {
    const showBackgroundGlow =
      interactiveBackground && (backgroundGlow ?? (interactive && tone === "control"));
    const eventProps = buildPointerEventProps({
      interactive,
      pressable,
      onPointerDown,
      onPointerLeave,
      onPointerMove,
      onPointerUp
    });

    return createElement(
      as,
      {
        ...props,
        ...eventProps,
        className: liquidGlassClassName({ active, className, scrollable, tone, variant }),
        "data-glass-active": active ? "true" : undefined,
        "data-glass-background-glow": showBackgroundGlow ? "true" : undefined,
        "data-glass-interactive": interactive ? "true" : undefined,
        "data-glass-interactive-background": interactiveBackground ? "true" : "false",
        "data-glass-outline-only": outlineOnly ? "true" : undefined,
        "data-glass-pressable": pressable ? "true" : undefined,
        "data-glass-scrollable": scrollable ? "true" : undefined,
        "data-glass-tone": tone,
        "data-glass-variant": variant,
        "data-liquid-glass": "surface",
        ref,
        style: withInitialPointer(style)
      },
      <LiquidGlassMaterialLayers outlineOnly={outlineOnly} variant={variant} />,
      scrollable ? <div className="liquid-glass-content">{children}</div> : children
    );
  }
);

export const LiquidGlassButton = forwardRef<HTMLButtonElement, LiquidGlassButtonProps>(
  function LiquidGlassButton(
    {
      active = false,
      backgroundGlow,
      children,
      className,
      onPointerDown,
      onPointerLeave,
      onPointerMove,
      onPointerUp,
      style,
      tone = "control",
      type = "button",
      variant = "regular",
      ...props
    },
    ref
  ) {
    const showBackgroundGlow = backgroundGlow ?? !props.disabled;
    const eventProps = buildPointerEventProps<HTMLButtonElement>({
      interactive: !props.disabled,
      pressable: !props.disabled,
      onPointerDown,
      onPointerLeave,
      onPointerMove,
      onPointerUp
    });

    return (
      <button
        {...props}
        {...eventProps}
        className={liquidGlassClassName(
          { active, className, scrollable: false, tone, variant },
          "liquid-glass-button no-drag"
        )}
        data-glass-active={active ? "true" : undefined}
        data-glass-background-glow={showBackgroundGlow && !props.disabled ? "true" : undefined}
        data-glass-interactive={props.disabled ? undefined : "true"}
        data-glass-pressable={props.disabled ? undefined : "true"}
        data-no-drag="true"
        data-glass-tone={tone}
        data-glass-variant={variant}
        data-liquid-glass="button"
        ref={ref}
        style={withInitialPointer(style)}
        type={type}
      >
        <LiquidGlassMaterialLayers variant={variant} />
        <span className="liquid-glass-content">{children}</span>
      </button>
    );
  }
);

function LiquidGlassMaterialLayers({
  outlineOnly = false,
  variant
}: {
  outlineOnly?: boolean;
  variant: "regular" | "clear";
}) {
  // Keep a sharp child content layer above the refractive backdrop/lens layers.
  return (
    <>
      {variant === "clear" && !outlineOnly ? (
        <span
          aria-hidden="true"
          className="liquid-glass-dim"
          data-glass-variant="clear"
        />
      ) : null}
      {outlineOnly ? null : <span aria-hidden="true" className="liquid-glass-backdrop" />}
      {outlineOnly ? null : <span aria-hidden="true" className="liquid-glass-lens" />}
      {outlineOnly ? null : <span aria-hidden="true" className="liquid-glass-dither" />}
      <span aria-hidden="true" className="liquid-glass-edge" />
    </>
  );
}

function LiquidGlassRefractionDefs() {
  return (
    <svg
      aria-hidden="true"
      className="liquid-glass-defs"
      focusable="false"
      height="0"
      width="0"
    >
      <defs>
        <filter
          id="megle-liquid-glass-refraction"
          colorInterpolationFilters="sRGB"
          x="-20%"
          y="-20%"
          width="140%"
          height="140%"
        >
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.011 0.017"
            numOctaves="2"
            seed="17"
            result="glassNoise"
          />
          <feGaussianBlur in="glassNoise" stdDeviation="0.45" result="softGlassNoise" />
          <feDisplacementMap
            in="SourceGraphic"
            in2="softGlassNoise"
            scale="14"
            xChannelSelector="R"
            yChannelSelector="G"
            result="refracted"
          />
          <feColorMatrix
            in="refracted"
            type="matrix"
            values="1.015 0 0 0 0  0 1.025 0 0 0  0 0 1.05 0 0  0 0 0 1 0"
            result="chromaticLift"
          />
          <feComposite in="chromaticLift" in2="SourceGraphic" operator="over" />
        </filter>
        <filter
          id="megle-liquid-glass-edge"
          colorInterpolationFilters="sRGB"
          x="-30%"
          y="-30%"
          width="160%"
          height="160%"
        >
          <feTurbulence
            type="turbulence"
            baseFrequency="0.028 0.04"
            numOctaves="1"
            seed="29"
            result="edgeNoise"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="edgeNoise"
            scale="8"
            xChannelSelector="R"
            yChannelSelector="B"
          />
        </filter>
      </defs>
    </svg>
  );
}

function buildPointerEventProps<TElement extends HTMLElement = HTMLElement>({
  interactive,
  pressable,
  onPointerDown,
  onPointerLeave,
  onPointerMove,
  onPointerUp
}: {
  interactive: boolean;
  pressable: boolean;
  onPointerDown?: (event: ReactPointerEvent<TElement>) => void;
  onPointerLeave?: (event: ReactPointerEvent<TElement>) => void;
  onPointerMove?: (event: ReactPointerEvent<TElement>) => void;
  onPointerUp?: (event: ReactPointerEvent<TElement>) => void;
}) {
  return {
    onPointerDown(event: ReactPointerEvent<TElement>) {
      if (interactive) {
        if (shouldPressSurface(event, pressable)) {
          event.currentTarget.setAttribute("data-glass-pressed", "true");
          event.currentTarget.style.setProperty("--glass-pressure", "1");
        }
      }
      onPointerDown?.(event);
    },
    onPointerLeave(event: ReactPointerEvent<TElement>) {
      onPointerLeave?.(event);
    },
    onPointerMove(event: ReactPointerEvent<TElement>) {
      onPointerMove?.(event);
    },
    onPointerUp(event: ReactPointerEvent<TElement>) {
      if (interactive) {
        event.currentTarget.removeAttribute("data-glass-pressed");
        event.currentTarget.style.setProperty("--glass-pressure", "0");
      }
      onPointerUp?.(event);
    }
  };
}

function shouldPressSurface<TElement extends HTMLElement>(
  event: ReactPointerEvent<TElement>,
  pressable: boolean
) {
  const { target, currentTarget } = event;
  return pressable && (target === currentTarget || currentTarget.contains(target as Node));
}

function nearestPointOnGlassEdge(clientX: number, clientY: number, rect: DOMRect) {
  const x = clamp(clientX, rect.left, rect.right);
  const y = clamp(clientY, rect.top, rect.bottom);
  const inside =
    clientX >= rect.left && clientX <= rect.right && clientY >= rect.top && clientY <= rect.bottom;

  if (!inside) {
    return { x, y };
  }

  const candidates = [
    { x, y: rect.top, distance: clientY - rect.top },
    { x: rect.right, y, distance: rect.right - clientX },
    { x, y: rect.bottom, distance: rect.bottom - clientY },
    { x: rect.left, y, distance: clientX - rect.left }
  ];
  return candidates.reduce((nearest, candidate) =>
    candidate.distance < nearest.distance ? candidate : nearest
  );
}

function distanceToGlassEdge(clientX: number, clientY: number, rect: DOMRect) {
  const edgePoint = nearestPointOnGlassEdge(clientX, clientY, rect);
  return Math.hypot(clientX - edgePoint.x, clientY - edgePoint.y);
}


function updateInteractiveAffordancePointers(clientX: number, clientY: number) {
  const targets = interactiveAffordanceTargets();
  const maxDistance = pointerEdgeProximity();
  for (const target of targets) {
    if (target.closest('[data-titlebar-search="true"]')) {
      target.removeAttribute("data-interactive-pointer-target");
      target.removeAttribute("data-interactive-pointer");
      target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
      continue;
    }
    target.dataset.interactivePointerTarget = "true";
    if (interactiveAffordanceDisabled(target)) {
      hideInteractiveAffordancePointer(target);
      continue;
    }
    const rect = cachedBoundingRect(target);
    if (rect.width <= 0 || rect.height <= 0) {
      hideInteractiveAffordancePointer(target);
      continue;
    }
    const distance = distanceToGlassEdge(clientX, clientY, rect);
    if (distance > maxDistance) {
      target.dataset.interactivePointer = "idle";
      target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
      continue;
    }
    const pointer = interactiveAffordancePointerPercent(target, clientX, clientY, rect);
    const opacity = Math.pow(1 - distance / maxDistance, 1.55);
    target.dataset.interactivePointer = "active";
    target.style.setProperty("--interactive-pointer-x", `${pointer.x}%`);
    target.style.setProperty("--interactive-pointer-y", `${pointer.y}%`);
    target.style.setProperty("--interactive-pointer-opacity", opacity.toFixed(3));
  }
}

function interactiveAffordanceTargets() {
  const candidates = document.querySelectorAll<HTMLElement>(INTERACTIVE_AFFORDANCE_SELECTOR);
  const targets = new Set<HTMLElement>();
  for (const candidate of candidates) {
    const owner = candidate.closest<HTMLElement>(INTERACTIVE_AFFORDANCE_OWNER_SELECTOR);
    let target = candidate;
    if (owner && owner !== candidate) {
      clearInteractiveAffordancePointer(candidate);
      target = owner;
    }
    const visualTarget = interactiveAffordanceVisualTarget(target);
    if (visualTarget !== target) {
      clearInteractiveAffordancePointer(target);
    }
    targets.add(visualTarget);
  }
  return targets;
}

function interactiveAffordanceVisualTarget(target: HTMLElement) {
  const selector = target.getAttribute(INTERACTIVE_POINTER_TARGET_SELECTOR_ATTRIBUTE);
  if (!selector) {
    return target;
  }
  try {
    return target.querySelector<HTMLElement>(selector) ?? target;
  } catch {
    return target;
  }
}

function interactiveAffordanceDisabled(target: HTMLElement) {
  if (target.matches(":disabled,[aria-disabled='true']")) {
    return true;
  }
  return target.matches(".settings-style-slider-control") && Boolean(target.querySelector("input:disabled"));
}

function interactiveAffordancePointerPercent(target: HTMLElement, clientX: number, clientY: number, rect: DOMRect) {
  const pointer = pointerPercentForRect(clientX, clientY, rect);
  if (target.matches(".settings-style-slider-control")) {
    return { ...pointer, y: 50 };
  }
  return pointer;
}

function hideInteractiveAffordancePointers() {
  const targets = document.querySelectorAll<HTMLElement>("[data-interactive-pointer-target='true']");
  for (const target of targets) {
    hideInteractiveAffordancePointer(target);
  }
}

function hideInteractiveAffordancePointer(target: HTMLElement) {
  target.dataset.interactivePointer = "idle";
  target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
}

function clearInteractiveAffordancePointer(target: HTMLElement) {
  target.removeAttribute("data-interactive-pointer-target");
  target.removeAttribute("data-interactive-pointer");
  target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
}

function pointerEdgeProximity() {
  // Read the sensing distance from the inline CSS var that interfaceStyle writes
  // onto <html>. Reading an inline style property is cheap (no getComputedStyle,
  // no reflow); we call this once per pointer frame, not per target. Falls back to
  // the base radius when the var is unset (e.g. before styles apply).
  const raw = document.documentElement.style.getPropertyValue("--glass-pointer-sensing-distance");
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : GLASS_POINTER_EDGE_PROXIMITY_PX;
}

function pointerEventInsideViewport(event: Event) {
  if (!("clientX" in event) || !("clientY" in event)) {
    return false;
  }
  const clientX = Number(event.clientX);
  const clientY = Number(event.clientY);
  return (
    Number.isFinite(clientX) &&
    Number.isFinite(clientY) &&
    clientX >= 0 &&
    clientX <= window.innerWidth &&
    clientY >= 0 &&
    clientY <= window.innerHeight
  );
}

function pointerPercentForRect(clientX: number, clientY: number, rect: DOMRect) {
  const x = ((clamp(clientX, rect.left, rect.right) - rect.left) / rect.width) * 100;
  const y = ((clamp(clientY, rect.top, rect.bottom) - rect.top) / rect.height) * 100;
  return {
    x: clampPercent(x),
    y: clampPercent(y)
  };
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function clampPercent(value: number) {
  return Math.min(100, Math.max(0, value));
}

function liquidGlassClassName(
  {
    active,
    className,
    scrollable,
    tone,
    variant
  }: {
    active: boolean;
    className?: string;
    scrollable: boolean;
    tone: "chrome" | "panel" | "elevated" | "control" | "primary" | "danger";
    variant: "regular" | "clear";
  },
  extra?: string
) {
  return [
    "liquid-glass",
    `liquid-glass-${variant}`,
    `liquid-glass-tone-${tone}`,
    scrollable ? "liquid-glass-scrollable" : "",
    active ? "liquid-glass-active" : "",
    extra ?? "",
    className ?? ""
  ]
    .filter(Boolean)
    .join(" ");
}

function withInitialPointer(style: CSSProperties | undefined): CSSProperties {
  return {
    "--glass-pointer-x": POINTER_RESET,
    "--glass-pointer-y": POINTER_RESET,
    "--glass-pointer-opacity": 0,
    "--glass-pressure": 0,
    ...style
  } as CSSProperties;
}

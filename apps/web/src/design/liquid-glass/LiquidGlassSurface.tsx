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
    // The last cursor position we saw, retained across frames. Layout changes
    // (scroll / resize / folder expand) move the affordances under a stationary
    // cursor, so we re-run the driver at this position after invalidating the rect
    // cache — otherwise the glow stays anchored to where the rows used to be.
    let lastCursor: { x: number; y: number } | null = null;

    function clearPointerFrame() {
      latestPointer = null;
      if (animationFrame) {
        window.cancelAnimationFrame(animationFrame);
        animationFrame = 0;
      }
    }

    function requestPointerUpdate(x: number, y: number) {
      latestPointer = { x, y };
      lastCursor = { x, y };
      if (animationFrame) return;
        animationFrame = window.requestAnimationFrame(() => {
          animationFrame = 0;
          if (latestPointer) {
            updateInteractiveAffordancePointers(latestPointer.x, latestPointer.y);
          }
        });
    }

    // A layout shift moved the affordances under the cursor; re-anchor the glow to
    // the same cursor against the fresh geometry. We wait TWO animation frames
    // before re-measuring: the first frame lets the browser finish the reflow the
    // shift triggered (including sticky tree-header repositioning, which resolves a
    // frame after the row insert), so getBoundingClientRect returns settled values
    // instead of stale-by-a-row coordinates.
    let layoutSettleFrame = 0;
    function handleLayoutShift() {
      invalidateAffordanceRectCache();
      if (!lastCursor) return;
      if (layoutSettleFrame) return;
      layoutSettleFrame = window.requestAnimationFrame(() => {
        layoutSettleFrame = window.requestAnimationFrame(() => {
          layoutSettleFrame = 0;
          invalidateAffordanceRectCache();
          if (lastCursor) {
            updateInteractiveAffordancePointers(lastCursor.x, lastCursor.y);
          }
        });
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
      lastCursor = null;
      clearPointerFrame();
      hideInteractiveAffordancePointers();
    }

    window.addEventListener("pointermove", handlePointerMove, { passive: true });
    window.addEventListener("mousemove", handleMouseMove, { passive: true });
    window.addEventListener("pointerleave", handlePointerExit);
    window.addEventListener("blur", handlePointerExit);
    // Affordance target rects are cached across pointer-move frames; their POSITIONS
    // change on scroll (capture phase catches inner scrollers like the media grid
    // and the sidebar tree), on resize, and on any LAYOUT change that shifts rows —
    // expanding/collapsing a folder inserts/removes rows and moves everything below
    // it WITHOUT firing scroll or resizing <body> (the tree scrolls inside its own
    // overflow container, so body height is unchanged). A MutationObserver on the
    // subtree catches those row insert/removes; a ResizeObserver catches container
    // reflows. Both route through handleLayoutShift, which re-anchors the glow to
    // the stationary cursor against fresh geometry. Invalidating is O(1).
    window.addEventListener("scroll", handleLayoutShift, { capture: true, passive: true });
    window.addEventListener("resize", handleLayoutShift, { passive: true });
    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(handleLayoutShift);
    resizeObserver?.observe(document.body);
    const mutationObserver =
      typeof MutationObserver === "undefined" ? null : new MutationObserver(handleLayoutShift);
    mutationObserver?.observe(document.body, { childList: true, subtree: true });

    return () => {
      clearPointerFrame();
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("pointerleave", handlePointerExit);
      window.removeEventListener("blur", handlePointerExit);
      window.removeEventListener("scroll", handleLayoutShift, { capture: true });
      window.removeEventListener("resize", handleLayoutShift);
      resizeObserver?.disconnect();
      mutationObserver?.disconnect();
      if (layoutSettleFrame) window.cancelAnimationFrame(layoutSettleFrame);
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
        style: withInitialPressure(style)
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
        style={withInitialPressure(style)}
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


// Targets lit (opacity > 0) on the previous pointer frame. We only write style to
// targets that ENTER, STAY IN, or LEAVE the ignite radius — the hundreds of
// off-screen affordances (virtualized tree rows, tiles below the fold) are never
// touched, so a pointer-move frame costs O(near-cursor targets), not O(all
// targets). data-interactive-pointer-target is a static marker set once; the live
// glow is driven purely by the --interactive-pointer-* vars (no per-frame
// attribute writes — nothing reads data-interactive-pointer).
let litAffordanceTargets = new Set<HTMLElement>();

function updateInteractiveAffordancePointers(clientX: number, clientY: number) {
  const targets = interactiveAffordanceTargets();
  const maxDistance = pointerEdgeProximity();
  const nextLit = new Set<HTMLElement>();
  for (const target of targets) {
    if (!target.dataset.interactivePointerTarget) {
      target.dataset.interactivePointerTarget = "true";
    }
    const rect = cachedBoundingRect(target);
    if (rect.width <= 0 || rect.height <= 0) {
      continue;
    }
    const distance = distanceToGlassEdge(clientX, clientY, rect);
    if (distance > maxDistance || interactiveAffordanceDisabled(target)) {
      continue;
    }
    // Single screen-space circle model: every target renders the SAME circle
    // (centred on the real cursor, radius = sensing distance in absolute px). We
    // write the cursor position in this element's LOCAL pixel space — it can fall
    // outside the element (negative / larger than the box), which is what lets the
    // one circle span neighbouring rows/buttons instead of each element lighting
    // up on its own. The radial gradient in CSS does ALL the falloff, so the
    // element-level opacity is just an on/off gate (no per-element dimming that
    // would flatten the circle into a vertical band).
    const localX = clientX - rect.left;
    // Sliders keep the glow vertically centred on the track (the cursor rides the
    // thumb but the highlight should hug the pill), everything else uses the real
    // cursor Y so the single circle stays continuous across the element.
    const localY = target.matches(".settings-style-slider-control")
      ? rect.height / 2
      : clientY - rect.top;
    target.style.setProperty("--interactive-pointer-x", `${localX.toFixed(1)}px`);
    target.style.setProperty("--interactive-pointer-y", `${localY.toFixed(1)}px`);
    target.style.setProperty("--interactive-pointer-opacity", "1");
    nextLit.add(target);
  }
  // Extinguish targets that were lit last frame but are no longer in range.
  for (const target of litAffordanceTargets) {
    if (!nextLit.has(target)) {
      target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
    }
  }
  litAffordanceTargets = nextLit;
}

function interactiveAffordanceTargets() {
  const candidates = document.querySelectorAll<HTMLElement>(INTERACTIVE_AFFORDANCE_SELECTOR);
  const targets = new Set<HTMLElement>();
  for (const candidate of candidates) {
    // Titlebar search opts out of the pointer glow entirely (it has its own
    // self-contained focus treatment); never route it or its inner controls here.
    if (candidate.closest('[data-titlebar-search="true"]')) {
      clearInteractiveAffordancePointer(candidate);
      continue;
    }
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

function hideInteractiveAffordancePointers() {
  for (const target of litAffordanceTargets) {
    target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
  }
  litAffordanceTargets = new Set();
}

function clearInteractiveAffordancePointer(target: HTMLElement) {
  // Steady-state no-op: only touch the DOM the first time we demote an element
  // (e.g. an inner tree button that redirects to its row owner), so re-running
  // this every frame over hundreds of child candidates costs nothing.
  if (!target.hasAttribute("data-interactive-pointer-target")) {
    return;
  }
  target.removeAttribute("data-interactive-pointer-target");
  target.style.setProperty("--interactive-pointer-opacity", POINTER_OPACITY_HIDDEN);
  litAffordanceTargets.delete(target);
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

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
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

// Seed the press-feedback var so the first pointerdown has a value to animate
// from. (The old pointer-glow surface plane — --glass-pointer-x/y/opacity — was
// removed; the hover glow now lives entirely on the --interactive-pointer-* vars
// driven by the global affordance loop.)
function withInitialPressure(style: CSSProperties | undefined): CSSProperties {
  return {
    "--glass-pressure": 0,
    ...style
  } as CSSProperties;
}

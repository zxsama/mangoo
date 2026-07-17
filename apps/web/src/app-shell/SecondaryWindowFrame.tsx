import { X } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  type PointerEvent as ReactPointerEvent,
  type ReactNode
} from "react";
import {
  getDesktopBridge,
  notifySecondaryWindowReady,
  type DesktopSecondaryWindowRole
} from "../core/desktop";
import { LiquidGlassLayer, LiquidGlassSurface } from "../design/liquid-glass";

export function SecondaryWindowFrame({
  bodyClassName,
  children,
  onRequestClose,
  role,
  title,
  windowClassName
}: {
  bodyClassName?: string;
  children: ReactNode;
  onRequestClose: () => void | Promise<unknown>;
  role: DesktopSecondaryWindowRole;
  title: string;
  windowClassName?: string;
}) {
  const controls = getDesktopBridge()?.windowControls;
  const frameRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    function handleKey(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      void onRequestClose();
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onRequestClose]);

  useLayoutEffect(() => {
    let contentRevealFrame = 0;
    const unsubscribeMaterialVisible = getDesktopBridge()?.onSecondaryWindowMaterialVisible?.(
      (payload) => {
        const frame = frameRef.current;
        if (!frame || payload.role !== role) return;
        frame.setAttribute(
          "data-secondary-material-visible-at",
          String(payload.materialVisibleAt)
        );
        performance.mark(`megle-secondary-${role}-material-visible`);
      }
    );
    const firstFrame = window.requestAnimationFrame(() => {
      const frame = frameRef.current;
      if (!frame) return;
      frame.dataset.secondaryRendererReady = "true";
      document.documentElement.dataset.secondaryRendererReady = role;
      performance.mark(`megle-secondary-${role}-renderer-ready`);
      void notifySecondaryWindowReady(role).then((result) => {
        const currentFrame = frameRef.current;
        if (!result || !currentFrame) return;
        currentFrame.setAttribute(
          "data-secondary-material-visible-at",
          String(result.materialVisibleAt)
        );
        currentFrame.setAttribute(
          "data-secondary-material-composited-at",
          String(result.materialCompositedAt)
        );
        currentFrame.setAttribute(
          "data-secondary-reveal-fallback",
          String(result.fallback)
        );
        currentFrame.dataset.secondaryWindowVisible = "true";
        performance.mark(`megle-secondary-${role}-material-composited`);
        contentRevealFrame = window.requestAnimationFrame(() => {
          contentRevealFrame = window.requestAnimationFrame(() => {
            const revealFrame = frameRef.current;
            if (!revealFrame) return;
            revealFrame.setAttribute("data-secondary-content-visible", "true");
            revealFrame.setAttribute("data-secondary-content-visible-at", String(Date.now()));
            performance.mark(`megle-secondary-${role}-content-visible`);
          });
        });
      });
    });
    return () => {
      unsubscribeMaterialVisible?.();
      window.cancelAnimationFrame(firstFrame);
      if (contentRevealFrame) window.cancelAnimationFrame(contentRevealFrame);
    };
  }, [role]);

  function handlePointerDown(event: ReactPointerEvent<HTMLElement>) {
    if ((event.target as HTMLElement).closest("button")) return;
    if (!controls) return;
    void controls.beginDrag({
      clientX: event.clientX,
      screenX: event.screenX,
      screenY: event.screenY,
      titlebarOffsetY: event.clientY,
      viewportWidth: window.innerWidth
    });
  }

  return (
    <LiquidGlassLayer>
      <LiquidGlassSurface
        as="div"
        className={`secondary-window${windowClassName ? ` ${windowClassName}` : ""}`}
        data-secondary-content-visible="false"
        data-secondary-window-role={role}
        ref={frameRef}
        tone="chrome"
      >
        <header
          className="secondary-window-titlebar"
          onPointerDown={handlePointerDown}
          onPointerMove={() => void controls?.moveDrag()}
          onPointerUp={() => void controls?.endDrag()}
        >
          <span className="secondary-window-title">{title}</span>
          <button
            aria-label="关闭"
            className="secondary-window-close no-drag"
            onClick={() => void onRequestClose()}
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </header>
        <div className={`secondary-window-body${bodyClassName ? ` ${bodyClassName}` : ""}`}>
          {children}
        </div>
      </LiquidGlassSurface>
    </LiquidGlassLayer>
  );
}

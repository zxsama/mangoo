import { X } from "lucide-react";
import { useEffect, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { getDesktopBridge } from "../core/desktop";
import { LiquidGlassLayer } from "../design/liquid-glass";

export function SecondaryWindowFrame({
  bodyClassName,
  children,
  onRequestClose,
  title,
  windowClassName
}: {
  bodyClassName?: string;
  children: ReactNode;
  onRequestClose: () => void | Promise<unknown>;
  title: string;
  windowClassName?: string;
}) {
  const controls = getDesktopBridge()?.windowControls;

  useEffect(() => {
    function handleKey(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      void onRequestClose();
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onRequestClose]);

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
      <div className={`secondary-window${windowClassName ? ` ${windowClassName}` : ""}`}>
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
      </div>
    </LiquidGlassLayer>
  );
}

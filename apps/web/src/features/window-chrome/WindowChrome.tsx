import { Maximize2, Minimize2, Minus, X } from "../../design/icons";
import { useEffect, useState } from "react";
import { getWindowControls } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";

/**
 * Frameless-window controls. Renders nothing when the desktop bridge does
 * not expose `windowControls`, so the same React tree continues to work in
 * the browser dev environment with no Electron host.
 */
export function WindowChrome() {
  const controls = getWindowControls();
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    if (!controls) return;
    let cancelled = false;

    function refresh() {
      if (!controls) return;
      void controls.isMaximized().then((value) => {
        if (!cancelled) setIsMaximized(value);
      });
    }

    refresh();
    window.addEventListener("resize", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("resize", refresh);
    };
  }, [controls]);

  if (!controls) {
    return null;
  }

  async function handleMaximize() {
    if (!controls) return;
    const next = await controls.maximize();
    setIsMaximized(next);
  }

  return (
    <div
      className="window-chrome no-drag"
      data-no-drag="true"
      role="group"
      aria-label="窗口控制"
    >
      <LiquidGlassButton
        aria-label="最小化"
        className="window-chrome-button"
        data-titlebar-control="window-minimize"
        onClick={() => void controls.minimize()}
        title="最小化"
        tone="control"
        type="button"
      >
        <Minus size={14} />
      </LiquidGlassButton>
      <LiquidGlassButton
        aria-label={isMaximized ? "还原" : "最大化"}
        className="window-chrome-button"
        data-titlebar-control="window-maximize"
        onClick={() => void handleMaximize()}
        title={isMaximized ? "还原" : "最大化"}
        tone="control"
        type="button"
      >
        {isMaximized ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
      </LiquidGlassButton>
      <LiquidGlassButton
        aria-label="关闭"
        className="window-chrome-button window-chrome-button-close"
        data-titlebar-control="window-close"
        onClick={() => void controls.close()}
        title="关闭"
        tone="danger"
        type="button"
      >
        <X size={14} />
      </LiquidGlassButton>
    </div>
  );
}

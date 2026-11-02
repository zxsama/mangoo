import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode
} from "react";
import { useDampedWheelScroll } from "./useDampedWheelScroll";
import { CHANNEL_MATRICES } from "../features/preview/previewRenderSettings";
import { LiquidGlassSurface } from "../design/liquid-glass";

const SHELL_LEFT_WIDTH_STORAGE_KEY = "mangoo.shell.left-width";
const SHELL_RIGHT_WIDTH_STORAGE_KEY = "mangoo.shell.right-width";
const SHELL_LEFT_DEFAULT_WIDTH = 292;
const SHELL_RIGHT_DEFAULT_WIDTH = 270;
const SHELL_LEFT_MIN_WIDTH = 220;
const SHELL_RIGHT_MIN_WIDTH = 220;
const SHELL_LEFT_MAX_WIDTH = 420;
const SHELL_RIGHT_MAX_WIDTH = 420;
const SHELL_CENTER_MIN_WIDTH = 480;
const SHELL_RESIZE_KEYBOARD_STEP = 12;
const SHELL_RESIZE_KEYBOARD_STEP_LARGE = 24;

type ShellResizeSide = "left" | "right";

interface AppShellProps {
  layout: "library" | "simple";
  sidebarsHidden?: boolean;
  titlebarLeft: ReactNode;
  titlebarCenter: ReactNode;
  titlebarRight: ReactNode;
  sidebar: ReactNode;
  centerPane: ReactNode;
  rightPane: ReactNode;
  overlays: ReactNode;
}

export function AppShell({
  layout,
  sidebarsHidden = false,
  titlebarLeft,
  titlebarCenter,
  titlebarRight,
  sidebar,
  centerPane,
  rightPane,
  overlays
}: AppShellProps) {
  const shellRef = useRef<HTMLElement | null>(null);
  useDampedWheelScroll(shellRef);
  const [leftWidth, setLeftWidth] = useState(() =>
    readStoredShellColumnWidth(SHELL_LEFT_WIDTH_STORAGE_KEY, SHELL_LEFT_DEFAULT_WIDTH)
  );
  const [rightWidth, setRightWidth] = useState(() =>
    readStoredShellColumnWidth(SHELL_RIGHT_WIDTH_STORAGE_KEY, SHELL_RIGHT_DEFAULT_WIDTH)
  );
  const [activeResizeSide, setActiveResizeSide] = useState<ShellResizeSide | null>(null);

  useEffect(() => {
    document.body.dataset.shellResizing = "false";
    return () => {
      document.body.dataset.shellResizing = "false";
      window.dispatchEvent(new Event("mangoo:shell-resize-state"));
    };
  }, []);

  const shellStyle = useMemo(
    () =>
      ({
        "--shell-left-width": `${leftWidth}px`,
        "--shell-right-width": `${rightWidth}px`
      }) as CSSProperties,
    [leftWidth, rightWidth]
  );

  const updateColumnWidth = useCallback(
    (side: ShellResizeSide, width: number) => {
      const constrainedWidth = constrainShellColumnWidth(
        side,
        width,
        shellRef.current?.getBoundingClientRect().width ?? null,
        side === "left" ? rightWidth : leftWidth
      );
      if (side === "left") {
        setLeftWidth(constrainedWidth);
        writeStoredShellColumnWidth(SHELL_LEFT_WIDTH_STORAGE_KEY, constrainedWidth);
      } else {
        setRightWidth(constrainedWidth);
        writeStoredShellColumnWidth(SHELL_RIGHT_WIDTH_STORAGE_KEY, constrainedWidth);
      }
    },
    [leftWidth, rightWidth]
  );

  const resizeColumnAtPointer = useCallback(
    (side: ShellResizeSide, event: PointerEvent<HTMLElement>) => {
      const rect = shellRef.current?.getBoundingClientRect();
      if (!rect) return;
      const nextWidth = side === "left" ? event.clientX - rect.left : rect.right - event.clientX;
      updateColumnWidth(side, nextWidth);
    },
    [updateColumnWidth]
  );

  const handleColumnResizePointerDown = useCallback(
    (side: ShellResizeSide, event: PointerEvent<HTMLElement>) => {
      event.currentTarget.focus({ preventScroll: true });
      event.preventDefault();
      event.stopPropagation();
      event.currentTarget.setPointerCapture(event.pointerId);
      document.body.dataset.shellResizing = "true";
      window.dispatchEvent(new Event("mangoo:shell-resize-state"));
      setActiveResizeSide(side);
      resizeColumnAtPointer(side, event);
    },
    [resizeColumnAtPointer]
  );

  const handleColumnResizePointerMove = useCallback(
    (side: ShellResizeSide, event: PointerEvent<HTMLElement>) => {
      if (activeResizeSide !== side || (event.buttons & 1) === 0) return;
      resizeColumnAtPointer(side, event);
    },
    [activeResizeSide, resizeColumnAtPointer]
  );

  const handleColumnResizePointerEnd = useCallback((event: PointerEvent<HTMLElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    // Pointer-down focuses the separator so keyboard resizing remains available
    // during the drag. Clear that pointer-acquired focus when the gesture ends;
    // otherwise :focus-visible can keep the divider highlighted after hover ends.
    event.currentTarget.blur();
    document.body.dataset.shellResizing = "false";
    window.dispatchEvent(new Event("mangoo:shell-resize-state"));
    setActiveResizeSide(null);
  }, []);

  const handleColumnResizeKeyDown = useCallback(
    (side: ShellResizeSide, event: KeyboardEvent<HTMLElement>) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
        return;
      }
      event.preventDefault();
      const step = event.shiftKey ? SHELL_RESIZE_KEYBOARD_STEP_LARGE : SHELL_RESIZE_KEYBOARD_STEP;
      const direction = event.key === "ArrowRight" ? 1 : -1;
      const delta = side === "left" ? direction * step : -direction * step;
      updateColumnWidth(side, (side === "left" ? leftWidth : rightWidth) + delta);
    },
    [leftWidth, rightWidth, updateColumnWidth]
  );

  return (
    <main
      ref={shellRef}
      className="app-shell"
      data-layout={layout}
      data-sidebars-hidden={sidebarsHidden ? "true" : "false"}
      style={shellStyle}
    >
      <div className="shell-backdrop-canvas" aria-hidden="true" />
      {/* Channel-isolation matrices, referenced by `filter: url(#...)` on
          `.preview-image`. Generated from CHANNEL_MATRICES so the ids can never
          drift from previewRenderSettings.

          `colorInterpolationFilters="sRGB"` is required: the SVG default is
          linearRGB, which would gamma-shift every channel view and misreport
          the stored values the feature exists to show. */}
      <svg aria-hidden="true" className="preview-channel-filters" focusable="false">
        <defs>
          {CHANNEL_MATRICES.map((matrix) => (
            <filter colorInterpolationFilters="sRGB" id={matrix.id} key={matrix.id}>
              <feColorMatrix type="matrix" values={matrix.values} />
            </filter>
          ))}
        </defs>
      </svg>
      {sidebarsHidden ? null : (
        <>
          <div
            aria-label="Resize left sidebar"
            aria-orientation="vertical"
            aria-valuemax={SHELL_LEFT_MAX_WIDTH}
            aria-valuemin={SHELL_LEFT_MIN_WIDTH}
            aria-valuenow={leftWidth}
            className={`shell-column-resizer shell-column-resizer-left${activeResizeSide === "left" ? " active" : ""}`}
            data-no-drag="true"
            onKeyDown={(event) => handleColumnResizeKeyDown("left", event)}
            onPointerCancel={handleColumnResizePointerEnd}
            onPointerDown={(event) => handleColumnResizePointerDown("left", event)}
            onPointerMove={(event) => handleColumnResizePointerMove("left", event)}
            onPointerUp={handleColumnResizePointerEnd}
            role="separator"
            tabIndex={0}
          />
          <div
            aria-label="Resize right sidebar"
            aria-orientation="vertical"
            aria-valuemax={SHELL_RIGHT_MAX_WIDTH}
            aria-valuemin={SHELL_RIGHT_MIN_WIDTH}
            aria-valuenow={rightWidth}
            className={`shell-column-resizer shell-column-resizer-right${activeResizeSide === "right" ? " active" : ""}`}
            data-no-drag="true"
            onKeyDown={(event) => handleColumnResizeKeyDown("right", event)}
            onPointerCancel={handleColumnResizePointerEnd}
            onPointerDown={(event) => handleColumnResizePointerDown("right", event)}
            onPointerMove={(event) => handleColumnResizePointerMove("right", event)}
            onPointerUp={handleColumnResizePointerEnd}
            role="separator"
            tabIndex={0}
          />
        </>
      )}
      <LiquidGlassSurface
        as="section"
        className="workbench-column workbench-column-left"
        aria-label="Primary navigation shell"
        backgroundGlow
        interactive
        tone="chrome"
      >
        <header
          className="shell-titlebar shell-titlebar-left"
          aria-label="Primary navigation"
        >
          {titlebarLeft}
        </header>
        <section className="workbench-column-body workbench-column-body-left">
          {sidebar}
        </section>
      </LiquidGlassSurface>
      <LiquidGlassSurface
        as="section"
        className="workbench-column workbench-column-center"
        aria-label="Center workbench shell"
        backgroundGlow
        interactive
        tone="chrome"
      >
        <header
          className="shell-titlebar shell-titlebar-center"
          aria-label="Workspace toolbar"
        >
          {titlebarCenter}
        </header>
        <section className="workbench-column-body workbench-column-body-center">
          {centerPane}
        </section>
      </LiquidGlassSurface>
      <LiquidGlassSurface
        as="section"
        className="workbench-column workbench-column-right"
        aria-label="Inspector shell"
        backgroundGlow
        interactive
        tone="chrome"
      >
        <header
          className="shell-titlebar shell-titlebar-right"
          aria-label="Window actions"
        >
          {titlebarRight}
        </header>
        <section className="workbench-column-body workbench-column-body-right">
          {rightPane}
        </section>
      </LiquidGlassSurface>
      {overlays}
    </main>
  );
}

function readStoredShellColumnWidth(key: string, fallback: number): number {
  try {
    const storedValue = window.localStorage.getItem(key);
    if (storedValue === null) {
      return fallback;
    }
    const value = Number(storedValue);
    return Number.isFinite(value) ? clamp(value, SHELL_LEFT_MIN_WIDTH, SHELL_LEFT_MAX_WIDTH) : fallback;
  } catch {
    return fallback;
  }
}

function writeStoredShellColumnWidth(key: string, value: number): void {
  try {
    window.localStorage.setItem(key, String(Math.round(value)));
  } catch {
    // Column resizing is still useful when storage is unavailable.
  }
}

function constrainShellColumnWidth(
  side: ShellResizeSide,
  width: number,
  shellWidth: number | null,
  oppositeWidth: number
): number {
  const min = side === "left" ? SHELL_LEFT_MIN_WIDTH : SHELL_RIGHT_MIN_WIDTH;
  const configuredMax = side === "left" ? SHELL_LEFT_MAX_WIDTH : SHELL_RIGHT_MAX_WIDTH;
  const centerAwareMax =
    shellWidth === null ? configuredMax : Math.max(min, shellWidth - oppositeWidth - SHELL_CENTER_MIN_WIDTH);
  return Math.round(clamp(width, min, Math.min(configuredMax, centerAwareMax)));
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

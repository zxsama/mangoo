import fs from "node:fs";
import path from "node:path";

export interface ViewerWindowBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Persisted Desktop-side viewer state (`userData/image-viewer.json`). */
export interface ImageViewerDesktopState {
  version: 1;
  associationWanted: boolean;
  /** Exactly what was written to the OS, so unregistering removes only ours. */
  registration: {
    executable: string;
    extensions: string[];
  } | null;
  window: {
    bounds: ViewerWindowBounds;
    maximized: boolean;
  } | null;
}

export const DEFAULT_VIEWER_SIZE = { width: 1120, height: 760 };

export function defaultDesktopState(): ImageViewerDesktopState {
  return { version: 1, associationWanted: false, registration: null, window: null };
}

function finite(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function parseBounds(value: unknown): ViewerWindowBounds | null {
  if (!value || typeof value !== "object") return null;
  const { x, y, width, height } = value as Record<string, unknown>;
  if (![x, y, width, height].every(finite)) return null;
  if ((width as number) < 320 || (height as number) < 240 || (width as number) > 20_000 || (height as number) > 20_000) return null;
  return { x: Math.round(x as number), y: Math.round(y as number), width: Math.round(width as number), height: Math.round(height as number) };
}

export function parseDesktopState(text: string): ImageViewerDesktopState {
  try {
    const raw = JSON.parse(text) as Record<string, unknown>;
    const state = defaultDesktopState();
    state.associationWanted = raw.associationWanted === true;
    const registration = raw.registration as Record<string, unknown> | null | undefined;
    if (
      registration && typeof registration.executable === "string" && Array.isArray(registration.extensions)
      && registration.extensions.every((ext) => typeof ext === "string" && /^\.[a-z0-9]{1,16}$/.test(ext))
    ) {
      state.registration = { executable: registration.executable, extensions: registration.extensions as string[] };
    }
    const window = raw.window as Record<string, unknown> | null | undefined;
    const bounds = parseBounds(window?.bounds);
    if (bounds) state.window = { bounds, maximized: window?.maximized === true };
    return state;
  } catch {
    return defaultDesktopState();
  }
}

export class DesktopStateStore {
  private state: ImageViewerDesktopState;
  private readonly file: string;

  constructor(file: string) {
    this.file = file;
    let text = "";
    try {
      text = fs.readFileSync(file, "utf8");
    } catch {
      // First run: defaults.
    }
    this.state = text ? parseDesktopState(text) : defaultDesktopState();
  }

  get(): ImageViewerDesktopState {
    return this.state;
  }

  update(change: (state: ImageViewerDesktopState) => void): void {
    const next: ImageViewerDesktopState = JSON.parse(JSON.stringify(this.state));
    change(next);
    this.state = next;
    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true });
      const temporary = `${this.file}.${process.pid}.tmp`;
      fs.writeFileSync(temporary, `${JSON.stringify(next, null, 2)}\n`);
      fs.renameSync(temporary, this.file);
    } catch {
      // Persistence is best effort; the in-memory state stays authoritative.
    }
  }
}

/**
 * Keep a restored window on a visible display: if less than 120x80 px of it
 * intersects any work area, centre it on the primary one instead.
 */
export function visibleBounds(
  bounds: ViewerWindowBounds | null,
  workAreas: readonly ViewerWindowBounds[],
  fallbackSize = DEFAULT_VIEWER_SIZE
): ViewerWindowBounds {
  const primary = workAreas[0] ?? { x: 0, y: 0, width: 1280, height: 800 };
  const centred = (size: { width: number; height: number }): ViewerWindowBounds => {
    const width = Math.min(size.width, primary.width);
    const height = Math.min(size.height, primary.height);
    return {
      x: Math.round(primary.x + (primary.width - width) / 2),
      y: Math.round(primary.y + (primary.height - height) / 2),
      width,
      height
    };
  };
  if (!bounds) return centred(fallbackSize);
  const visible = workAreas.some((area) => {
    const overlapX = Math.min(bounds.x + bounds.width, area.x + area.width) - Math.max(bounds.x, area.x);
    const overlapY = Math.min(bounds.y + bounds.height, area.y + area.height) - Math.max(bounds.y, area.y);
    return overlapX >= 120 && overlapY >= 80;
  });
  return visible ? bounds : centred(bounds);
}

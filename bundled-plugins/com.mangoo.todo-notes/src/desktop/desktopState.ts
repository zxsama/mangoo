import fs from "node:fs";
import path from "node:path";

import {
  DEFAULT_STICKY_APPEARANCE,
  DEFAULT_TODO_SETTINGS,
  mergeAppearance,
  mergeSettings,
  isRecordId,
  type StickyAppearance,
  type TodoDesktopSettings
} from "../shared/ipc.ts";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Device-local placement of one pinned note. */
export interface PinnedNote {
  /** Top-left of the collapsed note (the anchor it returns to). */
  x: number;
  y: number;
  width: number;
  /** Height when expanded (user resizable). */
  expandedHeight: number;
  appearance: StickyAppearance;
  pinnedAt: number;
}

/**
 * Persisted Desktop-side TODO state (`userData/todo-notes.json`). Everything
 * here is per device: screen positions, window state, local preferences and
 * reminder bookkeeping. Item content lives in the plugin's shared records.
 */
export interface TodoDesktopState {
  version: 1;
  settings: TodoDesktopSettings;
  window: { bounds: Rect; maximized: boolean; alwaysOnTop: boolean } | null;
  /** itemId -> pinned note. */
  pinned: Record<string, PinnedNote>;
  reminders: { lastCheck: number; fired: Record<string, number> };
  /** Whether this app set the OS login item (only ours is ever removed). */
  loginItemSet: boolean;
}

export const STICKY_WIDTH = { min: 220, default: 280, max: 640 };
export const STICKY_HEIGHT = { collapsedMin: 44, collapsedMax: 160, expandedMin: 160, expandedDefault: 320, expandedMax: 900 };
export const MAX_PINNED = 48;

export function defaultTodoState(): TodoDesktopState {
  return { version: 1, settings: { ...DEFAULT_TODO_SETTINGS }, window: null, pinned: {}, reminders: { lastCheck: Date.now(), fired: {} }, loginItemSet: false };
}

function finite(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function parseRect(value: unknown, min = { width: 320, height: 240 }): Rect | null {
  if (!value || typeof value !== "object") return null;
  const { x, y, width, height } = value as Record<string, unknown>;
  if (![x, y, width, height].every(finite)) return null;
  if ((width as number) < min.width || (height as number) < min.height || (width as number) > 20_000 || (height as number) > 20_000) return null;
  return { x: Math.round(x as number), y: Math.round(y as number), width: Math.round(width as number), height: Math.round(height as number) };
}

export function parsePinnedNote(value: unknown): PinnedNote | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  if (![raw.x, raw.y, raw.width, raw.expandedHeight].every(finite)) return null;
  return {
    x: Math.round(raw.x as number),
    y: Math.round(raw.y as number),
    width: clamp(Math.round(raw.width as number), STICKY_WIDTH.min, STICKY_WIDTH.max),
    expandedHeight: clamp(Math.round(raw.expandedHeight as number), STICKY_HEIGHT.expandedMin, STICKY_HEIGHT.expandedMax),
    appearance: mergeAppearance(DEFAULT_STICKY_APPEARANCE, raw.appearance),
    pinnedAt: finite(raw.pinnedAt) ? raw.pinnedAt : 0
  };
}

export function parseTodoState(text: string): TodoDesktopState {
  const state = defaultTodoState();
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(text) as Record<string, unknown>;
  } catch {
    return state;
  }
  if (!raw || typeof raw !== "object") return state;
  state.settings = mergeSettings(DEFAULT_TODO_SETTINGS, raw.settings);
  const window = raw.window as Record<string, unknown> | null | undefined;
  const bounds = parseRect(window?.bounds, { width: 280, height: 300 });
  if (bounds) state.window = { bounds, maximized: window?.maximized === true, alwaysOnTop: window?.alwaysOnTop === true };
  if (raw.pinned && typeof raw.pinned === "object") {
    for (const [itemId, note] of Object.entries(raw.pinned as Record<string, unknown>).slice(0, MAX_PINNED)) {
      const value = isRecordId(itemId) ? parsePinnedNote(note) : null;
      if (value) state.pinned[itemId] = value;
    }
  }
  const reminders = raw.reminders as Record<string, unknown> | null | undefined;
  if (reminders && typeof reminders === "object" && finite(reminders.lastCheck)) {
    const fired: Record<string, number> = {};
    if (reminders.fired && typeof reminders.fired === "object") {
      for (const [itemId, at] of Object.entries(reminders.fired as Record<string, unknown>)) {
        if (isRecordId(itemId) && finite(at)) fired[itemId] = at;
      }
    }
    state.reminders = { lastCheck: reminders.lastCheck, fired };
  }
  state.loginItemSet = raw.loginItemSet === true;
  return state;
}

export class TodoStateStore {
  private state: TodoDesktopState;
  private readonly file: string;

  constructor(file: string) {
    this.file = file;
    let text = "";
    try {
      text = fs.readFileSync(file, "utf8");
    } catch {
      // First run: defaults.
    }
    this.state = text ? parseTodoState(text) : defaultTodoState();
  }

  get(): TodoDesktopState {
    return this.state;
  }

  update(change: (state: TodoDesktopState) => void): void {
    const next: TodoDesktopState = JSON.parse(JSON.stringify(this.state));
    change(next);
    this.state = next;
    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true });
      const temporary = `${this.file}.${process.pid}.tmp`;
      fs.writeFileSync(temporary, `${JSON.stringify(next, null, 2)}\n`);
      fs.renameSync(temporary, this.file);
    } catch {
      // Best effort; the in-memory state stays authoritative.
    }
  }
}

/** The work area containing most of `rect` (else the first one). */
export function workAreaFor(rect: Rect, workAreas: readonly Rect[]): Rect {
  let best = workAreas[0] ?? { x: 0, y: 0, width: 1280, height: 800 };
  let bestArea = -1;
  for (const area of workAreas) {
    const overlapX = Math.min(rect.x + rect.width, area.x + area.width) - Math.max(rect.x, area.x);
    const overlapY = Math.min(rect.y + rect.height, area.y + area.height) - Math.max(rect.y, area.y);
    const overlap = Math.max(0, overlapX) * Math.max(0, overlapY);
    if (overlap > bestArea) {
      best = area;
      bestArea = overlap;
    }
  }
  return best;
}

/**
 * Window bounds of a pinned note at `height`. The note grows downward from its
 * anchor; when that would leave the work area it grows upward instead, and it
 * is always pulled fully on screen (e.g. after a monitor was unplugged).
 */
export function stickyBounds(note: Pick<PinnedNote, "x" | "y" | "width">, height: number, workAreas: readonly Rect[]): Rect {
  const area = workAreaFor({ x: note.x, y: note.y, width: note.width, height: Math.max(height, 40) }, workAreas);
  const width = clamp(note.width, STICKY_WIDTH.min, Math.min(STICKY_WIDTH.max, area.width));
  const h = clamp(Math.round(height), STICKY_HEIGHT.collapsedMin, area.height);
  const x = clamp(note.x, area.x, area.x + area.width - width);
  let y = clamp(note.y, area.y, area.y + area.height - Math.min(h, STICKY_HEIGHT.collapsedMax));
  if (y + h > area.y + area.height) y = Math.max(area.y, area.y + area.height - h);
  return { x, y, width, height: h };
}

/** A new note's anchor: cascaded from the top-right of the primary work area. */
export function nextStickyAnchor(existing: readonly Pick<PinnedNote, "x" | "y">[], workArea: Rect): { x: number; y: number } {
  const step = 28;
  const baseX = workArea.x + workArea.width - STICKY_WIDTH.default - 24;
  const baseY = workArea.y + 24;
  for (let index = 0; index < 40; index += 1) {
    const x = baseX - Math.floor(index / 12) * (STICKY_WIDTH.default + 16);
    const y = baseY + (index % 12) * step;
    if (!existing.some((note) => Math.abs(note.x - x) < 8 && Math.abs(note.y - y) < 8)) return { x, y };
  }
  return { x: baseX, y: baseY };
}

/**
 * Keep a restored window on a visible display: if less than 120x80 px of it
 * intersects any work area, centre it on the primary one instead.
 */
export function visibleWindowBounds(bounds: Rect | null, workAreas: readonly Rect[], fallback = { width: 980, height: 680 }): Rect {
  const primary = workAreas[0] ?? { x: 0, y: 0, width: 1280, height: 800 };
  const centred = (size: { width: number; height: number }): Rect => {
    const width = Math.min(size.width, primary.width);
    const height = Math.min(size.height, primary.height);
    return { x: Math.round(primary.x + (primary.width - width) / 2), y: Math.round(primary.y + (primary.height - height) / 2), width, height };
  };
  if (!bounds) return centred(fallback);
  const visible = workAreas.some((area) => {
    const overlapX = Math.min(bounds.x + bounds.width, area.x + area.width) - Math.max(bounds.x, area.x);
    const overlapY = Math.min(bounds.y + bounds.height, area.y + area.height) - Math.max(bounds.y, area.y);
    return overlapX >= 120 && overlapY >= 80;
  });
  return visible ? bounds : centred(bounds);
}

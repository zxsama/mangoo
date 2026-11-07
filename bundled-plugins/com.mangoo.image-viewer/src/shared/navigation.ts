/**
 * Pure navigation model for the viewer: ordering a folder like a file manager
 * does and moving through it. Kept free of React and DOM so it is unit tested.
 */

export type ImageSortKey = "name" | "modified" | "size" | "type";
export type ImageSortDirection = "asc" | "desc";

export interface ImageEntry {
  path: string;
  name: string;
  ext: string;
  sizeBytes: number;
  modifiedAtMs: number;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Natural, case-insensitive name order ("img2" < "img10"), like Explorer. */
export function compareNames(left: string, right: string): number {
  return collator.compare(left, right) || (left < right ? -1 : left > right ? 1 : 0);
}

export function sortEntries(
  entries: readonly ImageEntry[],
  key: ImageSortKey,
  direction: ImageSortDirection
): ImageEntry[] {
  const sign = direction === "asc" ? 1 : -1;
  const byName = (a: ImageEntry, b: ImageEntry) => compareNames(a.name, b.name);
  const compare = (a: ImageEntry, b: ImageEntry): number => {
    switch (key) {
      case "modified":
        return (a.modifiedAtMs - b.modifiedAtMs) * sign || byName(a, b);
      case "size":
        return (a.sizeBytes - b.sizeBytes) * sign || byName(a, b);
      case "type":
        return compareNames(a.ext, b.ext) * sign || byName(a, b);
      case "name":
      default:
        return byName(a, b) * sign;
    }
  };
  return [...entries].sort(compare);
}

/** Case-insensitive on Windows-style paths, exact elsewhere. */
export function samePath(left: string, right: string): boolean {
  if (left === right) return true;
  const windowsLike = /^[a-zA-Z]:[\\/]|^\\\\/.test(left) || /^[a-zA-Z]:[\\/]|^\\\\/.test(right);
  return windowsLike && left.replace(/\//g, "\\").toLowerCase() === right.replace(/\//g, "\\").toLowerCase();
}

export function indexOfPath(entries: readonly ImageEntry[], path: string): number {
  return entries.findIndex((entry) => samePath(entry.path, path));
}

/**
 * Next index when stepping by `delta`. With `loop`, the folder wraps like most
 * viewers; without it the ends clamp. Returns -1 for an empty list.
 */
export function stepIndex(length: number, current: number, delta: number, loop: boolean): number {
  if (length <= 0) return -1;
  const from = current < 0 ? 0 : current;
  const target = from + delta;
  if (loop) return ((target % length) + length) % length;
  return Math.min(length - 1, Math.max(0, target));
}

/**
 * After the current entry disappears (deleted / renamed away), stay at the
 * same position so the user lands on the next picture, as in Explorer viewers.
 */
export function indexAfterRemoval(length: number, removedIndex: number): number {
  if (length <= 0) return -1;
  return Math.min(removedIndex, length - 1);
}

/** Neighbours worth prefetching around `index`, nearest first. */
export function prefetchOrder(length: number, index: number, radius: number, loop: boolean): number[] {
  const result: number[] = [];
  for (let distance = 1; distance <= radius; distance += 1) {
    for (const delta of [distance, -distance]) {
      const candidate = loop
        ? (((index + delta) % length) + length) % length
        : index + delta;
      if (candidate >= 0 && candidate < length && candidate !== index && !result.includes(candidate)) {
        result.push(candidate);
      }
    }
  }
  return result;
}

/** A user-typed new file name: no separators, reserved names or traversal. */
export function validateFileName(name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "名称不能为空";
  if (trimmed.length > 255) return "名称过长";
  if (/[\\/:*?"<>|\u0000-\u001f]/.test(trimmed)) return "名称不能包含 \\ / : * ? \" < > | 等字符";
  if (trimmed === "." || trimmed === ".." || /[. ]$/.test(trimmed)) return "名称不能以点或空格结尾";
  if (/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?$/i.test(trimmed)) return "该名称为系统保留名称";
  return null;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? value.toFixed(0) : value >= 10 ? value.toFixed(1) : value.toFixed(2)} ${units[unit]}`;
}

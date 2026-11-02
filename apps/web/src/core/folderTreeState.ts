import type { DirectorySortState, FolderRecord } from "@mangoo/core-client";

export type FolderChildrenByParent = Record<number, FolderRecord[]>;

export function reconcileFolderExpansionForSelection(
  current: Set<number>,
  folderId: number,
  hasChildren: boolean | undefined
) {
  const shouldExpand = hasChildren !== false;
  if (current.has(folderId) === shouldExpand) {
    return current;
  }
  const next = new Set(current);
  if (shouldExpand) {
    next.add(folderId);
  } else {
    next.delete(folderId);
  }
  return next;
}

export function mergeFolderChildrenByParent(
  current: FolderChildrenByParent,
  parentId: number,
  incoming: FolderRecord[],
  order: DirectorySortState["effectiveOrder"] = "name_asc"
): FolderChildrenByParent {
  if (incoming.length === 0) {
    return current;
  }

  const existing = current[parentId] ?? [];
  // Core pages are already globally ordered. Merge with sparse ancestor rows
  // without reversing pages or locale-sorting the loaded directory tree.
  const incomingIds = new Set(incoming.map((folder) => folder.id));
  const retained = existing.filter((folder) => !incomingIds.has(folder.id));
  const merged: FolderRecord[] = [];
  let index = 0;
  for (const folder of incoming) {
    while (index < retained.length && compareFolderRecords(retained[index], folder, order) < 0) merged.push(retained[index++]);
    merged.push(folder);
  }
  merged.push(...retained.slice(index));
  if (merged.length === existing.length && merged.every((folder, i) => folder === existing[i])) return current;
  return { ...current, [parentId]: merged };
}

export function mergeFolderAncestorPath(
  current: FolderChildrenByParent,
  path: FolderRecord[],
  sortStates: ReadonlyMap<number, DirectorySortState> = new Map()
): FolderChildrenByParent {
  let next = current;
  for (let index = 1; index < path.length; index += 1) {
    const parentId = path[index - 1].id;
    next = mergeFolderChildrenByParent(next, parentId, [path[index]], sortStates.get(parentId)?.effectiveOrder);
  }
  return next;
}

export function expandFolderPath(current: Set<number>, path: FolderRecord[]): Set<number> {
  if (path.length < 2) {
    return current;
  }

  const next = new Set(current);
  for (let index = 0; index < path.length - 1; index += 1) {
    next.add(path[index].id);
  }
  if (next.size === current.size) {
    return current;
  }
  return next;
}

export function compareFolderRecords(left: FolderRecord, right: FolderRecord, order: DirectorySortState["effectiveOrder"] = "name_asc"): number {
  // SQLite BINARY compares UTF-8 bytes, equivalent to Unicode codepoint order.
  // JS string comparison compares UTF-16 units and differs for astral names.
  const a = Array.from(left.name), b = Array.from(right.name);
  let result = 0;
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    result = a[index].codePointAt(0)! - b[index].codePointAt(0)!;
    if (result) break;
  }
  result ||= a.length - b.length || left.id - right.id;
  return order === "name_desc" ? -result : result;
}

export function isFolderKnownChildless({
  hasChildren,
  childrenLoaded,
  loadedChildCount,
  loading,
  hasMore = false,
  loadFailed = false
}: {
  hasChildren?: boolean;
  childrenLoaded: boolean;
  loadedChildCount: number;
  loading: boolean;
  hasMore?: boolean;
  loadFailed?: boolean;
}) {
  // Cached metadata may predate a scan. Positive evidence and unfinished or
  // failed reads must keep the branch reachable, even after a formerly empty page.
  if (loadedChildCount > 0 || hasMore || loading || loadFailed) return false;
  return hasChildren === false || childrenLoaded;
}

export function folderShowsOpenIcon({
  expanded,
  knownChildless,
  selected
}: {
  expanded: boolean;
  knownChildless: boolean;
  selected: boolean;
}) {
  return knownChildless ? selected : expanded;
}

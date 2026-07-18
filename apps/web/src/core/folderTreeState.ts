import type { FolderRecord } from "./types";

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
  incoming: FolderRecord[]
): FolderChildrenByParent {
  if (incoming.length === 0) {
    return current;
  }

  const existing = current[parentId] ?? [];
  const byId = new Map(existing.map((folder) => [folder.id, folder]));
  let changed = false;
  for (const folder of incoming) {
    const previous = byId.get(folder.id);
    if (previous !== folder) {
      byId.set(folder.id, folder);
      changed = true;
    }
  }
  if (!changed) {
    return current;
  }

  const merged = [...byId.values()].sort(compareFolderRecords);
  return { ...current, [parentId]: merged };
}

export function mergeFolderAncestorPath(
  current: FolderChildrenByParent,
  path: FolderRecord[]
): FolderChildrenByParent {
  let next = current;
  for (let index = 1; index < path.length; index += 1) {
    next = mergeFolderChildrenByParent(next, path[index - 1].id, [path[index]]);
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

function compareFolderRecords(left: FolderRecord, right: FolderRecord): number {
  const nameOrder = left.name.localeCompare(right.name);
  return nameOrder !== 0 ? nameOrder : left.id - right.id;
}

export function isFolderKnownChildless({
  hasChildren,
  childrenLoaded,
  loadedChildCount,
  loading
}: {
  hasChildren: boolean;
  childrenLoaded: boolean;
  loadedChildCount: number;
  loading: boolean;
}) {
  return !hasChildren || (childrenLoaded && loadedChildCount === 0 && !loading);
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

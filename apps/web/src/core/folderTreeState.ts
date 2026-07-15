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

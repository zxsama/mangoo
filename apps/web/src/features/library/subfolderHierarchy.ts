import type { FolderRecord } from "@mangoo/core-client";

export type SubfolderSiblingPosition = "single" | "first" | "middle" | "last";

export interface VisibleSubfolderEntry {
  depth: number;
  folder: FolderRecord;
  inheritedGroupPosition: SubfolderSiblingPosition | null;
  parentId: number;
  siblingIndex: number;
  siblingCount: number;
  siblingPosition: SubfolderSiblingPosition;
}

export function buildVisibleSubfolderEntries({
  childFoldersByParentId,
  expandedFolderIds,
  parentFolderId,
  recursiveExpansionEnabled
}: {
  childFoldersByParentId: Record<number, FolderRecord[]>;
  expandedFolderIds: Set<number>;
  parentFolderId: number | null;
  recursiveExpansionEnabled: boolean;
}): VisibleSubfolderEntry[] {
  if (parentFolderId === null) {
    return [];
  }

  const entries: VisibleSubfolderEntry[] = [];
  const visited = new Set<number>();

  const appendChildren = (parentId: number, depth: number) => {
    const folders = childFoldersByParentId[parentId] ?? [];
    folders.forEach((folder, siblingIndex) => {
      if (visited.has(folder.id)) {
        return;
      }
      visited.add(folder.id);
      entries.push({
        depth,
        folder,
        inheritedGroupPosition: null,
        parentId,
        siblingCount: folders.length,
        siblingIndex,
        siblingPosition: resolveSiblingPosition(siblingIndex, folders.length)
      });
      if (recursiveExpansionEnabled && expandedFolderIds.has(folder.id)) {
        appendChildren(folder.id, depth + 1);
      }
    });
  };

  appendChildren(parentFolderId, 0);
  assignInheritedGroupPositions(entries);
  return entries;
}

function assignInheritedGroupPositions(entries: VisibleSubfolderEntry[]) {
  let runStart = -1;
  const flushRun = (exclusiveEnd: number) => {
    if (runStart < 0) {
      return;
    }
    const count = exclusiveEnd - runStart;
    for (let index = runStart; index < exclusiveEnd; index += 1) {
      entries[index].inheritedGroupPosition = resolveSiblingPosition(index - runStart, count);
    }
    runStart = -1;
  };

  entries.forEach((entry, index) => {
    if (entry.depth === 0) {
      flushRun(index);
      return;
    }
    if (runStart < 0) {
      runStart = index;
    }
  });
  flushRun(entries.length);
}

function resolveSiblingPosition(index: number, count: number): SubfolderSiblingPosition {
  if (count <= 1) {
    return "single";
  }
  if (index === 0) {
    return "first";
  }
  if (index === count - 1) {
    return "last";
  }
  return "middle";
}

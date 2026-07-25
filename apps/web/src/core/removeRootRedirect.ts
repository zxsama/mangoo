// Pure decision for what the center view should do when a root is removed from
// the library. Extracted from useLibraryData.removeRoot so it is unit-testable
// without a React/hook harness.

export interface RemoveRootRedirectInput {
  removedRootId: number;
  // The currently-open selection.
  selectedRootId: number | null;
  // rootId of the folder highlighted in the inspector (selectedFolderInfo), if any.
  inspectedFolderRootId: number | null;
  // Roots that still exist AFTER the removal (already filtered), with their root
  // folder id, in display order.
  survivingRoots: ReadonlyArray<{ id: number; rootFolderId: number | null }>;
}

export type RemoveRootRedirect =
  // The open view referenced the removed root: navigate to this surviving root's
  // root folder (folderId null = root level).
  | { kind: "redirect"; rootId: number; folderId: number | null }
  // The open view referenced the removed root but nothing survives: blank the view.
  | { kind: "blank" }
  // The open view did NOT reference the removed root: leave navigation as-is.
  | { kind: "keep" };

export function prepareRootRemoval<T extends { id: number; rootFolderId: number | null }>(
  roots: ReadonlyArray<T>,
  removedRootId: number
): {
  nextRoots: T[];
  survivingRoots: Array<{ id: number; rootFolderId: number | null }>;
} {
  const nextRoots = roots.filter((root) => root.id !== removedRootId);
  return {
    nextRoots,
    survivingRoots: nextRoots.map((root) => ({
      id: root.id,
      rootFolderId: root.rootFolderId
    }))
  };
}

// Whether the inspector's highlighted folder must be cleared (its root is gone),
// independent of the main redirect decision.
export function shouldClearInspectedFolder(input: RemoveRootRedirectInput): boolean {
  return input.inspectedFolderRootId === input.removedRootId;
}

export function decideRemoveRootRedirect(input: RemoveRootRedirectInput): RemoveRootRedirect {
  // Only the open view (selectedRootId) drives the center redirect. The
  // inspector clear is handled separately (shouldClearInspectedFolder).
  if (input.selectedRootId !== input.removedRootId) {
    return { kind: "keep" };
  }
  const target = input.survivingRoots[0];
  if (!target) {
    return { kind: "blank" };
  }
  return { kind: "redirect", rootId: target.id, folderId: target.rootFolderId ?? null };
}

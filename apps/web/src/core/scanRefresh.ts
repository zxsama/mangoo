// Decide how the library UI should refresh when background scan tasks change
// status. Extracted from `useLibraryData`'s task-transition effect so the
// decision is unit-testable.
//
// The key case this encodes: a `root_scan` that finishes for a root the user is
// NOT currently viewing. Its sidebar entry (media count + rootFolderId, hence
// whether the tree can expand) is still the pre-scan placeholder from when the
// root was freshly added and empty. The old logic only reacted to the SELECTED
// root's scan, so a background scan of another just-added folder left its count
// stuck at 0 and its tree unexpandable until a manual reload. Those roots need
// a lightweight `listRoots` refresh; only the selected root needs the heavier
// full library reload (which also re-drives the current media view).

export interface ScanRefreshTask {
  id: number;
  kind: string;
  rootId: number | null;
  status: string;
}

export interface ScanRefreshDecision {
  // Selected root's scan finished: reload the whole library view.
  reloadLibrary: boolean;
  // Some OTHER root's scan finished: refresh just the roots list so its sidebar
  // media count + rootFolderId (expandability) fill in.
  refreshRootsList: boolean;
}

export function decideRootScanRefresh(
  tasks: ScanRefreshTask[],
  previousStatuses: Map<number, string>,
  selectedRootId: number | null
): ScanRefreshDecision {
  let refreshRootsList = false;
  for (const task of tasks) {
    if (task.kind !== "root_scan") {
      continue;
    }
    const prior = previousStatuses.get(task.id);
    const transitionedToSucceeded =
      Boolean(prior) && prior !== task.status && task.status === "succeeded";
    if (!transitionedToSucceeded) {
      continue;
    }
    if (task.rootId === selectedRootId) {
      // Full reload subsumes a roots-list refresh, so we can stop here.
      return { reloadLibrary: true, refreshRootsList: false };
    }
    refreshRootsList = true;
  }
  return { reloadLibrary: false, refreshRootsList };
}

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  decideRootScanRefresh,
  type ScanRefreshTask
} from "../src/core/scanRefresh.ts";

function task(
  id: number,
  kind: string,
  rootId: number | null,
  status: string
): ScanRefreshTask {
  return { id, kind, rootId, status };
}

describe("decideRootScanRefresh", () => {
  it("refreshes the roots list when a NON-selected root scan finishes", () => {
    // The regression: adding new folders while viewing another root left the new
    // folders' sidebar counts stuck at 0 and their trees unexpandable, because
    // only the selected root's scan triggered a refresh.
    const previous = new Map([[10, "running"]]);
    const tasks = [task(10, "root_scan", 5, "succeeded")];

    const decision = decideRootScanRefresh(tasks, previous, /* selectedRootId */ 2);

    assert.equal(decision.reloadLibrary, false);
    assert.equal(decision.refreshRootsList, true);
  });

  it("reloads the whole library when the SELECTED root scan finishes", () => {
    const previous = new Map([[10, "running"]]);
    const tasks = [task(10, "root_scan", 2, "succeeded")];

    const decision = decideRootScanRefresh(tasks, previous, 2);

    assert.equal(decision.reloadLibrary, true);
    // A full reload subsumes the lightweight roots refresh.
    assert.equal(decision.refreshRootsList, false);
  });

  it("prefers a full reload when both the selected and another root finish", () => {
    const previous = new Map([
      [10, "running"],
      [11, "running"]
    ]);
    const tasks = [
      task(10, "root_scan", 5, "succeeded"),
      task(11, "root_scan", 2, "succeeded")
    ];

    const decision = decideRootScanRefresh(tasks, previous, 2);

    assert.equal(decision.reloadLibrary, true);
  });

  it("does nothing when a root scan has not changed status", () => {
    const previous = new Map([[10, "succeeded"]]);
    const tasks = [task(10, "root_scan", 5, "succeeded")];

    const decision = decideRootScanRefresh(tasks, previous, 2);

    assert.equal(decision.reloadLibrary, false);
    assert.equal(decision.refreshRootsList, false);
  });

  it("ignores a scan first observed already-succeeded (no prior status)", () => {
    // No prior status means the first poll; loadLibrary already fetched fresh
    // roots on mount, so re-refreshing on every initial task load is wasteful.
    const previous = new Map<number, string>();
    const tasks = [task(10, "root_scan", 5, "succeeded")];

    const decision = decideRootScanRefresh(tasks, previous, 2);

    assert.equal(decision.refreshRootsList, false);
  });

  it("ignores non-root_scan task transitions", () => {
    const previous = new Map([
      [10, "running"],
      [11, "running"]
    ]);
    const tasks = [
      task(10, "thumbnail", 5, "succeeded"),
      task(11, "interactive_folder_scan", 5, "succeeded")
    ];

    const decision = decideRootScanRefresh(tasks, previous, 2);

    assert.equal(decision.reloadLibrary, false);
    assert.equal(decision.refreshRootsList, false);
  });
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { CoreApiError } from "@megle/core-client";
import { canCancelRootOperation, rootOperationPhaseLabel, syncFolderCoreErrorCode, syncFolderIdentitySuffix, syncFolderStateLabel, visibleSyncFolderActions } from "../src/features/sync/syncFolderModel.ts";

const folder = (overrides = {}) => ({
  descriptor: { libraryUuid: "library-a", libraryRootUuid: "root-library", rootUuid: "root-00000001", rootFolderUuid: "folder-a", requestedName: "Artists", materializedName: "Artists", materializationFingerprint: "hash", logicalRevision: 1, lifecycle: "active", inventoryState: "unselected", inventoryRevision: 1, operationRevision: 2 },
  binding: null,
  allowedActions: ["select"],
  ...overrides
});

describe("synchronized folder presentation", () => {
  it("keeps same-name roots distinct and maps contract states", () => {
    const first = folder();
    const second = folder({ descriptor: { ...folder().descriptor, rootUuid: "root-00000002" } });
    assert.equal(syncFolderIdentitySuffix(first, [first, second]), "root-000");
    assert.equal(syncFolderStateLabel(first), "未在此设备同步");
  });

  it("never exposes direct select or rebind on a local-only fork", () => {
    const localOnly = folder({ binding: { localBindingUuid: "binding", localPath: "D:\\Media", placementParent: "D:\\", state: "local-only-fork", bindingGeneration: 1, desiredMaterializedName: "Artists", desiredMaterializationFingerprint: "hash", operationRevision: 2 }, allowedActions: ["select", "rebind-existing", "re-enable", "publish-as-new"] });
    assert.deepEqual(visibleSyncFolderActions(localOnly), ["re-enable", "publish-as-new"]);
  });

  it("uses Core phases and honors the cutover cancellation boundary", () => {
    assert.equal(rootOperationPhaseLabel("verifying"), "正在验证文件");
    assert.equal(canCancelRootOperation("transferring", false), true);
    assert.equal(canCancelRootOperation("catalog-cutover", false), false);
    assert.equal(canCancelRootOperation("transferring", true), false);
  });

  it("uses the shared client, overlay and stable root UUID keys", () => {
    const hook = readFileSync(new URL("../src/features/sync/useSyncFolders.ts", import.meta.url), "utf8");
    const panel = readFileSync(new URL("../src/features/sync/SyncFoldersPanel.tsx", import.meta.url), "utf8");
    assert.match(hook, /createCoreClient/);
    assert.doesNotMatch(hook, /fetch\(/);
    assert.match(hook, /if \(busyRootRef\.current\) return null/);
    assert.match(hook, /operationUuid: draft\.operationUuid/);
    assert.doesNotMatch(hook, /operationUuid: preflight\.preflightToken/);
    assert.match(panel, /ModalSurface/);
    assert.match(panel, /libraryUuid}:\$\{rootUuid/);
    assert.match(panel, /visibleSyncFolderActions/);
    assert.match(panel, /未在此设备同步/);
    assert.match(panel, /requestAnimationFrame\(\(\) => restoreFocusRef\.current\?\.focus\(\)\)/);
    assert.match(panel, /aria-live="polite"/);
    assert.match(panel, /加载更多比较项/);
    assert.match(panel, /preview\.nextCursor === null/);
    assert.doesNotMatch(panel, /polls >= 30/);
    assert.match(panel, /解决位置冲突/);
    assert.match(panel, /controller\.preflightRename/);
    assert.match(panel, /controller\.preflightRebind/);
    assert.match(panel, /folder\.activeOperation/);
    assert.match(panel, /operation\.bytesDone/);
    assert.match(panel, /operation\.transferMode/);
    assert.doesNotMatch(panel, /等待位置版本信息|后端提供 marker\/snapshot|后端公开当前操作标识/);
    assert.match(hook, /preflightSyncFolderRename/);
    assert.match(hook, /commitSyncFolderRename/);
    assert.match(hook, /preflightSyncFolderRebind/);
    assert.match(hook, /commitSyncFolderRebind/);
    assert.match(hook, /COMPARISON_POLL_ATTEMPTS = 8/);
    assert.match(hook, /page\.nextCursor/);
    assert.match(hook, /if \(busyRootRef\.current\) return null/);
  });

  it("recognizes stale comparison failures for plan recovery", () => {
    assert.equal(syncFolderCoreErrorCode(new CoreApiError(409, { code: "comparison-expired" })), "comparison-expired");
    assert.equal(syncFolderCoreErrorCode(new Error("offline")), null);
  });

  it("keeps long text and compact layouts resilient", () => {
    const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
    assert.match(styles, /\.settings-sync-folder-meta \{[^}]*overflow-wrap: anywhere/s);
    assert.match(styles, /\.sync-folder-conflict-list code \{[^}]*overflow-wrap: anywhere/s);
    assert.match(styles, /@media \(max-width: 719px\)[^]*\.sync-folder-conflict-list li \{ grid-template-columns: 1fr; \}/s);
    assert.match(styles, /\.managed-library-restore-preflight-volumes span \{[^}]*overflow-wrap: anywhere/s);
  });
});

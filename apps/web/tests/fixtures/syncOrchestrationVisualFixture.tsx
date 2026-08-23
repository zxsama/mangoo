import { StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import type { LibrarySummary, LibrarySyncStatus, SyncReconnectPlan } from "@megle/core-client";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import { OverlayProvider } from "../../src/design/overlays";
import { SyncManagementPanel } from "../../src/features/sync/SyncManagementPanel";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";
import "./syncOrchestrationVisualFixture.css";

type FixtureState = "lifecycle" | "restore" | "conflict" | "error" | "unbind" | "reconnect";
const state = (new URLSearchParams(location.search).get("state") ?? "lifecycle") as FixtureState;
const library: LibrarySummary = { active: false, availability: "available", bindingState: "synchronized", contextRevision: 3, libraryUuid: "018f0000-0000-7000-8000-000000000042", lifecycleState: "ready", managedRoot: "D:\\Media\\Artists", name: "Artists", rootUuid: "018f0000-0000-7000-8000-000000000043" };
const base: LibrarySyncStatus = { libraryUuid: library.libraryUuid, name: library.name, lifecycle: "syncing", phase: "active", catalogReadiness: "ready", providerId: "com.megle.onedrive", accountUuid: "account-fixture", remoteLibraryUuid: library.libraryUuid, allowedActions: ["pause","sync-now","unbind","resolve-conflicts"], lastSuccessAt: Date.now(), lastErrorCode: null, retryAfter: null, pollAfter: null, pendingUploadItems: 14, pendingDownloadItems: 3, pendingUploadBytes: 8388608, pendingDownloadBytes: 2097152, conflictCount: 0, localDivergenceCount: 0, operationRevision: 7, restoreUuid: null, restoreItemsDone: null, restoreItemsTotal: null, restoreBytesDone: null, restoreBytesTotal: null };
const variants: Record<FixtureState, LibrarySyncStatus> = {
  lifecycle: base,
  restore: { ...base, lifecycle: "restoring", phase: "restoring-media", catalogReadiness: "catalog-ready", allowedActions: ["pause"], restoreUuid: "restore-fixture", restoreItemsDone: 420, restoreItemsTotal: 1000, restoreBytesDone: 420000000, restoreBytesTotal: 1000000000 },
  conflict: { ...base, conflictCount: 3, allowedActions: ["pause","sync-now","unbind","resolve-conflicts"] },
  error: { ...base, lifecycle: "error", phase: "retry-wait", lastErrorCode: "provider_unavailable", retryAfter: Date.now() + 15000, allowedActions: ["retry","pause","unbind"] },
  unbind: { ...base, lifecycle: "syncing", allowedActions: ["unbind"] },
  reconnect: { ...base, lifecycle: "local-only", phase: "complete", allowedActions: ["enable","reconnect"], localDivergenceCount: 12 },
};
const reconnectPlan: SyncReconnectPlan = { planUuid: "018f0000-0000-7000-8000-000000000044", libraryUuid: library.libraryUuid, remoteLibraryUuid: library.libraryUuid, originIdentity: library.libraryUuid, localDivergenceCount: 12, remoteDivergenceCount: 4, remoteExists: true, remoteCompatible: true, sameRemoteIdentity: true, originRemoteWatermark: 81, baseRemoteObjectCount: 31, remoteObjectCount: 35, remoteDescriptorSha256: "aa", remoteBindingEpoch: "old-epoch", remoteAdvanced: true, mergeAllowed: true, publishNewAllowed: true };

function Fixture() {
  useEffect(() => {
    if (state === "unbind" || state === "reconnect") {
      const label = state === "unbind" ? "取消同步" : "重新连接";
      requestAnimationFrame(() => {
        const button = Array.from(document.querySelectorAll("button")).find((candidate) => candidate.textContent?.includes(label));
        button?.click();
        if (state === "reconnect") requestAnimationFrame(() => button?.click());
      });
    }
  }, []);
  return <main className="sync-orchestration-visual-fixture" data-sync-orchestration-fixture={state}>
    <SyncManagementPanel libraries={[library]} statuses={[variants[state]]} busyLibraryId={null} error={null} providers={[{ pluginId:"com.megle.onedrive",accountUuid:"account-fixture",accountDisplayName:"OneDrive · Design",runtimeState:"ready",authState:"authenticated",lastHealthyAt:Date.now(),lastErrorCode:null }]}
      onAction={() => undefined} onEnable={() => undefined} onUnbind={() => undefined}
      onReconnectPreview={async () => reconnectPlan} onReconnectCommit={() => undefined}
      onAccountChange={() => undefined} onRemoveLocalCopy={() => undefined} />
  </main>;
}

createRoot(document.getElementById("root")!).render(<StrictMode><OverlayProvider><LiquidGlassLayer><Fixture /></LiquidGlassLayer></OverlayProvider></StrictMode>);

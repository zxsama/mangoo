import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { actionLabel, syncStatusLabel, syncSummaryModel } from "../src/features/sync/syncModel.ts";

const status = (overrides = {}) => ({
  libraryUuid: "library-a",
  name: "Library A",
  lifecycle: "syncing",
  phase: "uploading",
  catalogReadiness: "catalog-ready",
  providerId: "provider",
  accountUuid: "account",
  remoteLibraryUuid: "remote",
  allowedActions: ["pause", "sync-now"],
  lastSuccessAt: null,
  lastErrorCode: null,
  retryAfter: null,
  pollAfter: null,
  pendingUploadItems: 2,
  pendingDownloadItems: 0,
  pendingUploadBytes: 12,
  pendingDownloadBytes: 0,
  conflictCount: 0,
  localDivergenceCount: 0,
  operationRevision: 1,
  ...overrides
});

describe("sync orchestration presentation", () => {
  it("derives aggregate attention without duplicating Core action rules", () => {
    const summary = syncSummaryModel({
      revision: 1,
      generatedAt: 1,
      schedulerState: "running",
      providers: [],
      libraries: [status(), status({ libraryUuid: "library-b", lifecycle: "error", lastErrorCode: "authorization_required" })]
    });
    assert.equal(summary.activeCount, 1);
    assert.equal(summary.attentionCount, 1);
    assert.match(summary.label, /需要处理/);
    assert.equal(syncStatusLabel(status({ conflictCount: 2 })), "2 个冲突");
    assert.equal(actionLabel("sync-now"), "立即同步");
  });

  it("uses the shared TanStack Query controller and foreground/network wake bridge", () => {
    const source = readFileSync(new URL("../src/features/sync/useSyncOrchestration.ts", import.meta.url), "utf8");
    const queryClient = readFileSync(new URL("../src/core/queryClient.ts", import.meta.url), "utf8");
    const popover = readFileSync(new URL("../src/features/notifications/NotificationPopover.tsx", import.meta.url), "utf8");
    const shell = readFileSync(new URL("../src/app-shell/ShellOverlayHost.tsx", import.meta.url), "utf8");
    assert.match(source, /useQuery\(/);
    assert.match(source, /refetchInterval:/);
    assert.match(queryClient, /focusManager\.subscribe/);
    assert.match(queryClient, /onlineManager\.subscribe/);
    assert.match(queryClient, /wakeSync/);
    assert.doesNotMatch(source, /libraries\.map\([^]*getSyncStatus/);
    assert.doesNotMatch(popover, /useSyncOrchestration/);
    assert.match(shell, /function SyncAwareNotificationPopover/);
    assert.doesNotMatch(shell, /syncSummaryLabel/);
  });

  it("uses shared overlays and pointer targets for restore and management", () => {
    const wizard = readFileSync(new URL("../src/features/library/ManagedLibraryWizard.tsx", import.meta.url), "utf8");
    const panel = readFileSync(new URL("../src/features/sync/SyncManagementPanel.tsx", import.meta.url), "utf8");
    assert.match(wizard, /ModalSurface/);
    assert.match(wizard, /role="tablist"/);
    assert.match(wizard, /aria-busy=\{remoteLoading\}/);
    assert.match(panel, /allowedActions\s*\.filter/);
    assert.match(panel, /data-interactive-pointer-target/);
    assert.match(panel, /role="alertdialog"/);
    assert.match(panel, /exportLibrarySyncDiagnostics/);
    assert.match(panel, /onReconnectPreview/);
    assert.match(panel, /三方预览/);
    assert.match(panel, /OneDriveAccountSetup/);
    assert.match(wizard, /authResumeRef/);
    assert.match(wizard, /contextRevision/);
    assert.match(wizard, /preflightSyncRestore/);
    assert.match(wizard, /确认并恢复/);
    assert.match(wizard, /getSyncRestore/);
    assert.match(wizard, /pauseSyncRestore/);
    assert.match(wizard, /resumeSyncRestore/);
    assert.match(wizard, /cancelSyncRestore/);
  });

  it("ships visual fixtures for every orchestration state", () => {
    const fixture = readFileSync(new URL("./fixtures/syncOrchestrationVisualFixture.tsx", import.meta.url), "utf8");
    for (const state of ["lifecycle", "restore", "conflict", "error", "unbind", "reconnect"]) assert.match(fixture, new RegExp(`${state}:`));
    assert.match(fixture, /data-sync-orchestration-fixture/);
    const harness = readFileSync(new URL("../../../.tmp/visual-check/desktop-ui-regression.mjs", import.meta.url), "utf8");
    assert.match(harness, /syncOrchestrationFixtureEvidence/);
    assert.match(harness, /ui-sync-orchestration-\$\{state\}\.png/);
  });
});

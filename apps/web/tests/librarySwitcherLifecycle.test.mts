import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

interface LibrarySwitcherLifecycleResult {
  ok: boolean;
  error?: string;
  productionWiring: {
    activeWhenOpen: boolean;
    controls: string | null;
    expandedWhenClosed: string | null;
    expandedWhenOpen: string | null;
    hasPopup: string | null;
    keyboardFocusedSearch: boolean;
    keyboardRestoredTrigger: boolean;
    mousePreservedTriggerFocus: boolean;
  };
  loadingSourceFocus: {
    focusedSearch: boolean;
    restoredSource: boolean;
    visible: boolean;
  };
  focusRestore: {
    cancelRestoredTrigger: boolean;
    escapeRestoredTrigger: boolean;
    nestedMenuStayedOpen: boolean;
    rootStayedOpen: boolean;
  };
  pendingRemove: {
    actionDisabled: boolean;
    backdropBlocked: boolean;
    busy: boolean;
    cancelBlocked: boolean;
    cancelDisabled: boolean;
    duplicateCalls: number;
    escapeBlocked: boolean;
    recovered: boolean;
    reports: number;
    unhandledRejections: number;
  };
  adapterRemoveFailure: {
    actionEnabled: boolean;
    busyCleared: boolean;
    contractResult: boolean | "rejected";
    deleteCalls: number;
    modalStayedOpen: boolean;
    reports: number;
  };
  adapterRemoveFailureWithRefreshFailure: LibrarySwitcherLifecycleResult["adapterRemoveFailure"];
  adapterRemoveSuccessWithRefreshFailure: LibrarySwitcherLifecycleResult["adapterRemoveFailure"];
  iconMenu: {
    checkedItems: number;
    focusedRadioItem: boolean;
    keyboardSelections: number;
    menuRoleCount: number;
    radioItems: number;
    rovingTabStops: number;
    surfaceRole: string | null;
    visualGridPresent: boolean;
  };
  requestOwnership: {
    freshResponseWon: boolean;
    staleMutationIgnored: boolean;
    staleResponseIgnored: boolean;
    unmountReports: number;
    unmountUnhandledRejections: number;
  };
  hardMediaCancellation: {
    activeFetchAbortedOnStartingCore: boolean;
    activeFetchAbortedOnUnmount: boolean;
    noDuplicateAfterRetiredSettlement: boolean;
    retiredFetchAbortedOnStartingCore: boolean;
    retiredFetchAbortedOnUnmount: boolean;
    staleResponseIgnored: boolean;
    unhandledRejections: number;
  };
  mutationFailures: {
    reports: number;
    unhandledRejections: number;
  };
}

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const electronRunner = fileURLToPath(
  new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url),
);
const resultPrefix = "OVERLAY_LIFECYCLE_RESULT:";

let server: ViteDevServer;
let result: LibrarySwitcherLifecycleResult;

async function runFixture(url: string): Promise<LibrarySwitcherLifecycleResult> {
  const electronModule = await import("electron");
  const electronPath = electronModule.default as unknown as string;
  const environment = { ...process.env };
  delete environment.ELECTRON_RUN_AS_NODE;

  return await new Promise<LibrarySwitcherLifecycleResult>((resolve, reject) => {
    const child = spawn(electronPath, [electronRunner, url], {
      cwd: webRoot,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill();
      reject(new Error(`library switcher lifecycle fixture timed out\n${stderr}`));
    }, 45_000);

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on("exit", (code) => {
      clearTimeout(timeout);
      const markerIndex = stdout.lastIndexOf(resultPrefix);
      if (markerIndex < 0) {
        reject(
          new Error(
            `library switcher lifecycle fixture exited ${code} without a result\n${stdout}\n${stderr}`,
          ),
        );
        return;
      }
      const payload = stdout.slice(markerIndex + resultPrefix.length).trim();
      resolve(JSON.parse(payload) as LibrarySwitcherLifecycleResult);
    });
  });
}

before(async () => {
  server = await createServer({
    root: webRoot,
    logLevel: "error",
    server: {
      host: "127.0.0.1",
      port: 0,
      strictPort: false,
    },
  });
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address === "object");
  result = await runFixture(
    `http://127.0.0.1:${address.port}/tests/fixtures/library-switcher-lifecycle.html`,
  );
  assert.equal(result.ok, true, result.error);
});

after(async () => {
  await server?.close();
});

describe("mounted LibrarySwitcher lifecycle", () => {
  it("uses production titlebar wiring for keyboard and pointer opening", () => {
    assert.deepEqual(result.productionWiring, {
      activeWhenOpen: true,
      controls: "library-root",
      expandedWhenClosed: "false",
      expandedWhenOpen: "true",
      hasPopup: "dialog",
      keyboardFocusedSearch: true,
      keyboardRestoredTrigger: true,
      mousePreservedTriggerFocus: true,
    });
  });

  it("restores point-anchored loading actions through their explicit source element", () => {
    assert.deepEqual(result.loadingSourceFocus, {
      focusedSearch: true,
      restoredSource: true,
      visible: true,
    });
  });

  it("restores focus to the remove trigger after cancel and Escape", () => {
    assert.deepEqual(result.focusRestore, {
      cancelRestoredTrigger: true,
      escapeRestoredTrigger: true,
      nestedMenuStayedOpen: true,
      rootStayedOpen: true,
    });
  });

  it("owns remove confirmation as one busy recoverable mutation", () => {
    assert.deepEqual(result.pendingRemove, {
      actionDisabled: true,
      backdropBlocked: true,
      busy: true,
      cancelBlocked: true,
      cancelDisabled: true,
      duplicateCalls: 1,
      escapeBlocked: true,
      recovered: true,
      reports: 1,
      unhandledRejections: 0,
    });
  });

  it("keeps the modal retryable when the real useLibraryData adapter reports Core failure", () => {
    assert.deepEqual(result.adapterRemoveFailure, {
      actionEnabled: true,
      busyCleared: true,
      contractResult: false,
      deleteCalls: 1,
      modalStayedOpen: true,
      reports: 1,
    });
  });

  it("keeps DELETE failure stable when reconciliation also fails", () => {
    assert.deepEqual(result.adapterRemoveFailureWithRefreshFailure, {
      actionEnabled: true,
      busyCleared: true,
      contractResult: false,
      deleteCalls: 1,
      modalStayedOpen: true,
      reports: 1,
    });
  });

  it("keeps DELETE success stable when reconciliation fails", () => {
    assert.deepEqual(result.adapterRemoveSuccessWithRefreshFailure, {
      actionEnabled: true,
      busyCleared: true,
      contractResult: true,
      deleteCalls: 1,
      modalStayedOpen: false,
      reports: 1,
    });
  });

  it("uses one shared roving radio menu for icon and color choices", () => {
    assert.equal(result.iconMenu.surfaceRole, "group");
    assert.equal(result.iconMenu.menuRoleCount, 1);
    assert.ok(result.iconMenu.radioItems > 2);
    assert.equal(result.iconMenu.checkedItems, 2);
    assert.equal(result.iconMenu.rovingTabStops, 1);
    assert.equal(result.iconMenu.focusedRadioItem, true);
    assert.equal(result.iconMenu.keyboardSelections, 1);
    assert.equal(result.iconMenu.visualGridPresent, true);
  });

  it("rejects stale reloads and drops work after unmount", () => {
    assert.deepEqual(result.requestOwnership, {
      freshResponseWon: true,
      staleMutationIgnored: true,
      staleResponseIgnored: true,
      unmountReports: 0,
      unmountUnhandledRejections: 0,
    });
  });

  it("preserves same-key media ownership through retired settlement and both hard boundaries", () => {
    assert.deepEqual(result.hardMediaCancellation, {
      activeFetchAbortedOnStartingCore: true,
      activeFetchAbortedOnUnmount: true,
      noDuplicateAfterRetiredSettlement: true,
      retiredFetchAbortedOnStartingCore: true,
      retiredFetchAbortedOnUnmount: true,
      staleResponseIgnored: true,
      unhandledRejections: 0,
    });
  });

  it("reports rejected switch, create, open, add, and icon mutations", () => {
    assert.deepEqual(result.mutationFailures, {
      reports: 5,
      unhandledRejections: 0,
    });
  });
});

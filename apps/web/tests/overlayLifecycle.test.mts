import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

interface OverlayLifecycleResult {
  ok: boolean;
  error?: string;
  basicFocusWorks: boolean;
  reentrancy: {
    closeDismissals: number;
    closeRootId: string | null;
    dismissTopDismissals: number;
    dismissTopRootId: string | null;
    switchDismissals: number;
    switchRootId: string | null;
  };
  autofocus: {
    modalFocused: boolean;
    modalDismissals: number;
    modalRootDismissals: number;
    modalRootId: string | null;
    nestedFocused: boolean;
    nestedDismissals: number;
    nestedRootDismissals: number;
    nestedRootId: string | null;
  };
  controlledFocus: {
    controlledFocusCount: number;
    controlledItemFocusedBefore: boolean;
    controlledRootIdBefore: string | null;
    providerFocusCount: number;
    providerDismissals: number;
    providerItemFocusedBefore: boolean;
    providerRootIdBefore: string | null;
  };
  controlledFamily: {
    activeRootAfterFirstClose: string | null;
    activeRootAfterReopen: string | null;
    activeRootAfterSecondClose: string | null;
    nestedDismissalsAfterFirstClose: number;
    nestedDismissalsAfterSecondClose: number;
    rootDismissals: number;
    rootFocusesAfterFirstClose: number;
    rootFocusesAfterSecondClose: number;
  };
  deferredAutofocus: {
    asyncItemFocused: boolean;
    initialItemFocused: boolean;
    reopenItemFocused: boolean;
    triggerFocusedDuringClose: boolean;
  };
  scrollShell: {
    anchorMeasuresAfterScroll: number;
    anchorMeasuresBeforeScroll: number;
    hasScrollViewport: boolean;
    shellOverflowY: string;
  };
  dynamicMenu: {
    activeId: string;
    ariaDisabledClicks: number;
    disabledTabIndex: number;
    enabledClicks: number;
    enabledTabStops: number;
    enterFallbackClicks: number;
    pointerDownPrevented: boolean;
    pointerFocusedDisabled: boolean;
  };
}

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const electronRunner = fileURLToPath(
  new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)
);
const resultPrefix = "OVERLAY_LIFECYCLE_RESULT:";

let server: ViteDevServer;
let result: OverlayLifecycleResult;

async function runFixture(url: string): Promise<OverlayLifecycleResult> {
  const electronModule = await import("electron");
  const electronPath = electronModule.default as unknown as string;
  const environment = { ...process.env };
  delete environment.ELECTRON_RUN_AS_NODE;

  return await new Promise<OverlayLifecycleResult>((resolve, reject) => {
    const child = spawn(electronPath, [electronRunner, url], {
      cwd: webRoot,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"]
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill();
      reject(new Error(`overlay lifecycle Electron runner timed out\n${stderr}`));
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
        reject(new Error(
          `overlay lifecycle Electron runner exited ${code} without a result\n${stdout}\n${stderr}`
        ));
        return;
      }
      const payload = stdout.slice(markerIndex + resultPrefix.length).trim();
      resolve(JSON.parse(payload) as OverlayLifecycleResult);
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
      strictPort: false
    }
  });
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address === "object");
  result = await runFixture(
    `http://127.0.0.1:${address.port}/tests/fixtures/overlayLifecycle.html`
  );
  assert.equal(result.ok, true, result.error);
});

after(async () => {
  await server?.close();
});

describe("mounted overlay lifecycle", () => {
  it("runs in a focus-capable renderer", () => {
    assert.equal(result.basicFocusWorks, true);
  });

  it("commits removal before invoking re-entrant dismiss callbacks", () => {
    assert.deepEqual(result.reentrancy, {
      closeDismissals: 1,
      closeRootId: null,
      dismissTopDismissals: 1,
      dismissTopRootId: "notifications",
      switchDismissals: 1,
      switchRootId: "notifications"
    });
  });

  it("registers nested menus and modals before their autofocus moves focus", () => {
    assert.deepEqual(result.autofocus, {
      modalFocused: true,
      modalDismissals: 1,
      modalRootDismissals: 0,
      modalRootId: "library",
      nestedFocused: true,
      nestedDismissals: 1,
      nestedRootDismissals: 0,
      nestedRootId: "library"
    });
  });

  it("restores controlled focus once without duplicating provider restoration", () => {
    assert.deepEqual(result.controlledFocus, {
      controlledFocusCount: 1,
      controlledItemFocusedBefore: true,
      controlledRootIdBefore: "library",
      providerFocusCount: 1,
      providerDismissals: 1,
      providerItemFocusedBefore: true,
      providerRootIdBefore: "library"
    });
  });

  it("atomically clears a controlled root family and supports rapid re-registration", () => {
    assert.deepEqual(result.controlledFamily, {
      activeRootAfterFirstClose: null,
      activeRootAfterReopen: "library",
      activeRootAfterSecondClose: null,
      nestedDismissalsAfterFirstClose: 1,
      nestedDismissalsAfterSecondClose: 2,
      rootDismissals: 0,
      rootFocusesAfterFirstClose: 0,
      rootFocusesAfterSecondClose: 1
    });
  });

  it("autofocuses deferred items and a menu reopened during presence exit", () => {
    assert.deepEqual(result.deferredAutofocus, {
      asyncItemFocused: true,
      initialItemFocused: true,
      reopenItemFocused: true,
      triggerFocusedDuringClose: true
    });
  });

  it("keeps material fixed while inner scrolling avoids anchor measurement", () => {
    assert.equal(result.scrollShell.hasScrollViewport, true);
    assert.equal(result.scrollShell.shellOverflowY, "");
    assert.equal(
      result.scrollShell.anchorMeasuresAfterScroll,
      result.scrollShell.anchorMeasuresBeforeScroll
    );
  });

  it("synchronizes dynamic roving focus and blocks aria-disabled activation", () => {
    assert.equal(result.dynamicMenu.disabledTabIndex, -1);
    assert.equal(result.dynamicMenu.enabledTabStops, 1);
    assert.equal(result.dynamicMenu.activeId, "dynamic-a");
    assert.equal(result.dynamicMenu.ariaDisabledClicks, 0);
    assert.equal(result.dynamicMenu.enabledClicks, 1);
    assert.equal(result.dynamicMenu.pointerDownPrevented, true);
    assert.equal(result.dynamicMenu.pointerFocusedDisabled, false);
    assert.equal(result.dynamicMenu.enterFallbackClicks, 0);
  });
});

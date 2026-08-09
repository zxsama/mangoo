import {
  act,
  useLayoutEffect,
  useState,
  type ReactElement,
} from "react";
import { createRoot, type Root } from "react-dom/client";
import type { MegleDesktopBridge } from "../../src/core/desktop";
import type { LibraryLoadPhase, LibrarySummary } from "../../src/core/libraryTypes";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import { ShellTitlebarPrimaryActions } from "../../src/app-shell/ShellTopBar";
import {
  OverlayProvider,
  useOverlayStack,
  type OverlayStackApi,
} from "../../src/design/overlays";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import { LibraryLoadingOverlay } from "../../src/features/library/LibraryLoadingOverlay";
import { LibrarySwitcher } from "../../src/features/library/LibrarySwitcher";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

type NativeInputPayload =
  | { keyCode: string; type: "key" }
  | { type: "mouse-click"; x: number; y: number };
type NativeInputRequest = NativeInputPayload & { id: number };

interface LibrarySwitcherLifecycleResult {
  ok: boolean;
  error?: string;
  productionWiring?: {
    activeWhenOpen: boolean;
    controls: string | null;
    expandedWhenClosed: string | null;
    expandedWhenOpen: string | null;
    hasPopup: string | null;
    keyboardFocusedSearch: boolean;
    keyboardRestoredTrigger: boolean;
    mousePreservedTriggerFocus: boolean;
  };
  loadingSourceFocus?: {
    focusedSearch: boolean;
    restoredSource: boolean;
    visible: boolean;
  };
  focusRestore?: {
    cancelRestoredTrigger: boolean;
    escapeRestoredTrigger: boolean;
    nestedMenuStayedOpen: boolean;
    rootStayedOpen: boolean;
  };
  pendingRemove?: {
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
  adapterRemoveFailure?: {
    actionEnabled: boolean;
    busyCleared: boolean;
    contractResult: boolean | "rejected";
    deleteCalls: number;
    modalStayedOpen: boolean;
    reports: number;
  };
  adapterRemoveFailureWithRefreshFailure?: LibrarySwitcherLifecycleResult["adapterRemoveFailure"];
  adapterRemoveSuccessWithRefreshFailure?: LibrarySwitcherLifecycleResult["adapterRemoveFailure"];
  iconMenu?: {
    checkedItems: number;
    focusedRadioItem: boolean;
    keyboardSelections: number;
    menuRoleCount: number;
    radioItems: number;
    rovingTabStops: number;
    surfaceRole: string | null;
    visualGridPresent: boolean;
  };
  requestOwnership?: {
    freshResponseWon: boolean;
    staleMutationIgnored: boolean;
    staleResponseIgnored: boolean;
    unmountReports: number;
    unmountUnhandledRejections: number;
  };
  hardMediaCancellation?: {
    activeFetchAbortedOnStartingCore: boolean;
    activeFetchAbortedOnUnmount: boolean;
    noDuplicateAfterRetiredSettlement: boolean;
    retiredFetchAbortedOnStartingCore: boolean;
    retiredFetchAbortedOnUnmount: boolean;
    staleResponseIgnored: boolean;
    unhandledRejections: number;
  };
  mutationFailures?: {
    reports: number;
    unhandledRejections: number;
  };
}

interface MountedScenario {
  container: HTMLDivElement;
  root: Root;
}

interface ScenarioControl {
  api: OverlayStackApi | null;
}

interface AdapterRemoveControl {
  removeResult: unknown;
  rootCount: number;
}

interface HardMediaCancellationControl {
  library: LibraryState | null;
}

interface Deferred<T> {
  promise: Promise<T>;
  reject: (cause: unknown) => void;
  resolve: (value: T) => void;
}

interface ProductionScenarioProps {
  control: ScenarioControl;
  library?: LibraryState;
  loading?: boolean;
}

const lifecycleWindow = window as Window & {
  __overlayLifecycleInputRequest?: NativeInputRequest;
  __overlayLifecycleInputResponseId?: number;
  __overlayLifecycleResult?: LibrarySwitcherLifecycleResult;
};

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const ACTIVE_LIBRARY: LibrarySummary = {
  active: true,
  available: true,
  dbPath: "C:\\Fixture\\library.sqlite",
  id: "fixture-library",
  name: "Alpha Library",
};

const INACTIVE_LIBRARY: LibrarySummary = {
  active: false,
  available: true,
  dbPath: "C:\\Fixture\\beta.sqlite",
  id: "beta-library",
  name: "Beta Library",
};

const delay = (milliseconds = 0) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function createLibraryState(overrides: Partial<LibraryState> = {}): LibraryState {
  return {
    roots: [{ displayName: "Pictures", id: 7 }],
    addRoot: async () => undefined,
    removeRoot: async () => true,
    ...overrides,
  } as unknown as LibraryState;
}

function installBridge(overrides: Partial<MegleDesktopBridge> = {}): void {
  window.megleDesktop = {
    listLibraries: async () => [ACTIVE_LIBRARY, INACTIVE_LIBRARY],
    ...overrides,
  } as MegleDesktopBridge;
}

async function settle(milliseconds = 0): Promise<void> {
  await act(async () => {
    await delay(milliseconds);
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await Promise.resolve();
  });
}

async function waitFor(condition: () => boolean, message: string): Promise<void> {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (condition()) return;
    await settle(10);
  }
  throw new Error(`Timed out waiting for ${message}`);
}

let nativeInputRequestId = 0;

async function requestNativeInput(request: NativeInputPayload): Promise<void> {
  const id = ++nativeInputRequestId;
  lifecycleWindow.__overlayLifecycleInputRequest = { ...request, id };
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (lifecycleWindow.__overlayLifecycleInputResponseId === id) return;
    await delay(10);
  }
  throw new Error(`Timed out waiting for native input response ${id}`);
}

const requestNativeKey = (keyCode: string) =>
  requestNativeInput({ keyCode, type: "key" });

const requestNativePoint = (x: number, y: number) =>
  requestNativeInput({ type: "mouse-click", x, y });

const requestNativeClick = (element: HTMLElement) => {
  const bounds = element.getBoundingClientRect();
  return requestNativePoint(
    Math.round(bounds.left + bounds.width / 2),
    Math.round(bounds.top + bounds.height / 2),
  );
};

async function mountScenario(element: ReactElement): Promise<MountedScenario> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(element);
  });
  await settle(20);
  return { container, root };
}

async function unmountScenario({ container, root }: MountedScenario): Promise<void> {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  document.querySelectorAll("[data-overlay-layer-kind]").forEach((node) => node.remove());
  await settle();
}

async function findOpenSurface(id: string): Promise<HTMLElement | null> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const surface = document.getElementById(id);
    if (
      surface?.dataset.overlayState === "open" &&
      surface.style.visibility === "visible"
    ) {
      return surface;
    }
    await settle(10);
  }
  return null;
}

async function findOpenLayer(id: string): Promise<HTMLElement | null> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const layer = document.getElementById(id);
    if (layer?.dataset.overlayState === "open") return layer;
    await settle(10);
  }
  return null;
}

async function waitForLayerRemoval(id: string): Promise<void> {
  await waitFor(() => !document.getElementById(id), `overlay layer #${id} to exit`);
}

async function findElement(selector: string, root: ParentNode = document): Promise<HTMLElement> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const element = root.querySelector(selector);
    if (element instanceof HTMLElement) return element;
    await settle(10);
  }
  throw new Error(`Timed out waiting for ${selector}`);
}

async function findButton(
  text: string,
  root: ParentNode = document,
): Promise<HTMLButtonElement> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const button = Array.from(root.querySelectorAll("button")).find(
      (candidate) => candidate.textContent?.trim() === text,
    );
    if (button instanceof HTMLButtonElement) return button;
    await settle(10);
  }
  throw new Error(`Timed out waiting for button ${text}`);
}

function StackCapture({ control }: { control: ScenarioControl }) {
  const api = useOverlayStack();
  useLayoutEffect(() => {
    control.api = api;
  }, [api, control]);
  return null;
}

function ProductionLibrarySwitcherScenario({
  control,
  library = createLibraryState(),
  loading = false,
}: ProductionScenarioProps) {
  const overlays = useOverlayStack();
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [openSource, setOpenSource] = useState<{
    autoFocus: boolean;
    element: HTMLButtonElement | null;
  }>({ autoFocus: false, element: null });
  const open = overlays.isRootOpen("library");
  const openSwitcher = (element?: HTMLButtonElement, autoFocus = false) => {
    setOpenSource({ autoFocus, element: element ?? null });
    overlays.openRoot("library");
  };

  return (
    <LiquidGlassLayer>
      <StackCapture control={control} />
      {loading ? (
        <LibraryLoadingOverlay
          onRetry={() => undefined}
          onSwitchLibrary={openSwitcher}
          phase={{ kind: "error", message: "Fixture load error" }}
        />
      ) : (
        <ShellTitlebarPrimaryActions
          activeLibraryName={ACTIVE_LIBRARY.name}
          librarySwitcherButtonRef={setAnchor}
          librarySwitcherOpen={open}
          onLocateFolder={() => undefined}
          onOpenLibrarySwitcher={openSwitcher}
          onOpenPlugins={() => undefined}
          onOpenSettings={() => undefined}
          onToggleSidebars={() => undefined}
          sidebarsHidden={false}
        />
      )}
      <LibrarySwitcher
        anchor={loading ? null : anchor}
        autoFocus={openSource.autoFocus}
        library={library}
        notificationContext={{
          libraryId: ACTIVE_LIBRARY.id,
          libraryName: ACTIVE_LIBRARY.name,
        }}
        onClose={() => overlays.closeRoot("library")}
        open={open}
        restoreFocusTarget={openSource.element}
      />
    </LiquidGlassLayer>
  );
}

function HookBackedLibrarySwitcherScenario({
  adapterControl,
  control,
}: {
  adapterControl: AdapterRemoveControl;
  control: ScenarioControl;
}) {
  const library = useLibraryData({
    activeLibraryId: ACTIVE_LIBRARY.id,
    activeLibraryName: ACTIVE_LIBRARY.name,
    persistentThumbnailCacheAutoRefresh: false,
  });
  adapterControl.rootCount = library.roots.length;
  const observedLibrary: LibraryState = {
    ...library,
    removeRoot: async (rootId) => {
      try {
        const result = await library.removeRoot(rootId);
        adapterControl.removeResult = result;
        return result;
      } catch (cause) {
        adapterControl.removeResult = "rejected";
        throw cause;
      }
    },
  };
  return <ProductionLibrarySwitcherScenario control={control} library={observedLibrary} />;
}

function HardMediaCancellationScenario({
  control,
}: {
  control: HardMediaCancellationControl;
}) {
  control.library = useLibraryData({
    activeLibraryId: ACTIVE_LIBRARY.id,
    activeLibraryName: ACTIVE_LIBRARY.name,
    persistentThumbnailCacheAutoRefresh: false,
  });
  return null;
}

async function openFromTitlebar(): Promise<HTMLButtonElement> {
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');
  if (!(trigger instanceof HTMLButtonElement)) throw new Error("Library trigger is not a button");
  await act(async () => {
    trigger.click();
  });
  if (!(await findOpenSurface("library-root"))) {
    throw new Error("LibrarySwitcher root did not open");
  }
  return trigger;
}

async function runProductionWiringScenario() {
  installBridge();
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');
  if (!(trigger instanceof HTMLButtonElement)) throw new Error("Library trigger is not a button");
  const controls = trigger.getAttribute("aria-controls");
  const hasPopup = trigger.getAttribute("aria-haspopup");
  const expandedWhenClosed = trigger.getAttribute("aria-expanded");

  trigger.focus();
  await requestNativeKey("Space");
  const keyboardSurface = await findOpenSurface("library-root");
  if (!keyboardSurface) throw new Error("Keyboard activation did not open LibrarySwitcher");
  await settle(30);
  const keyboardFocusedSearch =
    document.activeElement === keyboardSurface.querySelector('input[aria-label="搜索资源库"]');
  const expandedWhenOpen = trigger.getAttribute("aria-expanded");
  const activeWhenOpen = trigger.dataset.glassActive === "true";
  await requestNativeKey("Escape");
  await settle(30);
  const keyboardRestoredTrigger = document.activeElement === trigger;
  await waitForLayerRemoval("library-root");

  await requestNativeClick(trigger);
  const pointerSurface = await findOpenSurface("library-root");
  if (!pointerSurface) throw new Error("Pointer activation did not open LibrarySwitcher");
  await settle(30);
  const mousePreservedTriggerFocus = document.activeElement === trigger;

  const result = {
    activeWhenOpen,
    controls,
    expandedWhenClosed,
    expandedWhenOpen,
    hasPopup,
    keyboardFocusedSearch,
    keyboardRestoredTrigger,
    mousePreservedTriggerFocus,
  };
  await unmountScenario(mounted);
  return result;
}

async function runLoadingSourceFocusScenario() {
  installBridge();
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} loading />
    </OverlayProvider>,
  );
  const source = await findButton("切换其它库");
  source.focus();
  await requestNativeKey("Space");
  const surface = await findOpenSurface("library-root");
  const search = surface?.querySelector('input[aria-label="搜索资源库"]');
  const focusedSearch = search instanceof HTMLInputElement && document.activeElement === search;
  if (search instanceof HTMLInputElement && !focusedSearch) search.focus();
  await requestNativeKey("Escape");
  await settle(30);
  const result = {
    focusedSearch,
    restoredSource: document.activeElement === source,
    visible: surface !== null,
  };
  await unmountScenario(mounted);
  return result;
}

async function runFocusRestoreScenario() {
  installBridge();
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  await openFromTitlebar();
  const removeTrigger = await findElement('button[aria-label="Remove folder"]');
  await requestNativeClick(removeTrigger);
  const removeMenu = await findOpenSurface("library-remove-folders");
  if (!removeMenu) throw new Error("Remove-folder menu did not open");
  const removeItem = await findButton("Pictures", removeMenu);

  await requestNativeClick(removeItem);
  const cancelModal = await findOpenLayer("library-remove-confirm");
  if (!cancelModal) throw new Error("Remove confirmation did not open for cancellation");
  const cancelButton = await findButton("取消", cancelModal);
  await requestNativeClick(cancelButton);
  await settle(30);
  const cancelRestoredTrigger = document.activeElement === removeTrigger;
  await waitForLayerRemoval("library-remove-confirm");

  await requestNativeClick(removeItem);
  const escapeModal = await findOpenLayer("library-remove-confirm");
  if (!escapeModal) throw new Error("Remove confirmation did not reopen for Escape");
  await requestNativeKey("Escape");
  await settle(30);
  const escapeRestoredTrigger = document.activeElement === removeTrigger;

  const result = {
    cancelRestoredTrigger,
    escapeRestoredTrigger,
    nestedMenuStayedOpen:
      document.getElementById("library-remove-folders")?.dataset.overlayState === "open",
    rootStayedOpen:
      control.api?.activeRootId === "library" &&
      document.getElementById("library-root")?.dataset.overlayState === "open",
  };
  await unmountScenario(mounted);
  return result;
}

async function runPendingRemoveScenario() {
  const removeRequest = deferred<boolean>();
  let removeCalls = 0;
  let reports = 0;
  let unhandledRejections = 0;
  const handleUnhandled = (event: PromiseRejectionEvent) => {
    unhandledRejections += 1;
    event.preventDefault();
  };
  window.addEventListener("unhandledrejection", handleUnhandled);
  installBridge({
    notifications: {
      reportError: async () => {
        reports += 1;
        return true;
      },
    } as unknown as MegleDesktopBridge["notifications"],
  });
  const library = createLibraryState({
    removeRoot: async () => {
      removeCalls += 1;
      return removeRequest.promise;
    },
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} library={library} />
    </OverlayProvider>,
  );
  await openFromTitlebar();
  const removeTrigger = await findElement('button[aria-label="Remove folder"]');
  await requestNativeClick(removeTrigger);
  const removeMenu = await findOpenSurface("library-remove-folders");
  if (!removeMenu) throw new Error("Remove-folder menu did not open");
  await requestNativeClick(await findButton("Pictures", removeMenu));
  const modal = await findOpenLayer("library-remove-confirm");
  if (!modal) throw new Error("Remove confirmation did not open");
  const cancelButton = await findButton("取消", modal);
  const removeButton = await findButton("移除", modal);

  await act(async () => {
    removeButton.click();
    removeButton.click();
  });
  await settle(20);
  const busy = modal.getAttribute("aria-busy") === "true";
  const cancelDisabled = cancelButton.disabled;
  const actionDisabled = removeButton.disabled;

  await requestNativeKey("Escape");
  await settle(20);
  const escapeBlocked = document.getElementById("library-remove-confirm")?.dataset.overlayState === "open";
  await requestNativePoint(4, 4);
  await settle(20);
  const backdropBlocked = document.getElementById("library-remove-confirm")?.dataset.overlayState === "open";
  await act(async () => {
    cancelButton.click();
  });
  await settle(20);
  const cancelBlocked = document.getElementById("library-remove-confirm")?.dataset.overlayState === "open";

  removeRequest.reject(new Error("remove failed"));
  await settle(60);
  const recoveredModal = document.getElementById("library-remove-confirm");
  const recovered =
    recoveredModal?.dataset.overlayState === "open" &&
    recoveredModal.getAttribute("aria-busy") !== "true" &&
    !cancelButton.disabled &&
    !removeButton.disabled;

  const result = {
    actionDisabled,
    backdropBlocked,
    busy,
    cancelBlocked,
    cancelDisabled,
    duplicateCalls: removeCalls,
    escapeBlocked,
    recovered,
    reports,
    unhandledRejections,
  };
  window.removeEventListener("unhandledrejection", handleUnhandled);
  await unmountScenario(mounted);
  return result;
}

async function runAdapterRemoveScenario({
  deleteFails,
  refreshFails,
}: {
  deleteFails: boolean;
  refreshFails: boolean;
}) {
  const originalFetch = window.fetch;
  const coreUrl = "http://library-adapter.fixture/api";
  let deleteCalls = 0;
  let reports = 0;
  const root = {
    createdAt: 1,
    displayName: "Pictures",
    enabled: true,
    id: 7,
    lastScanAt: null,
    mediaCount: 0,
    path: "C:\\Fixture\\Pictures",
    reachable: true,
    rootFolderId: null,
  };
  const jsonResponse = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), {
      headers: { "content-type": "application/json" },
      status,
    });
  window.fetch = (async (input, init) => {
    const requestUrl =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url;
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    const url = new URL(requestUrl);
    if (!url.href.startsWith(coreUrl)) return originalFetch(input, init);
    if (method === "DELETE" && url.pathname === "/api/roots/7") {
      deleteCalls += 1;
      return deleteFails
        ? jsonResponse({ code: "remove-failed", message: "Core remove failed" }, 500)
        : jsonResponse({ accepted: true, rootId: 7, taskId: 11 });
    }
    if (method === "GET" && url.pathname === "/api/roots") {
      if (refreshFails && deleteCalls > 0) {
        return jsonResponse({ code: "refresh-failed", message: "Core refresh failed" }, 500);
      }
      return jsonResponse({ items: [root], nextCursor: null, totalCount: 1 });
    }
    return jsonResponse({ items: [], nextCursor: null, totalCount: 0 });
  }) as typeof window.fetch;
  installBridge({
    coreUrl,
    notifications: {
      reportError: async () => {
        reports += 1;
        return true;
      },
    } as unknown as MegleDesktopBridge["notifications"],
    sessionToken: "fixture-token",
  });
  const adapterControl: AdapterRemoveControl = {
    removeResult: "pending",
    rootCount: 0,
  };
  const control: ScenarioControl = { api: null };
  let mounted: MountedScenario | null = null;
  try {
    mounted = await mountScenario(
      <OverlayProvider>
        <HookBackedLibrarySwitcherScenario
          adapterControl={adapterControl}
          control={control}
        />
      </OverlayProvider>,
    );
    await waitFor(() => adapterControl.rootCount === 1, "useLibraryData roots");
    await openFromTitlebar();
    await requestNativeClick(await findElement('button[aria-label="Remove folder"]'));
    const removeMenu = await findOpenSurface("library-remove-folders");
    if (!removeMenu) throw new Error("Adapter remove-folder menu did not open");
    await requestNativeClick(await findButton("Pictures", removeMenu));
    const modal = await findOpenLayer("library-remove-confirm");
    if (!modal) throw new Error("Adapter remove confirmation did not open");
    const removeButton = await findButton("移除", modal);
    await requestNativeClick(removeButton);
    await waitFor(
      () => adapterControl.removeResult !== "pending",
      "useLibraryData remove result",
    );
    await settle(60);
    const recoveredModal = document.getElementById("library-remove-confirm");
    return {
      actionEnabled: !removeButton.disabled,
      busyCleared: recoveredModal?.getAttribute("aria-busy") !== "true",
      contractResult: adapterControl.removeResult as boolean | "rejected",
      deleteCalls,
      modalStayedOpen: recoveredModal?.dataset.overlayState === "open",
      reports,
    };
  } finally {
    if (mounted) await unmountScenario(mounted);
    window.fetch = originalFetch;
  }
}

async function runIconMenuScenario() {
  let keyboardSelections = 0;
  installBridge({
    setLibraryIcon: async () => {
      keyboardSelections += 1;
      return { ok: true };
    },
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  await openFromTitlebar();
  const iconTrigger = await findElement('button[aria-label="更改图标"]');
  await requestNativeClick(iconTrigger);
  const surface = await findOpenSurface(`library-icon-${ACTIVE_LIBRARY.id}`);
  if (!surface) throw new Error("Library icon menu did not open");
  await settle(50);
  const menus = surface.querySelectorAll('[role="menu"]');
  const radioItems = Array.from(surface.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]'));
  const focusedRadioItem =
    document.activeElement instanceof HTMLButtonElement &&
    document.activeElement.getAttribute("role") === "menuitemradio";
  const checkedItems = radioItems.filter((item) => item.getAttribute("aria-checked") === "true").length;
  const rovingTabStops = radioItems.filter((item) => item.tabIndex === 0).length;
  await requestNativeKey("ArrowRight");
  await requestNativeKey("Enter");
  await settle(30);

  const result = {
    checkedItems,
    focusedRadioItem,
    keyboardSelections,
    menuRoleCount: menus.length,
    radioItems: radioItems.length,
    rovingTabStops,
    surfaceRole: surface.getAttribute("role"),
    visualGridPresent:
      surface.querySelector(".library-icon-picker-grid") !== null &&
      surface.querySelector(".library-icon-picker-colors") !== null,
  };
  await unmountScenario(mounted);
  return result;
}

async function runRequestOwnershipScenario() {
  const listRequests: Array<Deferred<LibrarySummary[]>> = [];
  const iconRequest = deferred<{ ok: boolean }>();
  let reports = 0;
  let unmountUnhandledRejections = 0;
  const handleUnhandled = (event: PromiseRejectionEvent) => {
    unmountUnhandledRejections += 1;
    event.preventDefault();
  };
  window.addEventListener("unhandledrejection", handleUnhandled);
  installBridge({
    listLibraries: () => {
      const request = deferred<LibrarySummary[]>();
      listRequests.push(request);
      return request.promise;
    },
    setLibraryIcon: () => iconRequest.promise,
    notifications: {
      reportError: async () => {
        reports += 1;
        return true;
      },
    } as unknown as MegleDesktopBridge["notifications"],
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');
  await act(async () => trigger.click());
  await waitFor(() => listRequests.length === 1, "first library request");
  await requestNativeKey("Escape");
  await waitForLayerRemoval("library-root");
  await act(async () => trigger.click());
  await waitFor(() => listRequests.length === 2, "second library request");

  listRequests[1].resolve([{ ...ACTIVE_LIBRARY, name: "Fresh Library" }]);
  await settle(30);
  const freshResponseWon = document.body.textContent?.includes("Fresh Library") ?? false;
  listRequests[0].resolve([{ ...ACTIVE_LIBRARY, name: "Stale Library" }]);
  await settle(30);
  const staleResponseIgnored =
    (document.body.textContent?.includes("Fresh Library") ?? false) &&
    !(document.body.textContent?.includes("Stale Library") ?? false);

  await requestNativeClick(await findElement('button[aria-label="更改图标"]'));
  const iconSurface = await findOpenSurface(`library-icon-${ACTIVE_LIBRARY.id}`);
  if (!iconSurface) throw new Error("Icon menu did not open for ownership test");
  await requestNativeClick(await findElement(".library-icon-picker-cell", iconSurface));
  await requestNativeKey("Escape");
  await requestNativeKey("Escape");
  await waitForLayerRemoval("library-root");
  await act(async () => trigger.click());
  await waitFor(() => listRequests.length === 3, "new-session library request");
  listRequests[2].resolve([{ ...ACTIVE_LIBRARY, name: "Fresh Library" }]);
  await settle(30);
  iconRequest.resolve({ ok: true });
  await settle(40);
  const staleMutationIgnored = listRequests.length === 3;

  await requestNativeKey("Escape");
  await waitForLayerRemoval("library-root");
  const requestsBeforeUnmountOpen = listRequests.length;
  await act(async () => trigger.click());
  await waitFor(
    () => listRequests.length === requestsBeforeUnmountOpen + 1,
    "unmount library request",
  );
  const unmountRequest = listRequests.at(-1);
  if (!unmountRequest) throw new Error("Unmount library request was not captured");
  await unmountScenario(mounted);
  unmountRequest.reject(new Error("unmounted load failed"));
  await delay(60);
  window.removeEventListener("unhandledrejection", handleUnhandled);
  return {
    freshResponseWon,
    staleMutationIgnored,
    staleResponseIgnored,
    unmountReports: reports,
    unmountUnhandledRejections,
  };
}

async function runMediaOwnershipBoundary(boundary: "starting-core" | "unmount") {
  const originalFetch = window.fetch;
  const coreUrl = "http://hard-media-cancel.fixture/api";
  const mediaRequests: Array<{
    offset: string | null;
    response: Deferred<Response>;
    settled: boolean;
    signal: AbortSignal;
  }> = [];
  let phaseListener: ((phase: LibraryLoadPhase) => void) | null = null;
  let unhandledRejections = 0;
  const handleUnhandled = (event: PromiseRejectionEvent) => {
    unhandledRejections += 1;
    event.preventDefault();
  };
  window.addEventListener("unhandledrejection", handleUnhandled);
  const root = {
    createdAt: 1,
    displayName: "Pictures",
    enabled: true,
    id: 7,
    lastScanAt: null,
    mediaCount: 0,
    path: "C:\\Fixture\\Pictures",
    reachable: true,
    rootFolderId: null,
  };
  const jsonResponse = (body: unknown) =>
    new Response(JSON.stringify(body), {
      headers: { "content-type": "application/json" },
      status: 200,
    });
  window.fetch = (async (input, init) => {
    const requestUrl =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url;
    const url = new URL(requestUrl);
    if (!url.href.startsWith(coreUrl)) return originalFetch(input, init);
    if (url.pathname === "/api/roots") {
      return jsonResponse({ items: [root], nextCursor: null, totalCount: 1 });
    }
    if (url.pathname === "/api/media") {
      if (!init?.signal) throw new Error("production media fetch must own an AbortSignal");
      if (url.searchParams.get("offset") === "0" && !url.searchParams.has("cursor")) {
        return jsonResponse({ items: [], nextCursor: null, totalCount: 1_000 });
      }
      const response = deferred<Response>();
      const request = {
        offset: url.searchParams.get("offset"),
        response,
        settled: false,
        signal: init.signal,
      };
      mediaRequests.push(request);
      init.signal.addEventListener(
        "abort",
        () => {
          request.settled = true;
          response.reject(init.signal?.reason ?? new DOMException("The operation was aborted.", "AbortError"));
        },
        { once: true },
      );
      return response.promise;
    }
    return jsonResponse({ items: [], nextCursor: null, totalCount: 0 });
  }) as typeof window.fetch;
  installBridge({
    coreUrl,
    getLibraryPhase: async () => "ready",
    notifications: {
      reportError: async () => true,
    } as unknown as MegleDesktopBridge["notifications"],
    onLibraryPhase: (listener) => {
      phaseListener = listener;
      return () => {
        if (phaseListener === listener) phaseListener = null;
      };
    },
    sessionToken: "fixture-token",
  });
  const control: HardMediaCancellationControl = { library: null };
  let mounted: MountedScenario | null = null;
  try {
    mounted = await mountScenario(<HardMediaCancellationScenario control={control} />);
    await waitFor(() => control.library?.mediaTotalCount === 1_000, "initial media total count");
    if (!control.library) throw new Error("useLibraryData control was not mounted");
    const library = control.library;

    await act(async () => library.requestMediaWindow(200, 300));
    await waitFor(() => mediaRequests.length === 1, "first A media window");
    await act(async () => library.requestMediaWindow(400, 500));
    await waitFor(() => mediaRequests.length === 2, "B media window");
    await act(async () => library.requestMediaWindow(200, 300));

    const oldA = mediaRequests[0];
    const retiredB = mediaRequests[1];
    retiredB.settled = true;
    retiredB.response.resolve(jsonResponse({ items: [], nextCursor: null, totalCount: 1_000 }));
    await waitFor(() => mediaRequests.length === 3, "replacement A media window");
    const replacementA = mediaRequests[2];

    oldA.settled = true;
    oldA.response.resolve(jsonResponse({
      items: [{
        ext: "jpg",
        folderId: null,
        height: 10,
        id: 99,
        kind: "image",
        mtime: 1,
        name: "stale.jpg",
        rootId: 7,
        size: 1,
        width: 10,
      }],
      nextCursor: null,
      totalCount: 1_000,
    }));
    await settle(20);
    const staleResponseIgnored = control.library?.media.length === 0;

    await act(async () => library.requestMediaWindow(200, 300));
    await delay(20);
    const noDuplicateAfterRetiredSettlement = mediaRequests.length === 3;

    await act(async () => library.requestMediaWindow(600, 700));
    await delay(20);
    const activeC = mediaRequests.find(({ offset }) => offset === "600");
    const liveRequests = mediaRequests.filter((request) => !request.settled);

    if (boundary === "starting-core") {
      if (!phaseListener) throw new Error("library phase listener was not installed");
      await act(async () => phaseListener?.("starting-core"));
      await settle(20);
    } else {
      await unmountScenario(mounted);
      mounted = null;
    }

    const activeFetchAborted = activeC?.signal.aborted ?? false;
    const allLiveFetchesAborted = liveRequests.every(({ signal }) => signal.aborted);
    const retiredFetchAborted = replacementA.signal.aborted;
    for (const request of mediaRequests) {
      if (!request.settled) {
        request.settled = true;
        request.response.resolve(jsonResponse({ items: [], nextCursor: null, totalCount: 1_000 }));
      }
    }
    await Promise.all(
      mediaRequests.map(({ response }) => response.promise.catch(() => undefined)),
    );
    await settle(20);
    return {
      activeFetchAborted,
      allLiveFetchesAborted,
      noDuplicateAfterRetiredSettlement,
      retiredFetchAborted,
      staleResponseIgnored,
      unhandledRejections,
    };
  } finally {
    if (mounted) await unmountScenario(mounted);
    for (const request of mediaRequests) {
      if (!request.settled) {
        request.settled = true;
        request.response.resolve(jsonResponse({ items: [], nextCursor: null, totalCount: 1_000 }));
      }
    }
    await Promise.all(
      mediaRequests.map(({ response }) => response.promise.catch(() => undefined)),
    );
    window.removeEventListener("unhandledrejection", handleUnhandled);
    window.fetch = originalFetch;
  }
}

async function runHardMediaCancellationScenario() {
  const startingCore = await runMediaOwnershipBoundary("starting-core");
  const unmount = await runMediaOwnershipBoundary("unmount");
  return {
    activeFetchAbortedOnStartingCore:
      startingCore.activeFetchAborted && startingCore.allLiveFetchesAborted,
    activeFetchAbortedOnUnmount: unmount.activeFetchAborted && unmount.allLiveFetchesAborted,
    noDuplicateAfterRetiredSettlement:
      startingCore.noDuplicateAfterRetiredSettlement && unmount.noDuplicateAfterRetiredSettlement,
    retiredFetchAbortedOnStartingCore: startingCore.retiredFetchAborted,
    retiredFetchAbortedOnUnmount: unmount.retiredFetchAborted,
    staleResponseIgnored: startingCore.staleResponseIgnored && unmount.staleResponseIgnored,
    unhandledRejections: startingCore.unhandledRejections + unmount.unhandledRejections,
  };
}

async function runMutationFailureScenario() {
  let reports = 0;
  let unhandledRejections = 0;
  const handleUnhandled = (event: PromiseRejectionEvent) => {
    unhandledRejections += 1;
    event.preventDefault();
  };
  window.addEventListener("unhandledrejection", handleUnhandled);
  installBridge({
    createLibrary: async () => Promise.reject(new Error("create failed")),
    openLibrary: async () => Promise.reject(new Error("open failed")),
    pickFolder: async () => "C:\\Fixture\\folder",
    pickLibraryFile: async () => "C:\\Fixture\\open.sqlite",
    pickNewLibraryPath: async () => "C:\\Fixture\\create.sqlite",
    setLibraryIcon: async () => Promise.reject(new Error("icon failed")),
    switchLibrary: async () => Promise.reject(new Error("switch failed")),
    notifications: {
      reportError: async () => {
        reports += 1;
        return true;
      },
    } as unknown as MegleDesktopBridge["notifications"],
  });
  const library = createLibraryState({
    addRoot: async () => Promise.reject(new Error("add failed")),
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} library={library} />
    </OverlayProvider>,
  );
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');

  await act(async () => trigger.click());
  const firstRoot = await findOpenSurface("library-root");
  if (!firstRoot) throw new Error("Library root did not open for mutation failures");
  await settle(30);
  await requestNativeClick(
    await findElement(".library-switcher-row:not(.active) .library-switcher-main", firstRoot),
  );
  await settle(40);
  await waitForLayerRemoval("library-root");

  await act(async () => trigger.click());
  const createRoot = await findOpenSurface("library-root");
  if (!createRoot) throw new Error("Library root did not reopen for create failure");
  await requestNativeClick(await findButton("创建资源库…", createRoot));
  await settle(40);
  await waitForLayerRemoval("library-root");

  await act(async () => trigger.click());
  const openRoot = await findOpenSurface("library-root");
  if (!openRoot) throw new Error("Library root did not reopen for open failure");
  await requestNativeClick(await findButton("打开其它资源库…", openRoot));
  await settle(40);
  await waitForLayerRemoval("library-root");

  await act(async () => trigger.click());
  const addRoot = await findOpenSurface("library-root");
  if (!addRoot) throw new Error("Library root did not reopen for add failure");
  await requestNativeClick(await findElement('button[aria-label="Add folder"]', addRoot));
  await settle(40);

  await requestNativeClick(await findElement('button[aria-label="更改图标"]', addRoot));
  const iconSurface = await findOpenSurface(`library-icon-${ACTIVE_LIBRARY.id}`);
  if (!iconSurface) throw new Error("Icon menu did not open for icon failure");
  await requestNativeClick(await findElement(".library-icon-picker-cell", iconSurface));
  await settle(60);

  const result = { reports, unhandledRejections };
  window.removeEventListener("unhandledrejection", handleUnhandled);
  await unmountScenario(mounted);
  return result;
}

async function run(): Promise<void> {
  try {
    lifecycleWindow.__overlayLifecycleResult = {
      productionWiring: await runProductionWiringScenario(),
      loadingSourceFocus: await runLoadingSourceFocusScenario(),
      focusRestore: await runFocusRestoreScenario(),
      pendingRemove: await runPendingRemoveScenario(),
      adapterRemoveFailure: await runAdapterRemoveScenario({
        deleteFails: true,
        refreshFails: false,
      }),
      adapterRemoveFailureWithRefreshFailure: await runAdapterRemoveScenario({
        deleteFails: true,
        refreshFails: true,
      }),
      adapterRemoveSuccessWithRefreshFailure: await runAdapterRemoveScenario({
        deleteFails: false,
        refreshFails: true,
      }),
      iconMenu: await runIconMenuScenario(),
      requestOwnership: await runRequestOwnershipScenario(),
      hardMediaCancellation: await runHardMediaCancellationScenario(),
      mutationFailures: await runMutationFailureScenario(),
      ok: true,
    };
  } catch (error) {
    lifecycleWindow.__overlayLifecycleResult = {
      error: error instanceof Error ? error.stack ?? error.message : String(error),
      ok: false,
    };
  }
}

void run();

import {
  act,
  useLayoutEffect,
  useRef,
  useState,
  type ReactElement,
} from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { forgetLibraryEntry, type MangooDesktopBridge } from "../../src/core/desktop";
import { subscribeLibraryPhase } from "../../src/core/desktop";
import type { LibraryLoadPhase, LibrarySummary } from "../../src/core/libraryTypes";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import { ShellTitlebarPrimaryActions } from "../../src/app-shell/ShellTopBar";
import {
  OverlayProvider,
  useOverlayStack,
  type OverlayStackApi,
} from "../../src/design/overlays";
import { LiquidGlassLayer, useInterfaceStyle } from "../../src/design/liquid-glass";
import { LibraryLoadingOverlay } from "../../src/features/library/LibraryLoadingOverlay";
import { LibrarySwitcher } from "../../src/features/library/LibrarySwitcher";
import { LibraryAppearanceEditor } from "../../src/features/library/LibraryAppearanceEditor";
import { IDENTITY_CATEGORIES, IDENTITY_ICONS, matchesIdentityIcon } from "../../src/features/library/identityArtwork";
import { useShortcuts } from "../../src/features/shortcuts/useShortcuts";
import type { FileOpsController } from "../../src/features/file-ops/useFileOps";
import { SettingsPanels } from "../../src/features/settings/settingsPanels";
import { DEFAULT_LIBRARY_GRID_PREFERENCES } from "../../src/features/media-grid/gridPreferences";
import { DEFAULT_PREVIEW_PREFERENCES } from "../../src/features/preview/previewPreferences";
import { queryClient } from "../../src/core/queryClient";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

type NativeInputPayload =
  | { keyCode: string; type: "key" | "cdp-key" }
  | { type: "mouse-click" | "mouse-move"; x: number; y: number };
type NativeInputRequest = NativeInputPayload & { id: number };

interface LibrarySwitcherLifecycleResult {
  ok: boolean;
  error?: string;
  followup?: string[];
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
  addFolderAfterBlur?: {
    addedPath: string | null;
    addCalls: number;
    committedRootUuid: string | null;
    commitCalls: number;
    enrollmentMode: string | null;
    enrolledExistingRoot: boolean;
    refreshCalls: number;
    reloadsAfterSelection: number;
    requestedName: string | null;
    rootFolderUuidIsValid: boolean;
    localAddedPath: string | null;
    localAddCalls: number;
    localEnrollmentCalls: number;
    localReloadsAfterSelection: number;
    switchedLibraryAddCalls: number;
    switchedLibraryEnrollmentCalls: number;
    switchedLibraryReports: number;
    switcherClosed: boolean;
  };
  managedLibraryWizard?: {
    createPersisted: boolean;
    explicitCloseRemoved: boolean;
    firstRunPersisted: boolean;
    focusRestored: boolean;
    modeSwitchPersisted: boolean;
    restoreModeSelected: boolean;
    restorePersisted: boolean;
  };
}

interface MountedScenario {
  container: HTMLDivElement;
  root: Root;
}

interface ScenarioControl {
  api: OverlayStackApi | null;
  sidebarToggles?: number;
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

const lifecycleWindow = window as Omit<Window, "__overlayLifecycleInputRequest"> & {
  __overlayLifecycleInputRequest?: NativeInputRequest;
  __overlayLifecycleInputResponseId?: number;
  __overlayLifecycleResult?: LibrarySwitcherLifecycleResult;
};

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const ACTIVE_LIBRARY: LibrarySummary = {
  active: true,
  availability: "available",
  bindingState: "local-only",
  contextRevision: 1,
  libraryUuid: "fixture-library",
  lifecycleState: "ready",
  databasePath: "C:\\Fixture\\library",
  name: "Alpha Library",
  rootUuid: "fixture-root",
};

const INACTIVE_LIBRARY: LibrarySummary = {
  active: false,
  availability: "available",
  bindingState: "local-only",
  contextRevision: 1,
  libraryUuid: "beta-library",
  lifecycleState: "ready",
  databasePath: "C:\\Fixture\\beta",
  name: "Beta Library",
  rootUuid: "beta-root",
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

type FixtureBridgeOverrides = Partial<MangooDesktopBridge> & {
  coreRequest?: (url: URL, method: string, init?: RequestInit) => Promise<Response | null> | Response | null;
  createLibrary?: () => Promise<unknown>;
  listLibraries?: () => Promise<LibrarySummary[]> | LibrarySummary[];
  openLibrary?: () => Promise<unknown>;
  pickLibraryFile?: () => Promise<string | null>;
  setLibraryIcon?: () => Promise<unknown>;
  switchLibrary?: () => Promise<unknown>;
};

const fixtureCoreUrl = "http://library-fixture.test/api";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json" },
    status,
  });
}

function installBridge(overrides: FixtureBridgeOverrides = {}): void {
  const {
    coreRequest,
    createLibrary: createLibraryOverride,
    listLibraries: listLibrariesOverride,
    openLibrary: _openLibraryOverride,
    pickLibraryFile: _pickLibraryFileOverride,
    setLibraryIcon: setLibraryIconOverride,
    switchLibrary: switchLibraryOverride,
    ...desktopOverrides
  } = overrides;
  window.mangooDesktop = {
    coreUrl: fixtureCoreUrl,
    sessionToken: "fixture-session-token",
    ...desktopOverrides,
  } as MangooDesktopBridge;

  if (desktopOverrides.coreUrl && desktopOverrides.coreUrl !== fixtureCoreUrl) return;

  window.fetch = (async (input, init) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
    if (!url.href.startsWith(fixtureCoreUrl)) {
      throw new Error(`Unexpected fixture request: ${url.href}`);
    }
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    const overridden = await coreRequest?.(url, method, init);
    if (overridden) return overridden;
    if (url.pathname === "/api/libraries" && method === "GET") {
      const items = await (listLibrariesOverride?.() ?? [ACTIVE_LIBRARY, INACTIVE_LIBRARY]);
      return jsonResponse({ items });
    }
    if (url.pathname === "/api/libraries/active" && method === "GET") {
      return jsonResponse({ contextRevision: ACTIVE_LIBRARY.contextRevision, library: ACTIVE_LIBRARY });
    }
    if (url.pathname === "/api/libraries" && method === "POST") {
      await createLibraryOverride?.();
      return jsonResponse({ library: ACTIVE_LIBRARY }, 201);
    }
    if (url.pathname.endsWith("/activate") && method === "POST") {
      await switchLibraryOverride?.();
      return jsonResponse({ contextRevision: ACTIVE_LIBRARY.contextRevision + 1, library: ACTIVE_LIBRARY });
    }
    if (/\/api\/libraries\/[^/]+$/.test(url.pathname) && method === "PATCH") {
      await setLibraryIconOverride?.();
      return jsonResponse({ library: ACTIVE_LIBRARY });
    }
    if (/\/api\/libraries\/[^/]+\/local-copy$/.test(url.pathname) && method === "DELETE") {
      return jsonResponse({ ok: true });
    }
    if (url.pathname === "/api/libraries/preflight" && method === "POST") {
      return jsonResponse({
        normalizedDatabasePath: "C:\\Fixture\\library",
        normalizedContentRoots: [{ path: "C:/Fixture/Files", createIfMissing: true }],
        pathRole: null, rootIndex: null,
        ok: true,
        preflightToken: "fixture-preflight-token",
        reason: "ok",
      });
    }
    throw new Error(`Unhandled fixture request: ${method} ${url.pathname}`);
  }) as typeof window.fetch;
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

const requestNativeClick = async (element: HTMLElement) => {
  element.scrollIntoView({ behavior: "instant", block: "nearest", inline: "nearest" });
  let bounds = element.getBoundingClientRect();
  let stable = 0;
  for (let attempt = 0; attempt < 30; attempt++) {
    await settle(20);
    const next = element.getBoundingClientRect();
    const hit = document.elementFromPoint(next.left + next.width / 2, next.top + next.height / 2);
    stable = element.contains(hit) && Math.abs(next.left - bounds.left) < 0.5 && Math.abs(next.top - bounds.top) < 0.5 ? stable + 1 : 0;
    bounds = next;
    if (stable >= 2) break;
  }
  if (stable < 2) throw new Error(`Native click target is obscured or moving: ${element.dataset.icon ?? element.className}`);
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
    root.render(<QueryClientProvider client={queryClient}>{element}</QueryClientProvider>);
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

async function findLibraryAction(selector: string): Promise<HTMLElement> {
  if (!document.querySelector('[id^="library-actions-"][data-overlay-state="open"]')) {
    await requestNativeClick(await findElement('.library-switcher-row.active .library-switcher-more'));
    await findOpenSurface(`library-actions-${ACTIVE_LIBRARY.libraryUuid}`);
  }
  return findElement(selector);
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
  const fileOperationRestoreFocusRef = useRef<HTMLElement | null>(null);
  const fileOps: FileOpsController = {
    rename: { target: null, busy: false, serverError: null },
    move: { target: null, busy: false, serverError: null, serverErrorCode: null },
    remove: { target: null, permanent: false, busy: false, serverError: null },
    openRename: () => undefined, openMove: () => undefined, openDelete: () => undefined,
    closeAll: () => undefined,
    submitRename: async () => undefined, submitMove: async () => undefined, submitDelete: async () => undefined,
  };
  useShortcuts({
    fileOps, library, fileOperationRestoreFocusRef, previewOpen: false,
    onClosePreview: () => undefined, onDismissContextMenu: () => undefined,
    onToggleSidebars: () => { control.sidebarToggles = (control.sidebarToggles ?? 0) + 1; },
  });
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
          libraryId: ACTIVE_LIBRARY.libraryUuid,
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
    activeLibraryId: ACTIVE_LIBRARY.libraryUuid,
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
    activeLibraryId: ACTIVE_LIBRARY.libraryUuid,
    activeLibraryName: ACTIVE_LIBRARY.name,
    persistentThumbnailCacheAutoRefresh: false,
  });
  return null;
}

async function openFromTitlebar(): Promise<HTMLButtonElement> {
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');
  if (!(trigger instanceof HTMLButtonElement)) throw new Error("Library trigger is not a button");
  if (trigger.getAttribute("aria-expanded") !== "true") await act(async () => {
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

async function runManagedLibraryWizardScenario() {
  installBridge();
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  const trigger = await openFromTitlebar();
  const switcher = await findOpenSurface("library-root");
  if (!switcher) throw new Error("Library root was not available for create wizard");
  await requestNativeClick(await findButton("创建资源库…", switcher));
  const createWizard = await findOpenLayer("managed-library-wizard");
  if (!createWizard) throw new Error("Create Library wizard did not remain open");
  await settle(220);
  const createPersisted =
    document.getElementById("managed-library-wizard")?.dataset.overlayState === "open" &&
    control.api?.activeRootId === "managed-library";
  const modeSwitchPersisted = !createWizard.textContent?.includes("从远端恢复");
  await requestNativeClick(await findButton("取消", createWizard));
  await waitForLayerRemoval("managed-library-wizard");
  const explicitCloseRemoved = document.getElementById("managed-library-wizard") === null;
  const focusRestored = document.activeElement === trigger;

  const restoreModeSelected = !switcher.textContent?.includes("从远端恢复");
  const restorePersisted = !document.querySelector("[data-plugin-view]");
  await unmountScenario(mounted);

  installBridge({ listLibraries: async () => [] });
  const firstRunControl: ScenarioControl = { api: null };
  const firstRunMounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={firstRunControl} />
    </OverlayProvider>,
  );
  const firstRunWizard = await findOpenLayer("managed-library-wizard");
  if (!firstRunWizard) throw new Error("First-run library wizard did not remain open");
  await settle(220);
  const firstRunPersisted =
    firstRunWizard.dataset.overlayState === "open" &&
    firstRunControl.api?.activeRootId === "managed-library";
  await unmountScenario(firstRunMounted);

  return {
    createPersisted,
    explicitCloseRemoved,
    firstRunPersisted,
    focusRestored,
    modeSwitchPersisted,
    restoreModeSelected,
    restorePersisted,
  };
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
  const removeTrigger = await findLibraryAction('button[aria-label="Remove folder"]');
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
    } as unknown as MangooDesktopBridge["notifications"],
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
  const removeTrigger = await findLibraryAction('button[aria-label="Remove folder"]');
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
    if (url.pathname === "/api/libraries/active") {
      return jsonResponse({ library: ACTIVE_LIBRARY, contextRevision: 1, runtimeGeneration: 1 });
    }
    if (method === "GET" && url.pathname === "/api/libraries") {
      return jsonResponse({ items: [ACTIVE_LIBRARY, INACTIVE_LIBRARY], contextRevision: 1 });
    }
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
    } as unknown as MangooDesktopBridge["notifications"],
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
    await requestNativeClick(await findLibraryAction('button[aria-label="Remove folder"]'));
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
  const iconTrigger = await findLibraryAction('button[aria-label="更改图标"]');
  await requestNativeClick(iconTrigger);
  const surface = await findOpenLayer("library-appearance-editor");
  if (!surface) throw new Error("Library icon menu did not open");
  await settle(50);
  const menus = surface.querySelectorAll('[role="menu"]');
  const radioItems = Array.from(surface.querySelectorAll<HTMLButtonElement>('[role="radio"]'));
  await act(async () => radioItems.find((item) => item.tabIndex === 0)?.focus());
  const focusedRadioItem = document.activeElement?.getAttribute("role") === "radio";
  const checkedItems = radioItems.filter((item) => item.getAttribute("aria-checked") === "true").length;
  const rovingTabStops = radioItems.filter((item) => item.tabIndex === 0).length;
  await requestNativeKey("ArrowRight");
  await requestNativeClick(await findElement(".library-appearance-apply", surface));
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
    } as unknown as MangooDesktopBridge["notifications"],
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');
  await waitFor(() => listRequests.length === 1, "initial first-run library request");
  listRequests[0].resolve([ACTIVE_LIBRARY, INACTIVE_LIBRARY]);
  await settle(20);
  listRequests.length = 0;
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

  await requestNativeClick(await findLibraryAction('button[aria-label="更改图标"]'));
  const iconSurface = await findOpenLayer("library-appearance-editor");
  if (!iconSurface) throw new Error("Icon menu did not open for ownership test");
  await requestNativeClick(await findElement(".library-icon-picker-cell", iconSurface));
  await requestNativeKey("Escape");
  await waitForLayerRemoval("library-appearance-editor");
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
    if (url.pathname === "/api/libraries/active") {
      return jsonResponse({ library: ACTIVE_LIBRARY, contextRevision: 1, runtimeGeneration: 1 });
    }
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
    } as unknown as MangooDesktopBridge["notifications"],
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
    } as unknown as MangooDesktopBridge["notifications"],
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
  await requestNativeKey("Escape");
  await settle(40);

  await act(async () => trigger.click());
  const addRoot = await findOpenSurface("library-root");
  if (!addRoot) throw new Error("Library root did not reopen for add failure");
  await requestNativeClick(await findLibraryAction('button[aria-label="Add folder"]'));
  await settle(40);

  await requestNativeClick(await findLibraryAction('button[aria-label="更改图标"]'));
  const iconSurface = await findOpenLayer("library-appearance-editor");
  if (!iconSurface) throw new Error("Icon menu did not open for icon failure");
  await requestNativeClick(await findElement(".library-icon-picker-cell", iconSurface));
  await requestNativeClick(await findElement(".library-appearance-apply", iconSurface));
  await settle(60);

  const result = { reports, unhandledRejections };
  window.removeEventListener("unhandledrejection", handleUnhandled);
  await unmountScenario(mounted);
  return result;
}

async function runAddFolderAfterBlurScenario() {
  const picker = deferred<string | null>();
  let addCalls = 0;
  let addedPath: string | null = null;
  let committedRootUuid: string | null = null;
  let commitCalls = 0;
  let enrollmentMode: string | null = null;
  let requestedName: string | null = null;
  let rootFolderUuid: string | null = null;
  let refreshCalls = 0;
  let listCalls = 0;
  const selectedPath = "C:\\Fixture\\selected-folder";
  const rootUuid = "11111111-1111-4111-8111-111111111111";
  const preflightToken = "22222222-2222-4222-8222-222222222222";
  installBridge({
    coreRequest: async (url, method, init) => {
      if (url.pathname === "/api/sync/status" && method === "GET") {
        return jsonResponse({ generatedAt: 1, libraries: [], providers: [], revision: 1, schedulerState: "running" });
      }
      if (url.pathname.endsWith("/sync-folders/enrollment/preflight") && method === "POST") {
        const body = JSON.parse(String(init?.body)) as { mode?: string };
        enrollmentMode = body.mode ?? null;
        return jsonResponse({
          expiresAt: Date.now() + 60_000,
          normalizedPath: selectedPath,
          ok: true,
          preflightToken,
          proposedMaterializedName: "selected-folder",
          reason: "ok",
          rootUuid,
        });
      }
      if (url.pathname.endsWith("/sync-folders/enrollment/commit") && method === "POST") {
        commitCalls += 1;
        const body = JSON.parse(String(init?.body)) as {
          requestedName?: string;
          rootFolderUuid?: string;
          rootUuid?: string;
        };
        committedRootUuid = body.rootUuid ?? null;
        requestedName = body.requestedName ?? null;
        rootFolderUuid = body.rootFolderUuid ?? null;
        return jsonResponse({
          errorCode: null,
          kind: "enroll",
          libraryUuid: ACTIVE_LIBRARY.libraryUuid,
          operationRevision: 1,
          operationUuid: "33333333-3333-4333-8333-333333333333",
          phase: "completed",
          rootUuid,
          state: "completed",
          updatedAt: 1,
        }, 202);
      }
      return null;
    },
    listLibraries: () => {
      listCalls += 1;
      return [{ ...ACTIVE_LIBRARY, bindingState: "synchronized" }, INACTIVE_LIBRARY];
    },
    pickFolder: () => picker.promise,
  });
  const library = createLibraryState({
    roots: [{ createdAt: 1, displayName: "selected-folder", enabled: true, id: 7, lastScanAt: 1, mediaCount: 1, fileCount: 1, path: selectedPath, reachable: true, rootFolderId: 1, icon: null, color: null }],
    addRoot: async (path) => {
      addCalls += 1;
      addedPath = path;
    },
    refresh: async () => {
      refreshCalls += 1;
    },
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={control} library={library} />
    </OverlayProvider>,
  );
  const trigger = await findElement('[data-titlebar-control="library-switcher"]');
  await act(async () => trigger.click());
  const surface = await findOpenSurface("library-root");
  if (!surface) throw new Error("Library root did not open for add-folder blur test");
  await requestNativeClick(await findLibraryAction('button[aria-label="Add folder"]'));
  await act(async () => window.dispatchEvent(new Event("blur")));
  await waitForLayerRemoval("library-root");
  const listCallsBeforeSelection = listCalls;
  await act(async () => picker.resolve(selectedPath));
  await waitFor(() => addCalls === 1, "selected folder to be added locally after window blur");
  await settle(30);

  const result = {
    addedPath,
    addCalls,
    committedRootUuid,
    commitCalls,
    enrollmentMode,
    enrolledExistingRoot: library.roots.some((root) => root.path === selectedPath),
    refreshCalls,
    reloadsAfterSelection: listCalls - listCallsBeforeSelection,
    requestedName,
    rootFolderUuidIsValid: rootFolderUuid !== null && /^[0-9a-f-]{36}$/u.test(rootFolderUuid),
    switcherClosed: document.getElementById("library-root") === null,
    localAddedPath: null as string | null,
    localAddCalls: 0,
    localEnrollmentCalls: 0,
    localReloadsAfterSelection: 0,
    switchedLibraryAddCalls: 0,
    switchedLibraryEnrollmentCalls: 0,
    switchedLibraryReports: 0,
  };
  await unmountScenario(mounted);

  const localPicker = deferred<string | null>();
  const localSelectedPath = "C:\\Fixture\\local-folder";
  let localListCalls = 0;
  installBridge({
    coreRequest: (url, method) => {
      if (url.pathname === "/api/sync/status" && method === "GET") {
        return jsonResponse({ generatedAt: 1, libraries: [], providers: [], revision: 1, schedulerState: "running" });
      }
      if (url.pathname.includes("/sync-folders/enrollment/")) {
        result.localEnrollmentCalls += 1;
      }
      return null;
    },
    listLibraries: () => {
      localListCalls += 1;
      return [ACTIVE_LIBRARY, INACTIVE_LIBRARY];
    },
    pickFolder: () => localPicker.promise,
  });
  const localLibrary = createLibraryState({
    addRoot: async (path) => {
      result.localAddCalls += 1;
      result.localAddedPath = path;
    },
  });
  const localControl: ScenarioControl = { api: null };
  const localMounted = await mountScenario(
    <OverlayProvider>
      <ProductionLibrarySwitcherScenario control={localControl} library={localLibrary} />
    </OverlayProvider>,
  );
  const localTrigger = await findElement('[data-titlebar-control="library-switcher"]');
  await act(async () => localTrigger.click());
  const localSurface = await findOpenSurface("library-root");
  if (!localSurface) throw new Error("Library root did not open for local add-folder blur test");
  await requestNativeClick(await findLibraryAction('button[aria-label="Add folder"]'));
  await act(async () => window.dispatchEvent(new Event("blur")));
  await waitForLayerRemoval("library-root");
  const localListCallsBeforeSelection = localListCalls;
  await act(async () => localPicker.resolve(localSelectedPath));
  await waitFor(() => result.localAddCalls === 1, "local folder to be added after window blur");
  await settle(30);
  result.localReloadsAfterSelection = localListCalls - localListCallsBeforeSelection;
  await unmountScenario(localMounted);
  for (const synchronized of [false, true]) {
    const switchedPicker = deferred<string | null>();
    let switched = false;
    installBridge({
      coreRequest: (url, method) => {
        if (url.pathname === "/api/libraries/active" && method === "GET" && switched) {
          return jsonResponse({
            contextRevision: ACTIVE_LIBRARY.contextRevision + 1,
            library: { ...INACTIVE_LIBRARY, active: true },
          });
        }
        if (url.pathname === "/api/sync/status" && method === "GET") {
          return jsonResponse({ generatedAt: 1, libraries: [], providers: [], revision: 1, schedulerState: "running" });
        }
        if (url.pathname.includes("/sync-folders/enrollment/")) {
          result.switchedLibraryEnrollmentCalls += 1;
        }
        return null;
      },
      listLibraries: () => [
        { ...ACTIVE_LIBRARY, bindingState: synchronized ? "synchronized" : "local-only" },
        INACTIVE_LIBRARY,
      ],
      pickFolder: () => switchedPicker.promise,
      notifications: {
        reportError: async () => {
          result.switchedLibraryReports += 1;
          return true;
        },
      } as unknown as MangooDesktopBridge["notifications"],
    });
    const switchedControl: ScenarioControl = { api: null };
    const switchedMounted = await mountScenario(
      <OverlayProvider>
        <ProductionLibrarySwitcherScenario
          control={switchedControl}
          library={createLibraryState({ addRoot: async () => { result.switchedLibraryAddCalls += 1; } })}
        />
      </OverlayProvider>,
    );
    const switchedTrigger = await findElement('[data-titlebar-control="library-switcher"]');
    await act(async () => switchedTrigger.click());
    const switchedSurface = await findOpenSurface("library-root");
    if (!switchedSurface) throw new Error("Library root did not open for changed Library test");
    await requestNativeClick(await findLibraryAction('button[aria-label="Add folder"]'));
    await act(async () => window.dispatchEvent(new Event("blur")));
    await waitForLayerRemoval("library-root");
    const previousReports = result.switchedLibraryReports;
    switched = true;
    await act(async () => switchedPicker.resolve("C:\\Fixture\\different-library-folder"));
    await waitFor(() => result.switchedLibraryReports === previousReports + 1, "changed Library to reject the stale folder choice");
    await unmountScenario(switchedMounted);
  }
  return result;
}

function checkLibraryDialogPresentation(surface: HTMLElement, check: (value: unknown, label: string) => void, kind: "Open" | "Forget") {
  const header = surface.querySelector<HTMLElement>('.overlay-dialog-header')!;
  const body = surface.querySelector<HTMLElement>('.overlay-dialog-body')!;
  const actions = surface.querySelector<HTMLElement>('.overlay-dialog-actions')!;
  const headingStyle = getComputedStyle(header.querySelector('h2')!);
  check([header, body, actions].every((element) => parseFloat(getComputedStyle(element).paddingLeft) >= 16 && parseFloat(getComputedStyle(element).paddingRight) >= 16) &&
    headingStyle.fontSize === "20px" && headingStyle.lineHeight === "28px" && headingStyle.fontWeight === "600",
    `${kind} dialog shares padded creation typography and layout`);
  check(getComputedStyle(actions).display === "flex" && getComputedStyle(actions).justifyContent === "flex-end" &&
    [...actions.querySelectorAll('button')].every((button) => button.classList.contains('liquid-glass-button') &&
      getComputedStyle(button).borderTopStyle === "solid" && getComputedStyle(button).borderTopWidth === "1px" && button.getBoundingClientRect().height >= 30),
    `${kind} dialog keeps aligned shared glass actions with one thin boundary`);
  check(body.scrollWidth <= body.clientWidth + 1 && actions.getBoundingClientRect().bottom <= innerHeight && surface.getBoundingClientRect().top >= 0,
    `${kind} dialog body and actions stay within the viewport`);
  const input = body.querySelector('input');
  if (input) {
    const field = input.closest<HTMLElement>('.overlay-field-control')!;
    const style = getComputedStyle(input);
    const labelStyle = getComputedStyle(body.querySelector('label')!);
    check(getComputedStyle(field).display === "flex" && getComputedStyle(field).borderTopWidth === "1px" &&
      field.getBoundingClientRect().width >= body.clientWidth - parseFloat(getComputedStyle(body).paddingLeft) - parseFloat(getComputedStyle(body).paddingRight) - 2 &&
      style.borderTopWidth === "0px" && style.backgroundColor === "rgba(0, 0, 0, 0)" && style.appearance === "none" &&
      style.fontSize === "14px" && style.lineHeight === "20px" && labelStyle.fontSize === "14px" && labelStyle.fontWeight === "400",
      "Open dialog uses the shared full-width single-boundary field and regular body label");
  }
}

async function runForgetCountScenario(): Promise<string[]> {
  let active = true;
  let mediaReads = 0;
  const root = { createdAt: 1, displayName: "Pictures", enabled: true, id: 7, lastScanAt: null, mediaCount: 62, fileCount: 62, path: "C:/Fixture/Pictures", reachable: true, rootFolderId: null };
  installBridge({ coreRequest: (url, method) => {
    if (url.pathname === "/api/libraries/active") return jsonResponse({ library: active ? ACTIVE_LIBRARY : null, contextRevision: active ? 1 : 2, runtimeGeneration: 1 });
    if (method === "DELETE" && url.pathname.endsWith("/registration")) {
      if (url.pathname.includes(ACTIVE_LIBRARY.libraryUuid)) active = false;
      return jsonResponse({ library: active ? ACTIVE_LIBRARY : null, contextRevision: active ? 1 : 2 });
    }
    if (url.pathname === "/api/roots") return jsonResponse({ items: [root], nextCursor: null, totalCount: 1 });
    if (url.pathname === "/api/media") {
      mediaReads++;
      return jsonResponse({ items: [{ id: 61, name: "visible.jpg", ext: "jpg", rootId: 7, folderId: null, kind: "image", mtime: 1, size: 1, width: 10, height: 10 }], totalCount: 62, nextCursor: "next-page" });
    }
    return jsonResponse({ items: [], nextCursor: null, totalCount: 0 });
  } });
  const control: HardMediaCancellationControl = { library: null };
  const mounted = await mountScenario(<HardMediaCancellationScenario control={control} />);
  try {
    await waitFor(() => control.library?.mediaTotalCount === 62 && control.library.media.length === 1 && control.library.selectedMediaId === 61, "populated Library count");
    const before = control.library!;
    const readsBefore = mediaReads;
    await act(async () => { await forgetLibraryEntry(INACTIVE_LIBRARY.libraryUuid); });
    await settle(30);
    if (control.library!.mediaTotalCount !== 62 || control.library!.mediaSlots !== before.mediaSlots || control.library!.selectedMediaId !== 61 || mediaReads !== readsBefore) throw new Error("Inactive forget must preserve count, slots and selection without catalog reload");
    await act(async () => { await forgetLibraryEntry(ACTIVE_LIBRARY.libraryUuid); });
    await waitFor(() => !control.library!.loading && control.library!.roots.length === 0, "active forget empty catalog");
    const after = control.library!;
    if (after.mediaTotalCount !== 0 || after.media.length !== 0 || after.mediaSlots.size !== 0 || after.loadedMediaRanges.length !== 0 || after.selectedMediaId !== null || after.mediaHasMore) throw new Error("Active forget must clear count and all old pagination state without a replacement library");
    return ["Inactive forget preserves count, virtual slots and selection", "Active forget without replacement clears count and pagination state"];
  } finally { await unmountScenario(mounted); }
}

async function runFollowupScenario(): Promise<string[]> {
  const checks: string[] = [];
  const check = (condition: unknown, label: string) => { if (!condition) throw new Error(label); checks.push(label); };
  const initial = deferred<LibrarySummary[]>();
  let libraries = [ACTIVE_LIBRARY, { ...INACTIVE_LIBRARY, color: "#7cc4ff", icon: "camera" }, { ...INACTIVE_LIBRARY, libraryUuid: "missing", name: "Missing", databasePath: "C:/Fixture/missing", availability: "unavailable" as const }];
  let loading = true;
  let listFails = false;
  let patchFails = false;
  let pendingPatch: Deferred<Response> | null = null;
  let explorerFails = true;
  let openFails = true;
  let forgetFails = true;
  let picker = deferred<string | null>();
  let opening = deferred<Response>();
  let forgetting = deferred<Response>();
  const patches: Array<{ icon: string; color: string }> = [];
  const opens: string[] = [];
  const forgets: string[] = [];
  let switches = 0;
  let reports = 0;
  installBridge({
    listLibraries: () => { if (loading) return initial.promise; if (listFails) throw new Error("list unavailable"); return libraries; },
    pickFolder: () => picker.promise,
    shell: { openPath: async () => !explorerFails, revealPath: async () => true, copyText: async () => true, openPluginExternalLink: async () => ({ ok: true }) },
    notifications: { reportError: async () => { reports++; return true; } } as unknown as MangooDesktopBridge["notifications"],
    coreRequest: (url, method, init) => {
      if (method === "PATCH") {
        if (patchFails) return jsonResponse({ code: "update-failed", message: "Appearance unavailable" }, 500);
        const body = JSON.parse(String(init?.body));
        patches.push(body);
        libraries = libraries.map((item) => item.libraryUuid === ACTIVE_LIBRARY.libraryUuid ? { ...item, ...body } : item);
        if (pendingPatch) return pendingPatch.promise;
        return jsonResponse({ library: libraries[0] });
      }
      if (url.pathname.endsWith("/activate")) { switches++; return jsonResponse({ contextRevision: 2, library: libraries[1] }); }
      if (url.pathname === "/api/libraries/open") {
        opens.push(JSON.parse(String(init?.body)).databasePath);
        if (openFails) return jsonResponse({ code: "library_marker_not_found", error: "library-marker-not-found" }, 404);
        return opening.promise;
      }
      if (url.pathname.endsWith("/registration")) {
        forgets.push(url.pathname);
        if (forgetFails) return jsonResponse({ code: "stale_context_revision", error: "stale-context-revision" }, 409);
        return forgetting.promise;
      }
      return null;
    },
  });
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(<OverlayProvider><ProductionLibrarySwitcherScenario control={control} /></OverlayProvider>);
  const setInput = async (input: HTMLInputElement, value: string) => {
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await settle(20);
  };
  try {
    await openFromTitlebar();
    check(document.body.textContent?.includes("正在加载资源库"), "Loading is visible before registry response");
    loading = false;
    initial.resolve(libraries);
    await waitFor(() => document.querySelectorAll('.library-switcher-row').length === 3, "loaded rows");
    const main = await findElement('.library-switcher-row.active .library-switcher-main');
    const row = main.closest<HTMLElement>('.library-switcher-row')!;
    const rowMore = await findElement('.library-switcher-more', row);
    for (const target of [main, rowMore]) {
      const rect = target.getBoundingClientRect();
      await requestNativeInput({ type: "mouse-move", x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2) });
      await settle(30);
      check(row.getAttribute('data-interactive-pointer-target') === 'true'
        && row.style.getPropertyValue('--interactive-pointer-opacity') === '1'
        && row.querySelectorAll('[data-interactive-pointer-target]').length === 0,
      `Whole row owns pointer highlight over ${target === main ? 'name' : 'more'}`);
      const edge = getComputedStyle(row, '::after');
      check(edge.left === '0px' && edge.right === '0px' && edge.top === '0px' && edge.bottom === '0px',
        `Pointer edge spans entire row over ${target === main ? 'name' : 'more'}`);
    }
    await act(async () => main.focus());
    await requestNativeKey('Tab');
    await settle(200);
    check(document.activeElement === rowMore && rowMore.matches(':focus-visible'), 'Tab reaches independent more action');
    check(getComputedStyle(row).outlineWidth === '1px' && getComputedStyle(row).outlineOffset === '-1px'
      && getComputedStyle(main).outlineStyle === 'none' && getComputedStyle(rowMore).outlineStyle === 'none'
      && getComputedStyle(row).boxShadow === 'none', 'Keyboard and pointer share one row boundary');
    check(getComputedStyle(rowMore).color === getComputedStyle(main.querySelector('.library-switcher-name')!).color,
      'More keyboard focus emphasizes foreground');
    await requestNativeKey('Space');
    check(Boolean(await findOpenSurface(`library-actions-${ACTIVE_LIBRARY.libraryUuid}`)) && switches === 0,
      'Keyboard more opens menu without activating Library');
    await requestNativeKey('Escape');
    check(document.activeElement === rowMore, 'Menu Escape restores independent more focus');
    await act(async () => main.focus());
    check(getComputedStyle(main.querySelector('.library-switcher-name')!).textDecorationLine.includes('underline')
      && getComputedStyle(main).outlineStyle === 'none', 'Main keyboard focus emphasizes name within row boundary');
    const inactiveMore = await findElement('.library-switcher-row:not(.active) .library-switcher-more');
    await requestNativeClick(inactiveMore);
    check(Boolean(await findOpenSurface(`library-actions-${INACTIVE_LIBRARY.libraryUuid}`)) && switches === 0,
      'Inactive Library more never triggers activation');
    await requestNativeKey('Escape');
    check(getComputedStyle(main.querySelector('svg')!).color === "rgb(255, 255, 255)", "Unset appearance is pure white");
    check(getComputedStyle(document.querySelectorAll('.library-switcher-main svg')[2]).color === "rgb(124, 196, 255)", "Persisted blue remains blue");
    check(document.querySelectorAll<HTMLButtonElement>('.library-switcher-main')[2].disabled, "Unavailable activation is disabled");
    const nameStyle = getComputedStyle(main.querySelector('.library-switcher-name')!);
    const pathStyle = getComputedStyle(main.querySelector('.library-switcher-path')!);
    check(nameStyle.fontSize === "14px" && nameStyle.lineHeight === "20px" && nameStyle.fontWeight === "600" && pathStyle.fontSize === "12px" && pathStyle.lineHeight === "16px" && pathStyle.fontWeight === "400", "Readable row typography");
    const search = await findElement('input[aria-label="搜索资源库"]') as HTMLInputElement;
    await setInput(search, "BETA");
    check(document.querySelectorAll('.library-switcher-row').length === 1, "Search matches paths without case sensitivity");
    await setInput(search, "no-match");
    check(document.body.textContent?.includes("未找到匹配的资源库"), "No-results state is visible");
    await setInput(search, "");
    const more = await findElement('.library-switcher-row.active .library-switcher-more');
    await requestNativeClick(more);
    const actionsSurface = await findOpenSurface(`library-actions-${ACTIVE_LIBRARY.libraryUuid}`);
    if (!actionsSurface) throw new Error("Library actions menu missing");
    const actionItems = [...actionsSurface.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    const actionRects = actionItems.map((item) => item.getBoundingClientRect());
    check(actionRects.length >= 5 && actionRects.every((rect, index) =>
      rect.height >= 30 && Math.abs(rect.left - actionRects[0].left) < 1 &&
      Math.abs(rect.width - actionRects[0].width) < 1 &&
      (index === 0 || rect.top >= actionRects[index - 1].bottom)
    ), "Action menu places each full-width item on a separate readable row");
    check(actionItems.every((item) => {
      const style = getComputedStyle(item);
      return style.display === "flex" && style.backgroundColor === "rgba(0, 0, 0, 0)" &&
        style.borderTopWidth === "0px" && style.borderRightWidth === "0px" &&
        style.borderBottomWidth === "0px" && style.borderLeftWidth === "0px" && style.boxShadow === "none";
    }), "Action menu uses transparent shared controls without native button bevels");
    check(switches === 0, "More menu never activates a library");
    const appearance = await findLibraryAction('button[aria-label="更改图标"]');
    await requestNativeClick(appearance);
    const appearanceSurface = await findOpenLayer("library-appearance-editor");
    check((appearanceSurface?.querySelectorAll('.library-icon-picker-cell').length ?? 0) >= 64 && appearanceSurface?.querySelectorAll('.library-icon-picker-swatch').length === 11, "Expanded glyph catalog and all eleven colors remain selectable");
    check(appearanceSurface?.querySelectorAll('[role="radiogroup"]').length === 2, "Appearance has independent icon and color radio groups");
    const whiteSwatch = appearanceSurface?.querySelector<HTMLElement>('.library-icon-picker-swatch[data-color="#ffffff"][aria-checked="true"]');
    const selectionMark = whiteSwatch?.querySelector('svg');
    check(Boolean(whiteSwatch && selectionMark && getComputedStyle(whiteSwatch.querySelector('.library-icon-picker-color')!).backgroundColor === "rgb(255, 255, 255)" &&
      getComputedStyle(selectionMark).color === "rgb(24, 32, 43)" && getComputedStyle(selectionMark).visibility === "visible" &&
      getComputedStyle(selectionMark).display !== "none" && getComputedStyle(whiteSwatch).opacity === "1"),
    "Selected white swatch has a visible dark check instead of color-only selection");
    const swatchRect = whiteSwatch!.getBoundingClientRect();
    const markRect = selectionMark!.getBoundingClientRect();
    check(markRect.width >= 12 && markRect.height >= 12 &&
      Math.abs(markRect.left + markRect.width / 2 - swatchRect.left - swatchRect.width / 2) <= 1 &&
      Math.abs(markRect.top + markRect.height / 2 - swatchRect.top - swatchRect.height / 2) <= 1,
    "Selected swatch check is centered and visibly sized");
    const beforeChoices = patches.length;
    for (const button of appearanceSurface!.querySelectorAll<HTMLButtonElement>('[role="radio"]')) {
      button.scrollIntoView({ block: "nearest" });
      await settle(20);
      await requestNativeClick(button);
      check(button.getAttribute('aria-checked') === 'true' && patches.length === beforeChoices,
        `Draft choice ${button.dataset.icon ?? button.dataset.color} selects without a request`);
    }
    check(getComputedStyle(appearanceSurface!.querySelector('.library-appearance-preview svg')!).color === "rgb(229, 231, 235)", "Live preview follows the final rapid draft choice");
    await requestNativeKey("Escape");
    await waitForLayerRemoval("library-appearance-editor");
    check(document.activeElement?.classList.contains('library-switcher-more'), "Appearance Escape restores stable row focus");
    check(patches.length === beforeChoices, "Cancel discards all draft choices without a request");
    check(!actionsSurface.textContent?.includes('恢复默认'), 'Reset is absent from the more menu');
    await openFromTitlebar();
    await requestNativeClick(await findLibraryAction('button[aria-label="更改图标"]'));
    const resetSurface = await findOpenLayer("library-appearance-editor");
    check(Boolean(resetSurface?.textContent?.includes('恢复默认')), 'Reset belongs to the appearance editor');
    await requestNativeClick(await findElement('[data-icon="camera"]', resetSurface!));
    await requestNativeClick(await findElement('[data-color="#f06595"]', resetSurface!));
    patchFails = true;
    await requestNativeClick(await findElement('.library-appearance-apply', resetSurface!));
    await waitFor(() => document.body.textContent?.includes("Appearance unavailable") ?? false, "appearance error");
    check(resetSurface?.querySelector('[data-icon="camera"][aria-checked="true"]') && resetSurface.querySelector('[data-color="#f06595"][aria-checked="true"]'), "Failed apply preserves both draft choices");
    patchFails = false;
    const beforeReset = patches.length;
    await requestNativeClick(await findButton("恢复默认", resetSurface!));
    check(patches.length === beforeReset, "Reset changes only the draft");
    pendingPatch = deferred<Response>();
    const apply = await findElement('.library-appearance-apply', resetSurface!);
    await requestNativeClick(apply);
    await waitFor(() => patches.length === beforeReset + 1, "appearance apply");
    check([...resetSurface!.querySelectorAll<HTMLButtonElement>('button')].every((button) => button.disabled), 'Appearance save disables every choice and reset');
    await act(async () => { apply.click(); resetSurface!.querySelector('form')!.dispatchEvent(new Event('submit', {bubbles:true,cancelable:true})); });
    await requestNativeKey('Escape');
    check(patches.length === beforeReset + 1 && resetSurface!.getAttribute('aria-busy') === 'true', 'Busy apply prevents duplicate submissions and dismissal');
    await act(async () => window.dispatchEvent(new Event('blur')));
    await requestNativePoint(4, 4);
    check(control.api?.getContainingLayer(resetSurface!)?.id === 'library-appearance-editor' && Boolean(document.getElementById('library-root')), 'Busy child retains registered parent family across blur and outside pointer');
    pendingPatch.resolve(jsonResponse({ library: libraries[0] }));
    pendingPatch = null;
    await waitForLayerRemoval("library-appearance-editor");
    check(patches.at(-1)?.icon === "library" && patches.at(-1)?.color === "#ffffff", "Reset applies explicit default glyph and white together");
    await openFromTitlebar();
    await findLibraryAction('button[aria-label="更改图标"]');
    await requestNativeClick(await findButton("在文件资源管理器中打开"));
    await waitFor(() => document.body.textContent?.includes("无法打开此文件夹") ?? false, "Explorer failure");
    await settle(200);
    explorerFails = false;
    await requestNativeClick(await findButton("在文件资源管理器中打开"));
    await requestNativeKey("Escape");
    await waitForLayerRemoval(`library-actions-${ACTIVE_LIBRARY.libraryUuid}`);
    check(document.activeElement?.classList.contains("library-switcher-more"), "Menu Escape restores the row launcher");
    listFails = true;
    await requestNativeKey("Escape");
    await waitForLayerRemoval("library-root");
    await openFromTitlebar();
    await waitFor(() => document.body.textContent?.includes("list unavailable") ?? false, "registry error");
    listFails = false;
    await requestNativeClick(await findButton("重试"));
    await waitFor(() => !document.body.textContent?.includes("list unavailable"), "registry retry");
    check(document.querySelectorAll('.library-switcher-row').length === 3, "Registry failure preserves rows and can retry");
    await requestNativeClick(await findButton("打开其它资源库…"));
    const openModal = await findOpenLayer("library-open-dialog");
    if (!openModal) throw new Error("Open dialog missing");
    const path = openModal.querySelector('input')!;
    await act(async () => window.dispatchEvent(new Event("blur")));
    check(openModal.getAttribute("aria-busy") === "true", "Native picker keeps open dialog busy across blur");
    picker.resolve("C:\\Fixture\\Existing");
    await waitFor(() => openModal.textContent?.includes("此文件夹不是 Mangoo 资源库") ?? false, "open error");
    checkLibraryDialogPresentation(openModal, check, "Open");
    check(path.getAttribute("aria-invalid") === "true" && Boolean(path.getAttribute("aria-describedby")) && !openModal.textContent?.includes("status 404"), "Known Core errors give Chinese recovery guidance linked to the path field");
    check(opens.length === 1, "Desktop selection directly attempts safe open without duplicate confirmation");
    picker = deferred<string | null>();
    await requestNativeClick(await findButton("选择文件夹…", openModal));
    picker.resolve(null);
    await waitFor(() => openModal.getAttribute("aria-busy") !== "true", "cancelled picker");
    check(path.value === "C:\\Fixture\\Existing", "Cancelled picker preserves the selected path");
    check(path.value === "C:\\Fixture\\Existing", "Failed open retains the path for retry");
    openFails = false;
    const openButton = await findButton("打开资源库", openModal);
    await requestNativeClick(openButton);
    await act(async () => { openButton.click(); openModal.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })); });
    await requestNativeKey("Escape");
    check(opens.length === 2 && openButton.disabled && openModal.dataset.overlayState === "open", "Pending open blocks duplicate submit and dismissal");
    opening.resolve(jsonResponse({ library: ACTIVE_LIBRARY, activated: true }));
    await waitForLayerRemoval("library-open-dialog");
    check(opens.every((value) => value === "C:\\Fixture\\Existing"), "Open uses the selected managed-root path");
    await openFromTitlebar();
    await findLibraryAction('button[aria-label="更改图标"]');
    await requestNativeClick(await findButton("从列表移除…"));
    const forgetModal = await findOpenLayer("library-forget-confirm");
    if (!forgetModal) throw new Error("Forget confirmation missing");
    checkLibraryDialogPresentation(forgetModal, check, "Forget");
    check(forgetModal.textContent?.includes("文件和数据会保留") && forgetModal.textContent?.includes("当前资源库将关闭"), "Forget explains preservation and active context closing");
    await requestNativeClick(await findButton("从列表移除", forgetModal));
    await waitFor(() => forgetModal.textContent?.includes("活动资源库已改变") ?? false, "forget error");
    forgetFails = false;
    await requestNativeClick(await findButton("从列表移除", forgetModal));
    await requestNativeKey("Escape");
    check(forgetModal.dataset.overlayState === "open" && forgetModal.getAttribute("aria-busy") === "true", "Pending forget blocks Escape");
    libraries = libraries.filter((item) => !item.active);
    forgetting.resolve(jsonResponse({ library: null, contextRevision: 2 }));
    await waitForLayerRemoval("library-forget-confirm");
    await waitFor(() => document.querySelectorAll('.library-switcher-row').length === 2, "forgotten row removed");
    check(forgets.length === 2 && forgets.every((value) => value.endsWith("/registration")), "Forget retries only the safe registration endpoint");
    check(reports >= 4, "Failures retain notification reporting");
    return checks;
  } finally { await unmountScenario(mounted); }
}

async function runWhiteCreationScenario(): Promise<string[]> {
  const created: { payload: Record<string, unknown> | null } = { payload: null };
  installBridge({ listLibraries: () => [], pickFolder: async () => "C:/Fixture/new", resolveLibraryCreationPaths: async ({ name, databasePath }) => ({ databasePath: databasePath ?? `C:/Fixture/${name}/Database`, defaultContentPath: `C:/Fixture/${name}/Files` }), coreRequest: (url, method, init) => {
    if (url.pathname === "/api/libraries" && method === "POST") {
      created.payload = JSON.parse(String(init?.body));
      return jsonResponse({ activated: true, library: ACTIVE_LIBRARY });
    }
    return null;
  } });
  const mounted = await mountScenario(<OverlayProvider><ProductionLibrarySwitcherScenario control={{ api: null }} /></OverlayProvider>);
  try {
    const wizard = await findOpenLayer("managed-library-wizard");
    if (!wizard) throw new Error("Default create wizard missing");
    const input = wizard.querySelector('input')!;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Default white"); input.dispatchEvent(new Event("input", { bubbles: true })); });
    const databaseToggle = wizard.querySelector<HTMLButtonElement>('[aria-controls="managed-library-database-editor"]')!;
    const databaseEditor = wizard.querySelector<HTMLElement>('#managed-library-database-editor')!;
    if (databaseToggle.getAttribute("aria-expanded") !== "false" || !databaseEditor.hidden) throw new Error("Database customization must start collapsed");
    await requestNativeClick(databaseToggle);
    await waitFor(() => databaseToggle.getAttribute("aria-expanded") === "true" && !databaseEditor.hidden, "database customization revealed");
    await requestNativeClick(wizard.querySelector<HTMLButtonElement>('[aria-label="选择数据库目录"]')!);
    await waitFor(() => databaseEditor.querySelector<HTMLInputElement>('input')!.value === "C:/Fixture/new", "chosen database directory applied");
    await waitFor(() => !wizard.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled, "default create ready");
    await requestNativeClick(await findButton("创建并打开", wizard));
    await waitFor(() => created.payload !== null, "default create request");
    if (created.payload?.color !== "#ffffff" || created.payload?.icon !== "library") throw new Error("New creation must submit pure white and default glyph");
    return ["Database customization opens through its native pointer disclosure before choosing a directory", "New creation submits pure white and default glyph without appearance edits"];
  } finally { await unmountScenario(mounted); }
}

async function runEntryScenario(): Promise<string[]> {
  const checks: string[] = [];
  const check = (condition: unknown, label: string) => { if (!condition) throw new Error(label); checks.push(label); };
  let opens = 0;
  installBridge({ listLibraries: () => [], coreRequest: (url) => {
    if (url.pathname === "/api/libraries/open") { opens++; return jsonResponse({ activated: true, library: ACTIVE_LIBRARY }); }
    return null;
  } });
  const firstRun = await mountScenario(<OverlayProvider><ProductionLibrarySwitcherScenario control={{ api: null }} /></OverlayProvider>);
  try {
    const wizard = await findOpenLayer("managed-library-wizard");
    if (!wizard) throw new Error("First run wizard missing");
    await requestNativeClick(await findButton("打开已有资源库…", wizard));
    const modal = await findOpenLayer("library-open-dialog");
    if (!modal) throw new Error("First run open action is unreachable");
    check(modal.textContent?.includes("服务器上的资源库数据库目录") && !modal.textContent?.includes("选择文件夹…"), "Web first-run exposes manual server path without native picker");
    const input = modal.querySelector('input')!;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "/srv/existing"); input.dispatchEvent(new Event("input", { bubbles: true })); });
    await requestNativeClick(await findButton("打开资源库", modal));
    await waitForLayerRemoval("library-open-dialog");
    check(opens === 1 && !document.getElementById("managed-library-wizard"), "First run can safely open existing without being forced to create");
    await openFromTitlebar();
    check(document.body.textContent?.includes("还没有资源库"), "Empty list keeps create and open actions reachable");
  } finally { await unmountScenario(firstRun); }

  let picker = deferred<string | null>();
  opens = 0;
  installBridge({ pickFolder: () => picker.promise, coreRequest: (url) => {
    if (url.pathname === "/api/libraries/open") { opens++; return jsonResponse({ activated: true, library: ACTIVE_LIBRARY }); }
    return null;
  } });
  const native = await mountScenario(<OverlayProvider><ProductionLibrarySwitcherScenario control={{ api: null }} /></OverlayProvider>);
  const trigger = await openFromTitlebar();
  await requestNativeClick(await findButton("打开其它资源库…"));
  await findOpenLayer("library-open-dialog");
  await act(async () => window.dispatchEvent(new Event("blur")));
  picker.resolve(null);
  await waitForLayerRemoval("library-open-dialog");
  check(opens === 0 && document.activeElement === trigger, "Launcher picker cancel makes no API mutation and restores a reachable launcher");
  picker = deferred<string | null>();
  await openFromTitlebar();
  await requestNativeClick(await findButton("打开其它资源库…"));
  await findOpenLayer("library-open-dialog");
  await unmountScenario(native);
  picker.resolve("C:\\Fixture\\Late");
  await settle(40);
  check(opens === 0, "Picker response after unmount cannot open a Library");
  return checks;
}

function SettingsRegistryScenario() {
  const interfaceStyle = useInterfaceStyle();
  return <div className="settings-window-body" style={{ position: "fixed", inset: 0 }}><SettingsPanels gridPreferences={DEFAULT_LIBRARY_GRID_PREFERENCES} interfaceStyle={interfaceStyle} library={createLibraryState()} onGridPreferencesChange={() => {}} onPreviewPreferencesChange={() => {}} previewPreferences={DEFAULT_PREVIEW_PREFERENCES} /></div>;
}

async function runSettingsRegistryScenario(): Promise<string[]> {
  const checks: string[] = [];
  const check = (condition: unknown, label: string) => { if (!condition) throw new Error(label); checks.push(label); };
  let libraries = [ACTIVE_LIBRARY, INACTIVE_LIBRARY];
  let hold = false;
  let calls = 0;
  const pending: Deferred<LibrarySummary[]>[] = [];
  installBridge({ listLibraries: () => {
    calls += 1;
    if (!hold) return libraries;
    const request = deferred<LibrarySummary[]>();
    pending.push(request);
    return request.promise;
  } });
  const phases: LibraryLoadPhase[] = [];
  const unsubscribe = subscribeLibraryPhase((phase) => phases.push(phase));
  const publisher = new BroadcastChannel(`mangoo-library-context:${fixtureCoreUrl}`);
  let mounted: MountedScenario | null = await mountScenario(<OverlayProvider><SettingsRegistryScenario /></OverlayProvider>);
  try {
    await requestNativeClick(await findButton("资源库"));
    await waitFor(() => document.querySelectorAll('.settings-library-row').length === 2, "Settings library rows").catch((cause) => { throw new Error(`${String(cause)}: ${JSON.stringify({ calls, text: document.body.innerText, activeTab: document.querySelector('[role="tab"][aria-selected="true"]')?.textContent })}`); });
    await settle(30);
    const phasesBefore = phases.length;
    libraries = [ACTIVE_LIBRARY];
    publisher.postMessage("registry-settled");
    await waitFor(() => document.querySelectorAll('.settings-library-row').length === 1, "remote forgotten Settings row");
    check(document.querySelector('.settings-library-name')?.textContent?.includes(ACTIVE_LIBRARY.name), "Settings removes a remotely forgotten row");
    check(phases.length === phasesBefore, "Settings registry refresh does not publish media context phases");
    hold = true;
    publisher.postMessage("registry-settled");
    await waitFor(() => pending.length === 1, "first Settings invalidation request");
    publisher.postMessage("registry-settled");
    await waitFor(() => pending.length === 2, "second Settings invalidation request");
    pending[1].resolve([INACTIVE_LIBRARY]);
    await waitFor(() => document.querySelector('.settings-library-name')?.textContent?.includes(INACTIVE_LIBRARY.name) ?? false, "fresh Settings registry response");
    pending[0].resolve([ACTIVE_LIBRARY]);
    await settle(40);
    check(document.querySelector('.settings-library-name')?.textContent?.includes(INACTIVE_LIBRARY.name), "Settings ignores an older registry response");
    publisher.postMessage("registry-settled");
    await waitFor(() => pending.length === 3, "pending Settings request before unmount");
    await unmountScenario(mounted);
    mounted = null;
    pending[2].resolve([ACTIVE_LIBRARY]);
    const callsAtUnmount = calls;
    publisher.postMessage("registry-settled");
    await settle(40);
    check(calls === callsAtUnmount && !document.querySelector('.settings-library-row'), "Settings unsubscribes and retires pending registry results on unmount");
    return checks;
  } finally {
    if (mounted) await unmountScenario(mounted);
    unsubscribe();
    publisher.close();
  }
}

async function runShortcutOwnershipScenario(): Promise<string[]> {
  const checks: string[] = [];
  const check = (condition: unknown, label: string) => { if (!condition) throw new Error(label); checks.push(label); };
  let patches = 0;
  installBridge({ setLibraryIcon: async () => { patches++; } });
  const control: ScenarioControl = { api: null, sidebarToggles: 0 };
  const mounted = await mountScenario(<OverlayProvider><ProductionLibrarySwitcherScenario control={control} /></OverlayProvider>);
  try {
    await openFromTitlebar();
    const main = await findElement('.library-switcher-row.active .library-switcher-main');
    await act(async () => main.focus());
    await requestNativeKey('Tab');
    check(document.activeElement?.classList.contains('library-switcher-more') && control.sidebarToggles === 0,
      'Real global shortcuts leave root Tab navigation to the row');
    await requestNativeInput({ type: 'cdp-key', keyCode: 'Enter' });
    const menu = await findOpenSurface(`library-actions-${ACTIVE_LIBRARY.libraryUuid}`);
    check(Boolean(menu), 'Enter activates the independent more action');
    const launcher = await findElement('button[aria-label="更改图标"]');
    await act(async () => launcher.focus());
    await requestNativeKey('Tab');
    check(document.activeElement !== launcher && control.sidebarToggles === 0,
      'Real global shortcuts leave menu Tab navigation to the overlay');
    await act(async () => control.api?.closeRoot('library'));
    await waitForLayerRemoval('library-root');
    await openFromTitlebar();
    await findLibraryAction('button[aria-label="更改图标"]');
    await requestNativeClick(await findElement('button[aria-label="更改图标"]'));
    await findOpenLayer("library-appearance-editor");
    const choice = await findElement('.library-icon-picker button[role="radio"][tabindex="0"]');
    await act(async () => choice.focus());
    await requestNativeKey('Tab');
    check(document.activeElement !== choice && control.sidebarToggles === 0,
      'Real global shortcuts leave appearance Tab navigation to the overlay');
    await requestNativeKey('Escape');
    await waitForLayerRemoval('library-appearance-editor');
    await requestNativeKey('Escape');
    await waitForLayerRemoval('library-root');
    const trigger = await findElement('.shell-library-switcher-button');
    await act(async () => trigger.focus());
    await requestNativeKey('Tab');
    check(control.sidebarToggles === 1, 'Tab resumes the Library shortcut after popup closes');
    await act(async () => {
      const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
      event.preventDefault();
      trigger.dispatchEvent(event);
    });
    check(control.sidebarToggles === 1 && patches === 0,
      'Already handled keyboard events do not reach global shortcuts');
    return checks;
  } finally { await unmountScenario(mounted); }
}

async function runAppearanceDraftScenario(): Promise<string[]> {
  const checks: string[] = [];
  const setInput = async (input: HTMLInputElement, value: string) => {
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  };
  const check = (value: unknown, label: string) => { if (!value) throw new Error(label); checks.push(label); };
  let setOpen: (open: boolean) => void = () => {};
  let setSaved: (saved: { icon: string; color: string }) => void = () => {};
  let request = deferred<void>();
  let calls = 0;
  let closes = 0;
  const submitted: Array<{ icon: string; color: string }> = [];
  function DraftFixture() {
    const [open, updateOpen] = useState(true);
    const [saved, updateSaved] = useState({ icon: "unknown-saved-key", color: "#123456" });
    setOpen = updateOpen;
    setSaved = updateSaved;
    return <><button id="draft-anchor" onClick={() => updateOpen(true)}>外观</button><LibraryAppearanceEditor anchor={document.getElementById("draft-anchor") ?? { getBoundingClientRect: () => ({ left: 40, top: 40, right: 80, bottom: 70, width: 40, height: 30 }) }} {...saved} id="draft-editor" name="Draft Library" onApply={(icon, color) => { calls++; submitted.push({ icon, color }); return request.promise; }} onClose={() => { closes++; updateOpen(false); }} open={open} rootId="managed-library" /></>;
  }
  const mounted = await mountScenario(<OverlayProvider><DraftFixture /></OverlayProvider>);
  try {
    let surface = await findOpenLayer('draft-editor');
    if (!surface) throw new Error('Draft editor missing');
    check(Boolean(surface.querySelector('[data-color="#123456"][aria-checked="true"]')), 'Saved custom color is an accessible selected choice');
    const glyphChoice = surface.querySelector<HTMLElement>('.library-icon-picker-cell')!;
    check(getComputedStyle(glyphChoice.querySelector('svg')!).color === getComputedStyle(glyphChoice).color
      && getComputedStyle(glyphChoice.querySelector('svg')!).color !== 'rgb(18, 52, 86)', 'Dark saved color does not obscure neutral glyph choices');
    check(getComputedStyle(surface.querySelector('.library-icon-picker-check')!).color === 'rgb(255, 255, 255)', 'Dark custom swatch has a contrasting light check');
    check(surface.getAttribute('data-overlay-layer-kind') === 'root' && !surface.closest('.overlay-modal-backdrop'), 'Appearance uses a registered anchored popup without modal scrim');
    const grid = surface.querySelector<HTMLElement>('.library-icon-picker-grid')!;
    const choices = [...grid.querySelectorAll<HTMLButtonElement>('.library-icon-picker-cell')];
    check(choices.length === IDENTITY_ICONS.length + 1, 'Expanded catalog exposes every identity and preserves the unknown saved choice');
    const sections = [...grid.querySelectorAll<HTMLElement>('.library-icon-picker-group')];
    check(sections.map((section) => section.dataset.iconCategory).join(',') === [...IDENTITY_CATEGORIES.map((category) => category.key), 'current'].join(','), 'Visible type groups follow catalog order and retain a separate current group');
    check(IDENTITY_CATEGORIES.every((category, index) => {
      const section = sections[index];
      return section.querySelector('h3')?.textContent === category.label
        && [...section.querySelectorAll<HTMLElement>('[data-icon]')].map((button) => button.dataset.icon).join(',')
          === IDENTITY_ICONS.filter((icon) => icon.category === category.key).map((icon) => icon.key).join(',');
    }) && new Set(choices.map((button) => button.dataset.icon)).size === choices.length, 'Each visible heading contains its exact stable catalog members without duplicate choices');
    check(grid.querySelectorAll('[tabindex="0"]').length === 1 && grid.querySelectorAll('[role="radiogroup"]').length === 0
      && sections.every((section) => section.querySelector('h3')!.tabIndex === -1 && getComputedStyle(section).overflowY === 'visible'), 'Groups keep one radio Tab entry, noninteractive headings and one shared scroll area');
    await act(async () => choices[choices.length - 1].focus());
    check(document.activeElement === choices[choices.length - 1] && choices[choices.length - 1].getAttribute('aria-checked') === 'true'
      && grid.scrollTop > 0, 'Unknown saved choice is selected and keyboard reachable in its current group');
    const columns = () => choices.findIndex((button) => button.getBoundingClientRect().top > choices[0].getBoundingClientRect().top + 1);
    check(columns() === 11 && getComputedStyle(sections[0].querySelector('.library-icon-picker-group-grid')!).gap === '0px', 'Normal popup uses eleven compact columns without grid gaps');
    check(grid.clientHeight === 172, 'Grouped icons retain the fixed compact scroll viewport');
    check(choices.every((button) => button.getBoundingClientRect().width >= 28 && button.getBoundingClientRect().height === 28
      && button.querySelector('svg')!.getBoundingClientRect().width === 18), 'Picker has 18px glyphs inside pointer targets of at least 28px');
    for (const [width, expectedColumns] of [[248, 8], [312, 11]]) {
      // The scroll owner extends past the choices into the popup padding.
      // These cases constrain usable choice width, not the outer overlay rail.
      const gridStyle = getComputedStyle(grid);
      grid.style.width = `${width + parseFloat(gridStyle.paddingLeft) + parseFloat(gridStyle.paddingRight)}px`;
      await settle(20);
      check(columns() === expectedColumns && grid.scrollWidth === grid.clientWidth, `Picker responds to ${width}px available width without horizontal overflow`);
      await act(async () => choices[0].focus());
      await requestNativeKey('Down');
      check(document.activeElement === choices[expectedColumns] && choices[expectedColumns].getAttribute('aria-checked') === 'true', `ArrowDown follows the actual ${expectedColumns}-column row`);
      await requestNativeKey('Up');
      check(document.activeElement === choices[0], `ArrowUp returns along the actual ${expectedColumns}-column row`);
    }
    grid.style.removeProperty('width');
    const renderedRows = () => {
      const rows: HTMLButtonElement[][] = [];
      for (const button of grid.querySelectorAll<HTMLButtonElement>('.library-icon-picker-cell')) {
        const row = rows.at(-1);
        if (row && Math.abs(row[0].getBoundingClientRect().top - button.getBoundingClientRect().top) < button.getBoundingClientRect().height / 2) row.push(button);
        else rows.push([button]);
      }
      return rows;
    };
    for (const [width, zoom, expectedColumns] of [[312, 1, 11], [248, 1, 8], [248, 1.25, 8]]) {
      const gridStyle = getComputedStyle(grid);
      grid.style.width = `${width + parseFloat(gridStyle.paddingLeft) + parseFloat(gridStyle.paddingRight)}px`;
      grid.style.zoom = String(zoom);
      await settle(20);
      const rows = renderedRows();
      check(rows[0].length === expectedColumns, `Grouped rows retain ${expectedColumns} columns at ${width}px and ${zoom} zoom`);
      const partialIndex = rows.findIndex((row, index) => rows[index + 1] && row.length > rows[index + 1].length);
      check(partialIndex >= 0, `Catalog exercises a partial row at ${width}px and ${zoom} zoom`);
      const fullRow = rows[partialIndex];
      const partialRow = rows[partialIndex + 1];
      const previousRow = rows[partialIndex - 1];
      check(Boolean(previousRow), 'Partial-row fixture retains a preceding full row');
      await requestNativeClick(fullRow[fullRow.length - 1]);
      await requestNativeKey('Up');
      check(document.activeElement === previousRow[Math.min(fullRow.length, previousRow.length) - 1], `ArrowUp follows the previous row after clicking its last column at ${width}px and ${zoom} zoom`);
      await act(async () => fullRow[fullRow.length - 1].focus());
      await requestNativeKey('Down');
      check(document.activeElement === partialRow[partialRow.length - 1], `ArrowDown clamps to the nearest available partial-row column at ${width}px and ${zoom} zoom`);
      await requestNativeKey('Up');
      check(document.activeElement === fullRow[partialRow.length - 1], `ArrowUp preserves the actual partial-row column at ${width}px and ${zoom} zoom`);
      await requestNativeClick(partialRow[partialRow.length - 1]);
      await requestNativeKey('Up');
      check(document.activeElement === fullRow[partialRow.length - 1], `ArrowUp stays in the rendered column after pointer hover lifts a cell at ${width}px and ${zoom} zoom`);
      const boundary = rows.findIndex((row, index) => rows[index + 1] && row[0].closest('.library-icon-picker-group') !== rows[index + 1][0].closest('.library-icon-picker-group'));
      const before = rows[boundary];
      const after = rows[boundary + 1];
      await act(async () => before[before.length - 1].focus());
      await requestNativeKey('Down');
      check(document.activeElement === after[Math.min(before.length, after.length) - 1], `ArrowDown crosses a type heading at ${width}px and ${zoom} zoom`);
      await act(async () => before[before.length - 1].focus());
      await requestNativeKey('Right');
      check(document.activeElement === after[0], `ArrowRight follows DOM order across type groups at ${width}px and ${zoom} zoom`);
      await requestNativeKey('Left');
      check(document.activeElement === before[before.length - 1], `ArrowLeft reverses a type-group transition at ${width}px and ${zoom} zoom`);
      check(grid.scrollWidth === grid.clientWidth && grid.clientHeight === 172, `Grouped geometry remains bounded at ${width}px and ${zoom} zoom`);
    }
    grid.style.removeProperty('width');
    grid.style.removeProperty('zoom');
    await requestNativeKey('End');
    const newChoice = [...grid.querySelectorAll<HTMLButtonElement>('[data-icon]')].at(-1)!;
    check(document.activeElement === newChoice && newChoice.getAttribute('aria-checked') === 'true'
      && grid.scrollTop > 0 && newChoice.getBoundingClientRect().bottom <= grid.getBoundingClientRect().bottom + 1
      && surface.querySelector('.library-appearance-preview [data-identity-icon]')?.getAttribute('data-identity-icon') === newChoice.dataset.icon && calls === 0,
      'Keyboard reaches and reveals a new final-row glyph with draft preview only');
    await requestNativeKey('Right');
    check(document.activeElement === grid.querySelector('[data-icon]'), 'Horizontal navigation wraps from the last type to the first');
    await requestNativeKey('Left');
    check(document.activeElement === newChoice, 'Horizontal navigation wraps from the first type to the last');
    await requestNativeKey('Home');
    check(document.activeElement === grid.querySelector('[data-icon]'), 'Home reaches the first visible identity across all types');
    await act(async () => setOpen(false));
    await waitForLayerRemoval('draft-editor');
    await act(async () => setOpen(true));
    surface = await findOpenLayer('draft-editor');
    if (!surface) throw new Error('Draft editor did not reopen');
    check(Boolean(surface.querySelector('[data-icon="unknown-saved-key"][aria-checked="true"]')), 'Discarding expanded catalog edits restores the unknown saved identity');
    const search = surface.querySelector<HTMLInputElement>('input[aria-label="搜索图标"]')!;
    const checkMatches = (query: string, category = 'all') => {
      const expectedGroups = IDENTITY_CATEGORIES.filter((entry) => category === 'all' || entry.key === category)
        .map((entry) => ({ ...entry, icons: IDENTITY_ICONS.filter((icon) => icon.category === entry.key && matchesIdentityIcon(icon, query)) }))
        .filter((entry) => entry.icons.length > 0);
      const groups = [...surface.querySelectorAll<HTMLElement>('.library-icon-picker-group')];
      check(groups.length === expectedGroups.length && groups.every((group, index) => group.dataset.iconCategory === expectedGroups[index].key
        && group.querySelector('h3')?.textContent === expectedGroups[index].label
        && [...group.querySelectorAll<HTMLElement>('[data-icon]')].map((button) => button.dataset.icon).join(',') === expectedGroups[index].icons.map((icon) => icon.key).join(',')), `Search ${query} in ${category} preserves exact grouped membership and omits empty types`);
      check(surface.querySelectorAll('.library-icon-picker-cell[tabindex="0"]').length === (expectedGroups.length ? 1 : 0), `Search ${query} in ${category} keeps a valid single radio entry`);
    };
    await setInput(search, 'ROBOT');
    checkMatches('ROBOT');
    check(Boolean(surface.querySelector('[data-icon="robot"]')), 'English search reaches a newly bundled identity');
    await setInput(search, '露营');
    checkMatches('露营');
    check(Boolean(surface.querySelector('[data-icon="tent"]')), 'Chinese synonym search reaches a newly bundled identity');
    await setInput(search, 'camera');
    checkMatches('camera');
    check(surface.querySelectorAll('.library-icon-picker-cell').length > 0 && surface.querySelectorAll('.library-icon-picker-cell').length < 64, 'Search narrows the expanded icon catalog');
    await setInput(search, 'a');
    checkMatches('a');
    check(surface.querySelectorAll('.library-icon-picker-group').length > 1, 'Search results show multiple matching types together');
    await setInput(search, 'no-such-identity-match');
    checkMatches('no-such-identity-match');
    check(surface.querySelector('[role="status"]')?.textContent === '未找到图标', 'Empty search shows a status without empty headings');
    await setInput(search, '');
    const category = surface.querySelector<HTMLSelectElement>('select[aria-label="图标分类"]')!;
    await act(async () => { category.value = category.options[1].value; category.dispatchEvent(new Event('change', { bubbles: true })); });
    checkMatches('', category.value);
    check(surface.querySelectorAll('.library-icon-picker-group').length === 1, 'Category filters to one visible type heading');
    await setInput(search, 'book');
    checkMatches('book', category.value);
    await setInput(search, '');
    await act(async () => { category.value = 'all'; category.dispatchEvent(new Event('change', { bubbles: true })); });
    const hex = surface.querySelector<HTMLInputElement>('input[aria-label="HEX 颜色"]')!;
    await setInput(hex, '#abc');
    check(surface.querySelector('[data-color="#abc"][aria-checked="true"]') && calls === 0, 'Valid custom HEX updates only the draft');
    await setInput(hex, '#ab');
    check(hex.value === '#ab' && hex.getAttribute('aria-invalid') === 'true' && surface.querySelector<HTMLButtonElement>('.library-appearance-apply')!.disabled, 'Partial custom HEX remains editable and disables Apply');
    await act(async () => surface.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    check(calls === 0, 'Invalid HEX cannot submit the previous valid color');
    await requestNativeClick(await findElement('[data-color="#f06595"]', surface));
    await requestNativeClick(await findElement('.library-appearance-apply', surface));
    check(submitted[0]?.icon === 'unknown-saved-key' && submitted[0]?.color === '#f06595', 'Changing color preserves an unknown saved icon key');
    request.reject(new Error('retry this draft'));
    await waitFor(() => Boolean(surface.querySelector('[role="alert"]')), 'draft failure');
    await requestNativeClick(await findElement('[data-icon="camera"]', surface));
    await act(async () => setSaved({ icon: 'library', color: '#ffffff' }));
    check(Boolean(surface.querySelector('[data-icon="camera"][aria-checked="true"]') && surface.querySelector('[data-color="#f06595"][aria-checked="true"]')), 'Registry refresh cannot overwrite the open draft');
    request = deferred<void>();
    await requestNativeClick(await findElement('.library-appearance-apply', surface));
    await act(async () => setOpen(false));
    await waitForLayerRemoval('draft-editor');
    await act(async () => setOpen(true));
    const reopened = await findOpenLayer('draft-editor');
    if (!reopened) throw new Error('Fresh editor missing');
    request.resolve();
    await settle(60);
    check(closes === 0 && Boolean(document.getElementById('draft-editor')), 'An old save cannot close a newly opened editor');
    check(Boolean(reopened.querySelector('[data-icon="library"][aria-checked="true"]')), 'Reopening starts from saved metadata');
    check(getComputedStyle(reopened.querySelector('.library-icon-picker-check')!).color === 'rgb(24, 32, 43)', 'White swatch has a contrasting dark check');
    request = deferred<void>();
    await requestNativeClick(await findElement('[data-icon="camera"]', reopened));
    await requestNativeClick(await findElement('.library-appearance-apply', reopened));
    await unmountScenario(mounted);
    request.reject(new Error('late save failure'));
    await delay(80);
    check(calls === 3 && closes === 0, `Unmount retires completion callbacks without duplicate writes (calls=${calls}, closes=${closes})`);
    return checks;
  } finally { if (mounted.container.isConnected) await unmountScenario(mounted); }
}

async function run(): Promise<void> {
  try {
    if (new URLSearchParams(location.search).has("followup")) {
      lifecycleWindow.__overlayLifecycleResult = { followup: [...await runAppearanceDraftScenario(), ...await runShortcutOwnershipScenario(), ...await runFollowupScenario(), ...await runEntryScenario(), ...await runWhiteCreationScenario(), ...await runForgetCountScenario(), ...await runSettingsRegistryScenario()], ok: true };
      return;
    }
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
      addFolderAfterBlur: await runAddFolderAfterBlurScenario(),
      managedLibraryWizard: await runManagedLibraryWizardScenario(),
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

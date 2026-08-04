import {
  act,
  useLayoutEffect,
  useRef,
  useState,
  type ComponentType,
  type ReactElement
} from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  AnchoredOverlay,
  ModalSurface,
  OverlayMenuItem,
  OverlayMenuList,
  OverlayProvider,
  useOverlayStack,
  type OverlayLayerRegistration,
  type OverlayStackApi
} from "../../src/design/overlays";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import * as ShellOverlayHostModule from "../../src/app-shell/ShellOverlayHost";
import { ShellRightActions } from "../../src/app-shell/ShellTopBar";
import { FilterMenu } from "../../src/features/library/FilterMenu";
import { LayoutMenu } from "../../src/features/library/LayoutMenu";
import type { LibraryLayoutMode } from "../../src/features/media-grid/layoutMode";
import { SearchBar } from "../../src/features/library/SearchBar";
import { SortMenu, type SortOption } from "../../src/features/library/SortMenu";
import type { CompactNotificationModel } from "../../src/features/notifications/notificationModel";
import "../../src/design/overlays/overlay.css";

declare global {
  interface Window {
    __overlayLifecycleInputRequest?: NativeInputRequest;
    __overlayLifecycleInputResponseId?: number;
    __overlayLifecycleResult?: OverlayLifecycleResult;
  }
}

type NativeInputPayload = {
  type: "key";
  keyCode: string;
  modifiers?: string[];
} | {
  type: "mouse-click" | "mouse-move";
  x: number;
  y: number;
};
type NativeInputRequest = NativeInputPayload & { id: number };

interface OverlayLifecycleResult {
  ok: boolean;
  error?: string;
  basicFocusWorks?: boolean;
  reentrancy?: {
    closeDismissals: number;
    closeRootId: string | null;
    dismissTopDismissals: number;
    dismissTopRootId: string | null;
    switchDismissals: number;
    switchRootId: string | null;
  };
  autofocus?: {
    modalFocused: boolean;
    modalDismissals: number;
    modalRootDismissals: number;
    modalRootId: string | null;
    nestedFocused: boolean;
    nestedDismissals: number;
    nestedRootDismissals: number;
    nestedRootId: string | null;
  };
  controlledFocus?: {
    controlledFocusCount: number;
    controlledItemFocusedBefore: boolean;
    controlledRootIdBefore: string | null;
    providerFocusCount: number;
    providerDismissals: number;
    providerItemFocusedBefore: boolean;
    providerRootIdBefore: string | null;
  };
  controlledFamily?: {
    activeRootAfterFirstClose: string | null;
    activeRootAfterReopen: string | null;
    activeRootAfterSecondClose: string | null;
    nestedDismissalsAfterFirstClose: number;
    nestedDismissalsAfterSecondClose: number;
    rootDismissals: number;
    rootFocusesAfterFirstClose: number;
    rootFocusesAfterSecondClose: number;
  };
  modalFamilyFocus?: {
    activeRootAfterClose: string | null;
    focusAfterExit: string;
    focusDuringClose: string;
    modalDismissals: number;
    modalFocusedBefore: boolean;
    rootDismissals: number;
  };
  deferredAutofocus?: {
    asyncItemFocused: boolean;
    initialItemFocused: boolean;
    reopenItemFocused: boolean;
    triggerFocusedDuringClose: boolean;
    userFocusPreserved: boolean;
  };
  portalDescendantAutofocus?: {
    modalFocusPreserved: boolean;
    modalFocusedBefore: boolean;
    nestedFocusPreserved: boolean;
    nestedFocusedBefore: boolean;
  };
  scrollShell?: {
    anchorMeasuresAfterScroll: number;
    anchorMeasuresBeforeScroll: number;
    hasScrollViewport: boolean;
    shellOverflowY: string;
  };
  dynamicMenu?: {
    activeId: string;
    ariaDisabledClicks: number;
    enabledTabStops: number;
    enabledClicks: number;
    disabledTabIndex: number;
    enterFallbackClicks: number;
    pointerDownPrevented: boolean;
    pointerFocusedDisabled: boolean;
  };
  notificationRoot?: {
    activeRootAfterSwitch: string | null;
    blurClosed: boolean;
    escapeClosed: boolean;
    escapeFocus: string;
    keyboardAutoFocused: boolean;
    keyboardActiveRoot: string | null;
    keyboardOpened: boolean;
    keyboardTriggerFocused: boolean;
    mouseOpenFocus: string;
    outsidePointerClosed: boolean;
    productionWiringMounted: boolean;
    switchClosed: boolean;
    tabEnteredContent: boolean;
    tabKeptOpen: boolean;
  };
  titlebarControls?: {
    clearFocusedInput: boolean;
    clearPointerTarget: string | null;
    clearSynchronous: boolean;
    filterCompositeRoles: number;
    filterControls: string | null;
    filterSurfaceRole: string | null;
    inputPointerGlow: string | null;
    layoutCompositeRoles: number;
    layoutControls: string | null;
    layoutSelectedStates: Array<string | null>;
    layoutSurfaceRole: string | null;
    searchValueAfterInput: string;
    sortCompositeRoles: number;
    sortControls: string | null;
    sortSelectedStates: Array<string | null>;
    sortSurfaceRole: string | null;
  };
}

interface MountedScenario {
  root: Root;
  container: HTMLDivElement;
}

interface ScenarioControl {
  api: OverlayStackApi | null;
  addItems?: () => void;
  closeControlled?: () => void;
  disableSelectedItem?: () => void;
  enableRootTrigger?: () => void;
  openModal?: () => void;
  openNested?: () => void;
  openRoot?: () => void;
}

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const rect = (left: number, top: number, width: number, height: number) => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height
});

const delay = (milliseconds = 0) => new Promise<void>((resolve) => {
  window.setTimeout(resolve, milliseconds);
});

async function settle(milliseconds = 0): Promise<void> {
  await act(async () => {
    await delay(milliseconds);
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await Promise.resolve();
  });
}

let nativeInputRequestId = 0;

async function requestNativeInput(request: NativeInputPayload): Promise<void> {
  const id = ++nativeInputRequestId;
  window.__overlayLifecycleInputRequest = { ...request, id } as NativeInputRequest;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (window.__overlayLifecycleInputResponseId === id) return;
    await delay(10);
  }
  throw new Error(`Timed out waiting for native input response ${id}`);
}

const requestNativeKey = (keyCode: string) => requestNativeInput({ type: "key", keyCode });

const requestNativePointer = (type: "mouse-click" | "mouse-move", element: HTMLElement) => {
  const bounds = element.getBoundingClientRect();
  return requestNativeInput({
    type,
    x: Math.round(bounds.left + bounds.width / 2),
    y: Math.round(bounds.top + bounds.height / 2)
  });
};

async function mountScenario(element: ReactElement): Promise<MountedScenario> {
  const container = document.createElement("div");
  container.dataset.overlayLifecycleRoot = "true";
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(element);
  });
  await settle(20);
  return { root, container };
}

async function unmountScenario({ root, container }: MountedScenario): Promise<void> {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  document.querySelectorAll("[data-overlay-layer-kind]").forEach((node) => node.remove());
  await settle();
}

async function waitForSelector(selector: string): Promise<HTMLElement> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const element = document.querySelector<HTMLElement>(selector);
    if (element) return element;
    await settle(10);
  }
  throw new Error(`Timed out waiting for ${selector}`);
}

async function waitForOpenSurface(id: string): Promise<HTMLElement> {
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
  throw new Error(`Timed out waiting for open overlay surface #${id}`);
}

async function waitForFocus(id: string): Promise<boolean> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (document.activeElement?.id === id) return true;
    await settle(10);
  }
  return false;
}

function StackCapture({ control }: { control: ScenarioControl }) {
  const api = useOverlayStack();
  useLayoutEffect(() => {
    control.api = api;
  }, [api, control]);
  return null;
}

const manualLayer = (
  id: string,
  onDismiss: () => void,
  trigger: HTMLElement,
  surface: HTMLElement
): OverlayLayerRegistration => ({
  id,
  rootId: "filter",
  kind: "root",
  trigger,
  surface,
  dismissOnEscape: true,
  dismissOnFocusOutside: true,
  dismissOnOutsidePointer: true,
  dismissOnWindowBlur: true,
  restoreFocus: true,
  onDismiss
});

async function withProviderApi<T>(
  run: (readApi: () => OverlayStackApi, trigger: HTMLElement, surface: HTMLElement) => Promise<T>
): Promise<T> {
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider><StackCapture control={control} /></OverlayProvider>
  );
  const trigger = document.createElement("button");
  const surface = document.createElement("div");
  document.body.append(trigger, surface);
  try {
    return await run(() => {
      if (!control.api) throw new Error("overlay stack API was not captured");
      return control.api;
    }, trigger, surface);
  } finally {
    trigger.remove();
    surface.remove();
    await unmountScenario(mounted);
  }
}

async function runReentrancyScenarios() {
  const close = await withProviderApi(async (readApi, trigger, surface) => {
    let dismissals = 0;
    await act(async () => {
      readApi().openRoot("filter");
      readApi().registerLayer(manualLayer("close-root", () => {
        dismissals += 1;
        if (dismissals === 1) readApi().closeRoot("filter");
      }, trigger, surface));
      readApi().closeRoot("filter");
    });
    await settle();
    return { dismissals, rootId: readApi().activeRootId };
  });

  const rootSwitch = await withProviderApi(async (readApi, trigger, surface) => {
    let dismissals = 0;
    await act(async () => {
      readApi().openRoot("filter");
      readApi().registerLayer(manualLayer("switch-root", () => {
        dismissals += 1;
        if (dismissals === 1) readApi().openRoot("notifications");
      }, trigger, surface));
      readApi().openRoot("library");
    });
    await settle();
    return { dismissals, rootId: readApi().activeRootId };
  });

  const dismissTop = await withProviderApi(async (readApi, trigger, surface) => {
    let dismissals = 0;
    await act(async () => {
      readApi().openRoot("filter");
      readApi().registerLayer(manualLayer("dismiss-top", () => {
        dismissals += 1;
        if (dismissals === 1) readApi().openRoot("notifications");
      }, trigger, surface));
      readApi().dismissTop();
    });
    await settle();
    return { dismissals, rootId: readApi().activeRootId };
  });

  return {
    closeDismissals: close.dismissals,
    closeRootId: close.rootId,
    dismissTopDismissals: dismissTop.dismissals,
    dismissTopRootId: dismissTop.rootId,
    switchDismissals: rootSwitch.dismissals,
    switchRootId: rootSwitch.rootId
  };
}

function NestedAutoFocusScenario({ control, metrics }: {
  control: ScenarioControl;
  metrics: { nestedDismissals: number; rootDismissals: number };
}) {
  const [rootAnchor, setRootAnchor] = useState<HTMLButtonElement | null>(null);
  const [nestedAnchor, setNestedAnchor] = useState<HTMLButtonElement | null>(null);
  const [rootOpen, setRootOpen] = useState(false);
  const [nestedOpen, setNestedOpen] = useState(false);
  control.openNested = () => setNestedOpen(true);
  control.openRoot = () => setRootOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button id="nested-root-trigger" ref={setRootAnchor}>Root</button>
      <AnchoredOverlay
        anchor={rootAnchor}
        ariaLabel="Root menu"
        id="nested-root-surface"
        onDismiss={() => {
          metrics.rootDismissals += 1;
          setRootOpen(false);
        }}
        open={rootOpen}
        rootId="library"
      >
        <button id="nested-menu-trigger" ref={setNestedAnchor}>Nested</button>
        <AnchoredOverlay
          anchor={nestedAnchor}
          ariaLabel="Nested menu"
          id="nested-menu-surface"
          onDismiss={() => {
            metrics.nestedDismissals += 1;
            setNestedOpen(false);
          }}
          open={nestedOpen}
          parentId="nested-root-surface"
          rootId="library"
          surfaceRole="nested"
        >
          <OverlayMenuList ariaLabel="Nested choices">
            <OverlayMenuItem id="nested-autofocus-item">Nested item</OverlayMenuItem>
          </OverlayMenuList>
        </AnchoredOverlay>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

function ModalAutoFocusScenario({ control, metrics }: {
  control: ScenarioControl;
  metrics: { modalDismissals: number; rootDismissals: number };
}) {
  const [rootAnchor, setRootAnchor] = useState<HTMLButtonElement | null>(null);
  const [rootOpen, setRootOpen] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  const modalTriggerRef = useRef<HTMLButtonElement | null>(null);
  const modalInitialFocusRef = useRef<HTMLButtonElement | null>(null);
  control.openModal = () => setModalOpen(true);
  control.openRoot = () => setRootOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button id="modal-root-trigger" ref={setRootAnchor}>Root</button>
      <AnchoredOverlay
        anchor={rootAnchor}
        ariaLabel="Modal owner"
        id="modal-root-surface"
        onDismiss={() => {
          metrics.rootDismissals += 1;
          setRootOpen(false);
        }}
        open={rootOpen}
        rootId="library"
      >
        <button id="modal-open-trigger" ref={modalTriggerRef}>Open modal</button>
        <ModalSurface
          ariaLabelledBy="modal-title"
          id="autofocus-modal"
          initialFocusRef={modalInitialFocusRef}
          onDismiss={() => {
            metrics.modalDismissals += 1;
            setModalOpen(false);
          }}
          open={modalOpen}
          restoreFocusRef={modalTriggerRef}
          rootId="library"
        >
          <h2 id="modal-title">Modal</h2>
          <button id="modal-autofocus-item" ref={modalInitialFocusRef}>Confirm</button>
        </ModalSurface>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runAutoFocusScenarios() {
  const nestedControl: ScenarioControl = { api: null };
  const nestedMetrics = { nestedDismissals: 0, rootDismissals: 0 };
  const nestedMounted = await mountScenario(
    <NestedAutoFocusScenario control={nestedControl} metrics={nestedMetrics} />
  );
  await waitForSelector("#nested-root-trigger");
  await act(async () => nestedControl.openRoot?.());
  await waitForSelector("#nested-menu-trigger");
  await settle(20);
  await act(async () => nestedControl.openNested?.());
  await waitForSelector("#nested-autofocus-item");
  const nestedFocused = await waitForFocus("nested-autofocus-item");
  const nestedRootId = nestedControl.api?.activeRootId ?? null;
  await act(async () => nestedControl.api?.dismissTop());
  await settle();
  await unmountScenario(nestedMounted);

  const modalControl: ScenarioControl = { api: null };
  const modalMetrics = { modalDismissals: 0, rootDismissals: 0 };
  const modalMounted = await mountScenario(
    <ModalAutoFocusScenario control={modalControl} metrics={modalMetrics} />
  );
  await waitForSelector("#modal-root-trigger");
  await act(async () => modalControl.openRoot?.());
  await waitForSelector("#modal-open-trigger");
  await settle(20);
  await act(async () => modalControl.openModal?.());
  await waitForSelector("#modal-autofocus-item");
  const modalFocused = await waitForFocus("modal-autofocus-item");
  const modalRootId = modalControl.api?.activeRootId ?? null;
  await act(async () => modalControl.api?.dismissTop());
  await settle();
  await unmountScenario(modalMounted);

  return {
    modalFocused,
    modalDismissals: modalMetrics.modalDismissals,
    modalRootDismissals: modalMetrics.rootDismissals,
    modalRootId,
    nestedFocused,
    nestedDismissals: nestedMetrics.nestedDismissals,
    nestedRootDismissals: nestedMetrics.rootDismissals,
    nestedRootId
  };
}

function FocusRestoreScenario({ control, metrics }: {
  control: ScenarioControl;
  metrics: { dismissals: number; triggerFocuses: number };
}) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(false);
  control.closeControlled = () => setOpen(false);
  control.openRoot = () => setOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button
        id="focus-restore-trigger"
        onFocus={() => {
          metrics.triggerFocuses += 1;
        }}
        ref={setAnchor}
      >
        Trigger
      </button>
      <AnchoredOverlay
        anchor={anchor}
        ariaLabel="Focus restore"
        id="focus-restore-surface"
        onDismiss={() => {
          metrics.dismissals += 1;
          setOpen(false);
        }}
        open={open}
        rootId="library"
      >
        <OverlayMenuList ariaLabel="Focus choices">
          <OverlayMenuItem id="focus-restore-item">Item</OverlayMenuItem>
        </OverlayMenuList>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runFocusRestoreScenario(controlled: boolean) {
  const control: ScenarioControl = { api: null };
  const metrics = { dismissals: 0, triggerFocuses: 0 };
  const mounted = await mountScenario(
    <FocusRestoreScenario control={control} metrics={metrics} />
  );
  await waitForSelector("#focus-restore-trigger");
  await act(async () => control.openRoot?.());
  await waitForSelector("#focus-restore-item");
  const itemFocusedBefore = await waitForFocus("focus-restore-item");
  const rootIdBefore = control.api?.activeRootId ?? null;
  await act(async () => {
    if (controlled) control.closeControlled?.();
    else control.api?.dismissTop();
  });
  await settle(30);
  const result = { ...metrics };
  await unmountScenario(mounted);
  return { ...result, itemFocusedBefore, rootIdBefore };
}

async function runControlledFocusScenarios() {
  const controlled = await runFocusRestoreScenario(true);
  const provider = await runFocusRestoreScenario(false);
  return {
    controlledFocusCount: controlled.triggerFocuses,
    controlledItemFocusedBefore: controlled.itemFocusedBefore,
    controlledRootIdBefore: controlled.rootIdBefore,
    providerFocusCount: provider.triggerFocuses,
    providerDismissals: provider.dismissals,
    providerItemFocusedBefore: provider.itemFocusedBefore,
    providerRootIdBefore: provider.rootIdBefore
  };
}

function ControlledFamilyScenario({ control, metrics }: {
  control: ScenarioControl;
  metrics: { nestedDismissals: number; rootDismissals: number; rootFocuses: number };
}) {
  const [rootAnchor, setRootAnchor] = useState<HTMLButtonElement | null>(null);
  const [nestedAnchor, setNestedAnchor] = useState<HTMLButtonElement | null>(null);
  const [rootOpen, setRootOpen] = useState(false);
  const [nestedOpen, setNestedOpen] = useState(false);
  const [rootTriggerDisabled, setRootTriggerDisabled] = useState(true);
  control.closeControlled = () => setRootOpen(false);
  control.enableRootTrigger = () => setRootTriggerDisabled(false);
  control.openNested = () => setNestedOpen(true);
  control.openRoot = () => setRootOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button
        disabled={rootTriggerDisabled}
        id="controlled-family-trigger"
        onFocus={() => {
          metrics.rootFocuses += 1;
        }}
        ref={setRootAnchor}
      >
        Root
      </button>
      <AnchoredOverlay
        anchor={rootAnchor}
        ariaLabel="Controlled family root"
        id="controlled-family-root"
        onDismiss={() => {
          metrics.rootDismissals += 1;
          setRootOpen(false);
        }}
        open={rootOpen}
        rootId="library"
      >
        <button id="controlled-family-nested-trigger" ref={setNestedAnchor}>
          Nested
        </button>
        <AnchoredOverlay
          anchor={nestedAnchor}
          ariaLabel="Controlled family nested"
          id="controlled-family-nested"
          onDismiss={() => {
            metrics.nestedDismissals += 1;
            setNestedOpen(false);
          }}
          open={nestedOpen}
          parentId="controlled-family-root"
          rootId="library"
          surfaceRole="nested"
        >
          <OverlayMenuList ariaLabel="Controlled family choices">
            <OverlayMenuItem id="controlled-family-item">Nested item</OverlayMenuItem>
          </OverlayMenuList>
        </AnchoredOverlay>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runControlledFamilyScenario() {
  const control: ScenarioControl = { api: null };
  const metrics = { nestedDismissals: 0, rootDismissals: 0, rootFocuses: 0 };
  const mounted = await mountScenario(
    <ControlledFamilyScenario control={control} metrics={metrics} />
  );
  await act(async () => control.openRoot?.());
  await waitForSelector("#controlled-family-nested-trigger");
  await act(async () => control.openNested?.());
  await waitForFocus("controlled-family-item");

  await act(async () => control.closeControlled?.());
  await settle(20);
  const activeRootAfterFirstClose = control.api?.activeRootId ?? null;
  const nestedDismissalsAfterFirstClose = metrics.nestedDismissals;
  const rootFocusesAfterFirstClose = metrics.rootFocuses;

  await act(async () => control.enableRootTrigger?.());
  await settle();
  await act(async () => control.openRoot?.());
  await settle(10);
  await act(async () => control.openNested?.());
  await settle(30);
  const activeRootAfterReopen = control.api?.activeRootId ?? null;
  await act(async () => control.closeControlled?.());
  await settle(20);

  const result = {
    activeRootAfterFirstClose,
    activeRootAfterReopen,
    activeRootAfterSecondClose: control.api?.activeRootId ?? null,
    nestedDismissalsAfterFirstClose,
    nestedDismissalsAfterSecondClose: metrics.nestedDismissals,
    rootDismissals: metrics.rootDismissals,
    rootFocusesAfterFirstClose,
    rootFocusesAfterSecondClose: metrics.rootFocuses
  };
  await unmountScenario(mounted);
  return result;
}

function ControlledModalFamilyScenario({ control, metrics }: {
  control: ScenarioControl;
  metrics: { modalDismissals: number; rootDismissals: number };
}) {
  const [rootAnchor, setRootAnchor] = useState<HTMLButtonElement | null>(null);
  const modalInitialFocusRef = useRef<HTMLButtonElement>(null);
  const modalRestoreFocusRef = useRef<HTMLButtonElement>(null);
  const [rootOpen, setRootOpen] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  control.closeControlled = () => setRootOpen(false);
  control.openModal = () => setModalOpen(true);
  control.openRoot = () => setRootOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button id="modal-family-root-trigger" ref={setRootAnchor}>Root</button>
      <AnchoredOverlay
        anchor={rootAnchor}
        ariaLabel="Modal family root"
        id="modal-family-root"
        onDismiss={() => {
          metrics.rootDismissals += 1;
          setRootOpen(false);
        }}
        open={rootOpen}
        rootId="library"
      >
        <button id="modal-family-open-trigger" ref={modalRestoreFocusRef}>
          Open modal
        </button>
        <ModalSurface
          ariaLabelledBy="modal-family-title"
          id="modal-family-dialog"
          initialFocusRef={modalInitialFocusRef}
          onDismiss={() => {
            metrics.modalDismissals += 1;
            setModalOpen(false);
          }}
          open={modalOpen}
          restoreFocusRef={modalRestoreFocusRef}
          rootId="library"
        >
          <h2 id="modal-family-title">Modal family</h2>
          <button id="modal-family-initial" ref={modalInitialFocusRef}>Confirm</button>
        </ModalSurface>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runControlledModalFamilyScenario() {
  const control: ScenarioControl = { api: null };
  const metrics = { modalDismissals: 0, rootDismissals: 0 };
  const mounted = await mountScenario(
    <ControlledModalFamilyScenario control={control} metrics={metrics} />
  );
  await act(async () => control.openRoot?.());
  await waitForSelector("#modal-family-open-trigger");
  await act(async () => control.openModal?.());
  const modalFocusedBefore = await waitForFocus("modal-family-initial");

  await act(async () => control.closeControlled?.());
  await settle(20);
  const focusDuringClose = document.activeElement?.id ?? "";
  const activeRootAfterClose = control.api?.activeRootId ?? null;
  await settle(180);
  const result = {
    activeRootAfterClose,
    focusAfterExit: document.activeElement?.id ?? "",
    focusDuringClose,
    modalDismissals: metrics.modalDismissals,
    modalFocusedBefore,
    rootDismissals: metrics.rootDismissals
  };
  await unmountScenario(mounted);
  return result;
}

function DeferredAutofocusScenario({ control, initialItems }: {
  control: ScenarioControl;
  initialItems: boolean;
}) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [itemsVisible, setItemsVisible] = useState(initialItems);
  const [open, setOpen] = useState(false);
  control.addItems = () => setItemsVisible(true);
  control.closeControlled = () => setOpen(false);
  control.openRoot = () => setOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button id="deferred-autofocus-trigger" ref={setAnchor}>Trigger</button>
      <AnchoredOverlay
        anchor={anchor}
        ariaLabel="Deferred autofocus"
        id="deferred-autofocus-surface"
        onDismiss={() => setOpen(false)}
        open={open}
        rootId="library"
      >
        <input aria-label="Manual focus" id="deferred-autofocus-manual-control" />
        <OverlayMenuList ariaLabel="Deferred choices" style={{ minHeight: 24 }}>
          {itemsVisible ? (
            <>
              <OverlayMenuItem disabled id="deferred-autofocus-disabled">
                Disabled
              </OverlayMenuItem>
              <OverlayMenuItem id="deferred-autofocus-enabled">Enabled</OverlayMenuItem>
            </>
          ) : null}
        </OverlayMenuList>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runDeferredAutofocusScenarios() {
  const asyncControl: ScenarioControl = { api: null };
  const asyncMounted = await mountScenario(
    <DeferredAutofocusScenario control={asyncControl} initialItems={false} />
  );
  await act(async () => asyncControl.openRoot?.());
  await waitForOpenSurface("deferred-autofocus-surface");
  await settle(30);
  await act(async () => asyncControl.addItems?.());
  await waitForSelector("#deferred-autofocus-enabled");
  const asyncItemFocused = await waitForFocus("deferred-autofocus-enabled");
  await unmountScenario(asyncMounted);

  const reopenControl: ScenarioControl = { api: null };
  const reopenMounted = await mountScenario(
    <DeferredAutofocusScenario control={reopenControl} initialItems />
  );
  await act(async () => reopenControl.openRoot?.());
  const initialItemFocused = await waitForFocus("deferred-autofocus-enabled");
  await act(async () => reopenControl.closeControlled?.());
  await settle(10);
  const triggerFocusedDuringClose = document.activeElement?.id ===
    "deferred-autofocus-trigger";
  await act(async () => reopenControl.openRoot?.());
  const reopenItemFocused = await waitForFocus("deferred-autofocus-enabled");
  await unmountScenario(reopenMounted);

  const userControl: ScenarioControl = { api: null };
  const userMounted = await mountScenario(
    <DeferredAutofocusScenario control={userControl} initialItems={false} />
  );
  await act(async () => userControl.openRoot?.());
  await waitForOpenSurface("deferred-autofocus-surface");
  const manualControl = await waitForSelector("#deferred-autofocus-manual-control");
  await act(async () => manualControl.focus());
  await act(async () => userControl.addItems?.());
  await waitForSelector("#deferred-autofocus-enabled");
  await settle(40);
  const userFocusPreserved = document.activeElement === manualControl;
  await unmountScenario(userMounted);

  return {
    asyncItemFocused,
    initialItemFocused,
    reopenItemFocused,
    triggerFocusedDuringClose,
    userFocusPreserved
  };
}

function PortalDescendantAutofocusScenario({ control, descendantKind }: {
  control: ScenarioControl;
  descendantKind: "modal" | "nested";
}) {
  const [rootAnchor, setRootAnchor] = useState<HTMLButtonElement | null>(null);
  const [nestedAnchor, setNestedAnchor] = useState<HTMLButtonElement | null>(null);
  const modalInitialFocusRef = useRef<HTMLButtonElement>(null);
  const modalRestoreFocusRef = useRef<HTMLButtonElement>(null);
  const [parentItemsVisible, setParentItemsVisible] = useState(false);
  const [rootOpen, setRootOpen] = useState(false);
  const [nestedOpen, setNestedOpen] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  control.addItems = () => setParentItemsVisible(true);
  control.openModal = () => setModalOpen(true);
  control.openNested = () => setNestedOpen(true);
  control.openRoot = () => setRootOpen(true);

  return (
    <OverlayProvider>
      <StackCapture control={control} />
      <button id="portal-descendant-root-trigger" ref={setRootAnchor}>Root</button>
      <AnchoredOverlay
        anchor={rootAnchor}
        ariaLabel="Portal descendant root"
        id="portal-descendant-root"
        onDismiss={() => setRootOpen(false)}
        open={rootOpen}
        rootId="library"
      >
        <OverlayMenuList ariaLabel="Deferred parent choices" style={{ minHeight: 24 }}>
          {parentItemsVisible ? (
            <OverlayMenuItem id="portal-descendant-parent-item">Parent item</OverlayMenuItem>
          ) : null}
        </OverlayMenuList>
        {descendantKind === "nested" ? (
          <>
            <button id="portal-descendant-nested-trigger" ref={setNestedAnchor}>Nested</button>
            <AnchoredOverlay
              anchor={nestedAnchor}
              ariaLabel="Portal descendant nested"
              id="portal-descendant-nested"
              onDismiss={() => setNestedOpen(false)}
              open={nestedOpen}
              parentId="portal-descendant-root"
              rootId="library"
              surfaceRole="nested"
            >
              <OverlayMenuList ariaLabel="Nested descendant choices">
                <OverlayMenuItem id="portal-descendant-nested-item">
                  Nested item
                </OverlayMenuItem>
              </OverlayMenuList>
            </AnchoredOverlay>
          </>
        ) : (
          <>
            <button id="portal-descendant-modal-trigger" ref={modalRestoreFocusRef}>
              Modal
            </button>
            <ModalSurface
              ariaLabelledBy="portal-descendant-modal-title"
              id="portal-descendant-modal"
              initialFocusRef={modalInitialFocusRef}
              onDismiss={() => setModalOpen(false)}
              open={modalOpen}
              restoreFocusRef={modalRestoreFocusRef}
              rootId="library"
            >
              <h2 id="portal-descendant-modal-title">Modal descendant</h2>
              <button id="portal-descendant-modal-item" ref={modalInitialFocusRef}>
                Modal item
              </button>
            </ModalSurface>
          </>
        )}
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runPortalDescendantAutofocusScenario(
  descendantKind: "modal" | "nested"
) {
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <PortalDescendantAutofocusScenario
      control={control}
      descendantKind={descendantKind}
    />
  );
  await act(async () => control.openRoot?.());
  await waitForOpenSurface("portal-descendant-root");
  await settle(30);
  await act(async () => {
    if (descendantKind === "nested") control.openNested?.();
    else control.openModal?.();
  });
  const descendantItemId = descendantKind === "nested"
    ? "portal-descendant-nested-item"
    : "portal-descendant-modal-item";
  const focusedBefore = await waitForFocus(descendantItemId);
  await act(async () => control.addItems?.());
  await waitForSelector("#portal-descendant-parent-item");
  await settle(40);
  const focusPreserved = document.activeElement?.id === descendantItemId;
  await unmountScenario(mounted);
  return { focusPreserved, focusedBefore };
}

async function runPortalDescendantAutofocusScenarios() {
  const nested = await runPortalDescendantAutofocusScenario("nested");
  const modal = await runPortalDescendantAutofocusScenario("modal");
  return {
    modalFocusPreserved: modal.focusPreserved,
    modalFocusedBefore: modal.focusedBefore,
    nestedFocusPreserved: nested.focusPreserved,
    nestedFocusedBefore: nested.focusedBefore
  };
}

function ScrollShellScenario({ metrics }: { metrics: { anchorMeasures: number } }) {
  const anchor = useRef({
    getBoundingClientRect: () => {
      metrics.anchorMeasures += 1;
      return rect(80, 40, 0, 0);
    }
  });

  return (
    <OverlayProvider>
      <AnchoredOverlay
        anchor={anchor.current}
        ariaLabel="Scrollable overlay"
        id="scroll-shell-surface"
        onDismiss={() => undefined}
        open
        rootId="context-menu"
      >
        <div style={{ height: 1200, width: 240 }}>Scrollable content</div>
      </AnchoredOverlay>
    </OverlayProvider>
  );
}

async function runScrollShellScenario() {
  const metrics = { anchorMeasures: 0 };
  const mounted = await mountScenario(<ScrollShellScenario metrics={metrics} />);
  const surface = await waitForSelector("#scroll-shell-surface");
  await settle(60);
  const viewport = surface.querySelector<HTMLElement>(".overlay-scroll-viewport");
  const scrollTarget = viewport ?? surface;
  const before = metrics.anchorMeasures;
  scrollTarget.dispatchEvent(new Event("scroll"));
  await settle(40);
  const result = {
    anchorMeasuresAfterScroll: metrics.anchorMeasures,
    anchorMeasuresBeforeScroll: before,
    hasScrollViewport: viewport !== null,
    shellOverflowY: surface.style.overflowY
  };
  await unmountScenario(mounted);
  return result;
}

function DynamicMenuScenario({ control, metrics }: {
  control: ScenarioControl;
  metrics: { clicks: number };
}) {
  const [disabled, setDisabled] = useState(false);
  control.disableSelectedItem = () => setDisabled(true);
  const handleClick = () => {
    metrics.clicks += 1;
  };

  return (
    <OverlayProvider>
      <OverlayMenuList ariaLabel="Dynamic menu">
        <OverlayMenuItem id="dynamic-a" onClick={handleClick}>Alpha</OverlayMenuItem>
        <OverlayMenuItem
          aria-disabled={disabled || undefined}
          id="dynamic-b"
          onClick={handleClick}
          selected
        >
          Beta
        </OverlayMenuItem>
        {disabled ? <OverlayMenuItem id="dynamic-c">Charlie</OverlayMenuItem> : null}
      </OverlayMenuList>
    </OverlayProvider>
  );
}

async function runDynamicMenuScenario() {
  const control: ScenarioControl = { api: null };
  const metrics = { clicks: 0 };
  const mounted = await mountScenario(
    <DynamicMenuScenario control={control} metrics={metrics} />
  );
  await waitForSelector("#dynamic-b");
  await settle(20);
  await act(async () => control.disableSelectedItem?.());
  await waitForSelector("#dynamic-c");
  await settle(30);

  const disabled = document.querySelector<HTMLButtonElement>("#dynamic-b");
  const enabled = Array.from(document.querySelectorAll<HTMLButtonElement>(
    '[data-overlay-menu-item="true"]:not(:disabled):not([aria-disabled="true"])'
  ));
  disabled?.click();
  const ariaDisabledClicks = metrics.clicks;
  document.querySelector<HTMLButtonElement>("#dynamic-a")?.click();
  const enabledClicks = metrics.clicks;
  const activeId = (document.activeElement as HTMLElement | null)?.id ?? "";
  metrics.clicks = 0;
  const pointerDown = new PointerEvent("pointerdown", {
    bubbles: true,
    cancelable: true,
    pointerId: 1,
    pointerType: "mouse"
  });
  const pointerDownAllowed = disabled?.dispatchEvent(pointerDown) ?? true;
  if (pointerDownAllowed) disabled?.focus();
  const pointerFocusedDisabled = document.activeElement === disabled;
  disabled?.focus();
  disabled?.dispatchEvent(new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    key: "Enter"
  }));
  await settle();
  const result = {
    activeId,
    ariaDisabledClicks,
    disabledTabIndex: disabled?.tabIndex ?? 0,
    enabledClicks,
    enabledTabStops: enabled.filter((item) => item.tabIndex === 0).length,
    enterFallbackClicks: metrics.clicks,
    pointerDownPrevented: !pointerDownAllowed,
    pointerFocusedDisabled
  };
  await unmountScenario(mounted);
  return result;
}

const emptyNotificationModel: CompactNotificationModel = {
  activeLibraryId: null,
  active: [],
  thumbnail: {
    summaries: [],
    libraryCount: 0,
    cachedCount: 0,
    totalCandidateCount: 0,
    missingCount: 0,
    staleCount: 0,
    failedCount: 0,
    pendingCount: 0,
    activeBulkTaskCount: 0,
    totalCacheBytes: null,
    completionPercent: null,
    active: false
  },
  results: [],
  indicator: "idle",
  degraded: false,
  healthDetail: null
};

interface ShellNotificationWiring {
  closeNotifications: () => void;
  notificationAnchor: HTMLButtonElement | null;
  notificationAutoFocus: boolean;
  notificationButtonRef: (element: HTMLButtonElement | null) => void;
  notificationsOpen: boolean;
  onToggleNotifications: (autoFocus: boolean) => void;
}

interface ShellNotificationOverlayProps {
  notificationAnchor: HTMLElement | null;
  notificationAutoFocus: boolean;
  notificationModel: CompactNotificationModel;
  notificationsOpen: boolean;
  onCloseNotifications: () => void;
  onOpenNotificationCenter: () => void;
}

type ShellNotificationWiringHook = () => ShellNotificationWiring;
type ShellNotificationOverlayComponent = ComponentType<ShellNotificationOverlayProps>;

const ShellNotificationOverlay = (
  ShellOverlayHostModule as typeof ShellOverlayHostModule & {
    ShellNotificationOverlay?: ShellNotificationOverlayComponent;
  }
).ShellNotificationOverlay;

async function loadShellNotificationWiringHook(): Promise<ShellNotificationWiringHook | null> {
  const modulePath = "/src/app-shell/useShellNotificationOverlay.ts";
  const module = await import(/* @vite-ignore */ modulePath).catch(() => null) as null | {
    useShellNotificationOverlay?: ShellNotificationWiringHook;
  };
  return module?.useShellNotificationOverlay ?? null;
}

function NotificationRootLifecycleScenario({
  control,
  useNotificationWiring
}: {
  control: ScenarioControl;
  useNotificationWiring: ShellNotificationWiringHook;
}) {
  const wiring = useNotificationWiring();
  const overlays = useOverlayStack();
  control.api = overlays;
  control.openRoot = () => overlays.openRoot("notifications");

  return (
    <>
      <button id="notification-root-before">Before</button>
      <ShellRightActions
        notificationButtonRef={wiring.notificationButtonRef}
        notificationIndicator="idle"
        notificationsOpen={wiring.notificationsOpen}
        onToggleNotifications={wiring.onToggleNotifications}
      />
      <button id="notification-root-outside">Outside</button>
      {ShellNotificationOverlay ? (
        <ShellNotificationOverlay
          notificationAnchor={wiring.notificationAnchor}
          notificationAutoFocus={wiring.notificationAutoFocus}
          notificationModel={emptyNotificationModel}
          notificationsOpen={wiring.notificationsOpen}
          onCloseNotifications={wiring.closeNotifications}
          onOpenNotificationCenter={() => undefined}
        />
      ) : null}
    </>
  );
}

async function runNotificationRootLifecycleScenario() {
  const useNotificationWiring = await loadShellNotificationWiringHook();
  if (!useNotificationWiring || !ShellNotificationOverlay) {
    return {
      activeRootAfterSwitch: null,
      blurClosed: false,
      escapeClosed: false,
      escapeFocus: "",
      keyboardAutoFocused: false,
      keyboardActiveRoot: null,
      keyboardOpened: false,
      keyboardTriggerFocused: false,
      mouseOpenFocus: "",
      outsidePointerClosed: false,
      productionWiringMounted: false,
      switchClosed: false,
      tabEnteredContent: false,
      tabKeptOpen: false
    };
  }
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <NotificationRootLifecycleScenario
        control={control}
        useNotificationWiring={useNotificationWiring}
      />
    </OverlayProvider>
  );
  const before = await waitForSelector("#notification-root-before");
  const trigger = await waitForSelector('[data-compact-popover-trigger="notifications"]');
  const outside = await waitForSelector("#notification-root-outside");
  if (
    !(before instanceof HTMLButtonElement) ||
    !(trigger instanceof HTMLButtonElement) ||
    !(outside instanceof HTMLButtonElement)
  ) {
    throw new Error("notification lifecycle controls must be buttons");
  }

  const openNotificationByMouse = async () => {
    await requestNativePointer("mouse-click", trigger);
    await waitForSelector("#notifications-root");
    await settle(30);
  };
  const resetNotification = async () => {
    await act(async () => control.api?.closeRoot("notifications"));
    await settle(20);
  };

  await openNotificationByMouse();
  const mouseOpenFocus = (document.activeElement as HTMLElement | null)?.dataset
    .compactPopoverTrigger ?? "";
  await requestNativeKey("Tab");
  await settle(20);
  const tabEnteredContent = document.activeElement?.classList.contains("notification-open-center") ?? false;
  const tabKeptOpen = document.querySelector("#notifications-root") !== null;
  await resetNotification();

  before.focus();
  await requestNativeKey("Tab");
  const keyboardTriggerFocused = document.activeElement === trigger;
  await requestNativeKey("Space");
  await settle(30);
  const keyboardOpened =
    document.querySelector("#notifications-root") !== null;
  const keyboardActiveRoot = control.api?.activeRootId ?? null;
  const keyboardAutoFocused =
    document.activeElement?.classList.contains("notification-open-center") ?? false;
  if (keyboardOpened) {
    await requestNativeKey("Escape");
    await settle(20);
  }
  const escapeClosed = document.querySelector("#notifications-root") === null;
  const escapeFocus = (document.activeElement as HTMLElement | null)?.dataset
    .compactPopoverTrigger ?? "";
  await resetNotification();

  await openNotificationByMouse();
  await requestNativePointer("mouse-click", before);
  await settle(20);
  const outsidePointerClosed =
    document.querySelector("#notifications-root") === null;
  await resetNotification();

  await openNotificationByMouse();
  await act(async () => window.dispatchEvent(new Event("blur")));
  await settle(20);
  const blurClosed = document.querySelector("#notifications-root") === null;
  await resetNotification();

  await openNotificationByMouse();
  await act(async () => control.api?.openRoot("filter"));
  await settle(20);
  const switchClosed = document.querySelector("#notifications-root") === null;
  const activeRootAfterSwitch = control.api?.activeRootId ?? null;

  const result = {
    activeRootAfterSwitch,
    blurClosed,
    escapeClosed,
    escapeFocus,
    keyboardAutoFocused,
    keyboardActiveRoot,
    keyboardOpened,
    keyboardTriggerFocused,
    mouseOpenFocus,
    outsidePointerClosed,
    productionWiringMounted: true,
    switchClosed,
    tabEnteredContent,
    tabKeptOpen
  };
  await unmountScenario(mounted);
  return result;
}

function TitlebarControlsScenario() {
  const overlays = useOverlayStack();
  const [kind, setKind] = useState<"image" | "video" | "other" | undefined>();
  const [rating, setRating] = useState<1 | 2 | 3 | 4 | 5 | undefined>();
  const [favorite, setFavorite] = useState<boolean | undefined>();
  const [sort, setSort] = useState<SortOption>("mtime_desc");
  const [layout, setLayout] = useState<LibraryLayoutMode>("adaptive");
  const [query, setQuery] = useState("");
  const setRootOpen = (rootId: "filter" | "sort" | "layout", open: boolean) => {
    if (open) overlays.openRoot(rootId);
    else overlays.closeRoot(rootId);
  };

  return (
    <LiquidGlassLayer>
      <div
        id="titlebar-controls-fixture"
        style={{ alignItems: "flex-start", display: "flex", gap: 8 }}
      >
        <FilterMenu
          favorite={favorite}
          kind={kind}
          minRating={rating}
          onClear={() => {
            setKind(undefined);
            setRating(undefined);
            setFavorite(undefined);
          }}
          onOpenChange={(open) => setRootOpen("filter", open)}
          onSetKind={setKind}
          onSetMinRating={setRating}
          onToggleFavorite={() => setFavorite((current) => current === true ? undefined : true)}
          onToggleTag={() => undefined}
          open={overlays.isRootOpen("filter")}
          tagIds={[]}
          tags={[]}
        />
        <SortMenu
          onChange={setSort}
          onOpenChange={(open) => setRootOpen("sort", open)}
          open={overlays.isRootOpen("sort")}
          value={sort}
        />
        <LayoutMenu
          onChange={setLayout}
          onOpenChange={(open) => setRootOpen("layout", open)}
          open={overlays.isRootOpen("layout")}
          value={layout}
        />
        <SearchBar onChange={setQuery} value={query} />
        <output id="titlebar-search-value">{query}</output>
      </div>
    </LiquidGlassLayer>
  );
}

const COMPOSITE_ROLE_SELECTOR = '[role="menu"], [role="listbox"]';

function compositeRoleCount(surface: HTMLElement) {
  return Number(surface.matches(COMPOSITE_ROLE_SELECTOR)) +
    surface.querySelectorAll(COMPOSITE_ROLE_SELECTOR).length;
}

async function runTitlebarControlsScenario() {
  const mounted = await mountScenario(
    <OverlayProvider>
      <TitlebarControlsScenario />
    </OverlayProvider>
  );

  const filterTrigger = await waitForSelector('[data-titlebar-control="library-filter"]');
  await requestNativePointer("mouse-click", filterTrigger);
  const filterSurface = await waitForOpenSurface("filter-root");
  const filterCompositeRoles = compositeRoleCount(filterSurface);
  const filterSurfaceRole = filterSurface.getAttribute("role");
  const filterControls = filterTrigger.getAttribute("aria-controls");

  const sortTrigger = await waitForSelector('[data-titlebar-control="library-sort"]');
  await requestNativePointer("mouse-click", sortTrigger);
  const sortSurface = await waitForOpenSurface("sort-root");
  const sortCompositeRoles = compositeRoleCount(sortSurface);
  const sortSurfaceRole = sortSurface.getAttribute("role");
  const sortControls = sortTrigger.getAttribute("aria-controls");
  const sortSelectedStates = Array.from(sortSurface.querySelectorAll('[role="option"]'))
    .map((item) => item.getAttribute("aria-selected"));

  const layoutTrigger = await waitForSelector('[data-titlebar-control="library-layout"]');
  await requestNativePointer("mouse-click", layoutTrigger);
  const layoutSurface = await waitForOpenSurface("layout-root");
  const layoutCompositeRoles = compositeRoleCount(layoutSurface);
  const layoutSurfaceRole = layoutSurface.getAttribute("role");
  const layoutControls = layoutTrigger.getAttribute("aria-controls");
  const layoutSelectedStates = Array.from(layoutSurface.querySelectorAll('[role="option"]'))
    .map((item) => item.getAttribute("aria-selected"));

  const searchInput = await waitForSelector('input[aria-label="Search library"]');
  if (!(searchInput instanceof HTMLInputElement)) {
    throw new Error("titlebar search control must be an input");
  }
  const valueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (!valueSetter) throw new Error("native input value setter is unavailable");
  await act(async () => {
    valueSetter.call(searchInput, "needle");
    searchInput.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const searchValueAfterInput = document.querySelector("#titlebar-search-value")?.textContent ?? "";
  const clearButton = await waitForSelector('.search-bar-clear');
  await requestNativePointer("mouse-move", clearButton);
  await settle(30);
  const clearPointerTarget = clearButton.getAttribute("data-interactive-pointer-target");
  const inputPointerGlow = searchInput.getAttribute("data-pointer-glow");
  await requestNativePointer("mouse-click", clearButton);
  await settle(20);
  const clearSynchronous =
    searchInput.value === "" &&
    document.querySelector("#titlebar-search-value")?.textContent === "";
  const clearFocusedInput = document.activeElement === searchInput;

  const result = {
    clearFocusedInput,
    clearPointerTarget,
    clearSynchronous,
    filterCompositeRoles,
    filterControls,
    filterSurfaceRole,
    inputPointerGlow,
    layoutCompositeRoles,
    layoutControls,
    layoutSelectedStates,
    layoutSurfaceRole,
    searchValueAfterInput,
    sortCompositeRoles,
    sortControls,
    sortSelectedStates,
    sortSurfaceRole
  };
  await unmountScenario(mounted);
  return result;
}

async function run(): Promise<void> {
  try {
    const focusProbe = document.createElement("button");
    document.body.append(focusProbe);
    focusProbe.focus();
    const basicFocusWorks = document.activeElement === focusProbe;
    focusProbe.remove();
    window.__overlayLifecycleResult = {
      ok: true,
      basicFocusWorks,
      reentrancy: await runReentrancyScenarios(),
      autofocus: await runAutoFocusScenarios(),
      controlledFocus: await runControlledFocusScenarios(),
      controlledFamily: await runControlledFamilyScenario(),
      modalFamilyFocus: await runControlledModalFamilyScenario(),
      deferredAutofocus: await runDeferredAutofocusScenarios(),
      portalDescendantAutofocus: await runPortalDescendantAutofocusScenarios(),
      scrollShell: await runScrollShellScenario(),
      dynamicMenu: await runDynamicMenuScenario(),
      notificationRoot: await runNotificationRootLifecycleScenario(),
      titlebarControls: await runTitlebarControlsScenario()
    };
  } catch (error) {
    window.__overlayLifecycleResult = {
      ok: false,
      error: error instanceof Error ? error.stack ?? error.message : String(error)
    };
  }
}

void run();

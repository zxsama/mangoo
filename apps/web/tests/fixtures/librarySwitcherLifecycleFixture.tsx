import {
  act,
  useLayoutEffect,
  useState,
  type ReactElement,
} from "react";
import { createRoot, type Root } from "react-dom/client";
import type { LibraryState } from "../../src/core/useLibraryData";
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
  fallback?: {
    activeRootId: string | null;
    interactive: boolean;
    visible: boolean;
  };
  focusRestore?: {
    cancelRestoredTrigger: boolean;
    escapeRestoredTrigger: boolean;
    nestedMenuStayedOpen: boolean;
    rootStayedOpen: boolean;
  };
}

interface MountedScenario {
  container: HTMLDivElement;
  root: Root;
}

interface ScenarioControl {
  api: OverlayStackApi | null;
}

const lifecycleWindow = window as Window & {
  __overlayLifecycleInputRequest?: NativeInputRequest;
  __overlayLifecycleInputResponseId?: number;
  __overlayLifecycleResult?: LibrarySwitcherLifecycleResult;
};

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const delay = (milliseconds = 0) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));

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
  lifecycleWindow.__overlayLifecycleInputRequest = { ...request, id };
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (lifecycleWindow.__overlayLifecycleInputResponseId === id) return;
    await delay(10);
  }
  throw new Error(`Timed out waiting for native input response ${id}`);
}

const requestNativeKey = (keyCode: string) =>
  requestNativeInput({ keyCode, type: "key" });

const requestNativeClick = (element: HTMLElement) => {
  const bounds = element.getBoundingClientRect();
  return requestNativeInput({
    type: "mouse-click",
    x: Math.round(bounds.left + bounds.width / 2),
    y: Math.round(bounds.top + bounds.height / 2),
  });
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
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (!document.getElementById(id)) return;
    await settle(10);
  }
  throw new Error(`Timed out waiting for overlay layer #${id} to exit`);
}

async function findElement(selector: string): Promise<HTMLElement> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const element = document.querySelector(selector);
    if (element instanceof HTMLElement) return element;
    await settle(10);
  }
  throw new Error(`Timed out waiting for ${selector}`);
}

async function findButton(text: string): Promise<HTMLButtonElement> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const button = Array.from(document.querySelectorAll("button")).find(
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

const library = {
  roots: [{ displayName: "Pictures", id: 7 }],
  addRoot: async () => undefined,
  removeRoot: async () => undefined,
} as unknown as LibraryState;

function LibrarySwitcherScenario({ control }: { control: ScenarioControl }) {
  const overlays = useOverlayStack();
  return (
    <LiquidGlassLayer>
      <StackCapture control={control} />
      <LibraryLoadingOverlay
        onRetry={() => undefined}
        onSwitchLibrary={() => overlays.openRoot("library")}
        phase={{ kind: "error", message: "Fixture load error" }}
      />
      <LibrarySwitcher
        anchor={null}
        library={library}
        onClose={() => overlays.closeRoot("library")}
        open={overlays.isRootOpen("library")}
      />
    </LiquidGlassLayer>
  );
}

function LibrarySwitcherFocusScenario({ control }: { control: ScenarioControl }) {
  const overlays = useOverlayStack();
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  return (
    <LiquidGlassLayer>
      <StackCapture control={control} />
      <button
        id="library-switcher-fixture-trigger"
        onClick={() => overlays.openRoot("library")}
        ref={setAnchor}
        type="button"
      >
        Library
      </button>
      <LibrarySwitcher
        anchor={anchor}
        library={library}
        onClose={() => overlays.closeRoot("library")}
        open={overlays.isRootOpen("library")}
      />
    </LiquidGlassLayer>
  );
}

async function runFallbackScenario() {
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <LibrarySwitcherScenario control={control} />
    </OverlayProvider>,
  );
  const switchButton = await findButton("切换其它库");
  await requestNativeClick(switchButton);
  const surface = await findOpenSurface("library-root");
  let interactive = false;
  if (surface) {
    const input = surface.querySelector('input[aria-label="搜索资源库"]');
    if (input instanceof HTMLInputElement) {
      const bounds = input.getBoundingClientRect();
      await requestNativeClick(input);
      await settle(20);
      interactive =
        document.activeElement === input &&
        document.elementFromPoint(
          bounds.left + bounds.width / 2,
          bounds.top + bounds.height / 2,
        ) === input;
    }
  }
  const result = {
    activeRootId: control.api?.activeRootId ?? null,
    interactive,
    visible: surface !== null,
  };
  await unmountScenario(mounted);
  return result;
}

async function runFocusRestoreScenario() {
  const control: ScenarioControl = { api: null };
  const mounted = await mountScenario(
    <OverlayProvider>
      <LibrarySwitcherFocusScenario control={control} />
    </OverlayProvider>,
  );
  const rootTrigger = await findElement("#library-switcher-fixture-trigger");
  await act(async () => {
    rootTrigger.click();
  });
  await settle(20);
  const rootSurface = await findOpenSurface("library-root");
  if (!rootSurface) throw new Error("LibrarySwitcher root did not open");

  const removeTrigger = await findElement('button[aria-label="Remove folder"]');
  await requestNativeClick(removeTrigger);
  const removeMenu = await findOpenSurface("library-remove-folders");
  if (!removeMenu) throw new Error("Remove-folder menu did not open");
  const removeItem = await findButton("Pictures");

  await requestNativeClick(removeItem);
  const cancelModal = await findOpenLayer("library-remove-confirm");
  if (!cancelModal) throw new Error("Remove confirmation did not open for cancellation");
  const cancelButton = await findButton("取消");
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

async function run(): Promise<void> {
  try {
    window.megleDesktop = {
      listLibraries: async () => [
        {
          active: true,
          available: true,
          dbPath: "C:\\Fixture\\library.sqlite",
          id: "fixture-library",
          name: "Alpha Library",
        },
      ],
    };
    lifecycleWindow.__overlayLifecycleResult = {
      fallback: await runFallbackScenario(),
      focusRestore: await runFocusRestoreScenario(),
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

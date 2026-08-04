import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

const read = (path: string) =>
  readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

interface FileOperationOverlayResult {
  ok: boolean;
  error?: string;
  contextMenu: {
    activatedItem: string;
    clampedToViewport: boolean;
    focusedAcrossSeparator: boolean;
    restoredFocus: boolean;
    sharedRoot: boolean;
    singleMenuRole: boolean;
  };
  rename: {
    busyBlockedDismissal: boolean;
    callCount: number;
    failureVisible: boolean;
    fieldErrorLinked: boolean;
    invalidBlocked: boolean;
    restoredFocus: boolean;
    selectedStem: boolean;
    sharedModal: boolean;
  };
  move: {
    accessibleTreeModel: boolean;
    callCount: number;
    crossRootFailureVisible: boolean;
    descendantExcluded: boolean;
    keyboardNavigation: boolean;
    rovingTabStop: boolean;
    selectedTarget: number | null;
    selfExcluded: boolean;
    sharedModal: boolean;
  };
  movePagination: {
    busyVisible: boolean;
    duplicateBlocked: boolean;
    failureVisible: boolean;
    loadedBeyondFirstPage: boolean;
    selectedBeyondFirstPage: boolean;
  };
  remove: {
    busyBlockedDismissal: boolean;
    callCount: number;
    duplicateBlocked: boolean;
    failureVisible: boolean;
    permanentInputFocused: boolean;
    recycleCancelFocused: boolean;
    recycleClosedOnSuccess: boolean;
    restoredFocus: boolean;
    sharedAlertDialog: boolean;
    typedConfirmationRequired: boolean;
  };
  shortcuts: {
    deleteOwnerReplaced: boolean;
    deleteRootOwnedBeforeOpen: boolean;
    renameOwnerReplaced: boolean;
    renameRootOwnedBeforeOpen: boolean;
    staleContextOwnerCleared: boolean;
  };
}

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const fixtureDirectory = join(webRoot, ".tmp", "file-operation-overlays");
const fixtureHtmlPath = join(fixtureDirectory, "fixture.html");
const fixtureSourcePath = join(fixtureDirectory, "fixture.tsx");
const electronRunner = fileURLToPath(
  new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url),
);
const resultPrefix = "OVERLAY_LIFECYCLE_RESULT:";

let server: ViteDevServer;
let mountedResult: FileOperationOverlayResult;

const fixtureSource = String.raw`
import {
  act,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ComponentType,
  type ReactElement,
  type RefObject,
} from "react";
import { createRoot, type Root } from "react-dom/client";
import { OverlayProvider, useOverlayStack } from "/src/design/overlays/index.ts";
import { ShellOverlayHost } from "/src/app-shell/ShellOverlayHost.tsx";
import { ContextMenu } from "/src/features/file-ops/ContextMenu.tsx";
import { MoveDialog } from "/src/features/file-ops/MoveDialog.tsx";
import { useFileOpsController } from "/src/features/file-ops/useFileOps.ts";
import { useShortcuts } from "/src/features/shortcuts/useShortcuts.ts";
import "/src/styles.css";
import "/src/design/overlays/overlay.css";

type NativeInputPayload =
  | { keyCode: string; type: "key" }
  | { type: "mouse-click"; x: number; y: number };
type NativeInputRequest = NativeInputPayload & { id: number };
type OperationResult = { ok: boolean; code?: string; message?: string };

interface Deferred<T> {
  promise: Promise<T>;
  reject: (reason: unknown) => void;
  resolve: (value: T) => void;
}

interface FixtureResult {
  ok: boolean;
  error?: string;
  contextMenu?: {
    activatedItem: string;
    clampedToViewport: boolean;
    focusedAcrossSeparator: boolean;
    restoredFocus: boolean;
    sharedRoot: boolean;
    singleMenuRole: boolean;
  };
  rename?: {
    busyBlockedDismissal: boolean;
    callCount: number;
    failureVisible: boolean;
    fieldErrorLinked: boolean;
    invalidBlocked: boolean;
    restoredFocus: boolean;
    selectedStem: boolean;
    sharedModal: boolean;
  };
  move?: {
    accessibleTreeModel: boolean;
    callCount: number;
    crossRootFailureVisible: boolean;
    descendantExcluded: boolean;
    keyboardNavigation: boolean;
    rovingTabStop: boolean;
    selectedTarget: number | null;
    selfExcluded: boolean;
    sharedModal: boolean;
  };
  movePagination?: {
    busyVisible: boolean;
    duplicateBlocked: boolean;
    failureVisible: boolean;
    loadedBeyondFirstPage: boolean;
    selectedBeyondFirstPage: boolean;
  };
  remove?: {
    busyBlockedDismissal: boolean;
    callCount: number;
    duplicateBlocked: boolean;
    failureVisible: boolean;
    permanentInputFocused: boolean;
    recycleCancelFocused: boolean;
    recycleClosedOnSuccess: boolean;
    restoredFocus: boolean;
    sharedAlertDialog: boolean;
    typedConfirmationRequired: boolean;
  };
  shortcuts?: {
    deleteOwnerReplaced: boolean;
    deleteRootOwnedBeforeOpen: boolean;
    renameOwnerReplaced: boolean;
    renameRootOwnedBeforeOpen: boolean;
    staleContextOwnerCleared: boolean;
  };
}

interface MountedScenario {
  container: HTMLDivElement;
  root: Root;
}

interface ContextControl {
  activeRootId: () => string | null;
  open: () => void;
  selected: string;
}

interface OperationControl {
  activeRootId: () => string | null;
  deleteCalls: Array<{ fileIds: number[]; folderIds: number[]; permanent: boolean }>;
  deleteRequests: Array<Deferred<OperationResult>>;
  moveCalls: Array<{ fileIds: number[]; folderIds: number[]; targetFolderId: number }>;
  moveRequests: Array<Deferred<OperationResult>>;
  openDelete: (permanent: boolean) => void;
  openMove: () => void;
  openRename: () => void;
  renameCalls: Array<{ id: number; name: string }>;
  renameRequests: Array<Deferred<OperationResult>>;
}

interface ShortcutControl {
  ownerAtOpen: string | null;
  rootAtOpen: string | null;
  staleContextOwnerCleared: boolean;
}

interface MovePaginationControl {
  calls: number;
  requests: Array<Deferred<void>>;
  selectedTarget: number | null;
}

const lifecycleWindow = window as Window & {
  __overlayLifecycleInputRequest?: NativeInputRequest;
  __overlayLifecycleInputResponseId?: number;
  __overlayLifecycleResult?: FixtureResult;
};

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const ContextMenuCompat = ContextMenu as ComponentType<Record<string, unknown>>;
const MoveDialogCompat = MoveDialog as ComponentType<Record<string, unknown>>;
const ShellOverlayHostCompat = ShellOverlayHost as ComponentType<Record<string, unknown>>;
const useShortcutsCompat = useShortcuts as (options: Record<string, unknown>) => void;

const media = {
  addedAt: 1,
  ext: "jpg",
  folderId: 1,
  height: 100,
  id: 41,
  kind: "image",
  mtime: 1,
  name: "photo.jpg",
  rootId: 1,
  size: 100,
  width: 100,
};

const movedFolder = {
  childCount: 1,
  id: 2,
  mediaCount: 0,
  name: "Moved folder",
  parentId: 1,
  rootId: 1,
};

const movedDescendant = {
  childCount: 0,
  id: 3,
  mediaCount: 0,
  name: "Moved descendant",
  parentId: 2,
  rootId: 1,
};

const delay = (milliseconds = 0) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));

function deferred<T>(): Deferred<T> {
  let reject!: (reason: unknown) => void;
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    reject = rejectPromise;
    resolve = resolvePromise;
  });
  return { promise, reject, resolve };
}

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
  throw new Error("Timed out waiting for native input response " + id);
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
  await act(async () => root.unmount());
  container.remove();
  document.querySelectorAll("[data-overlay-layer-kind]").forEach((node) => node.remove());
  await settle();
}

async function waitFor(
  condition: () => boolean,
  message: string,
): Promise<void> {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (condition()) return;
    await settle(10);
  }
  throw new Error("Timed out waiting for " + message);
}

async function findElement(
  selector: string,
  root: ParentNode = document,
): Promise<HTMLElement> {
  let element: HTMLElement | null = null;
  await waitFor(() => {
    const candidate = root.querySelector(selector);
    element = candidate instanceof HTMLElement ? candidate : null;
    return element !== null;
  }, selector);
  return element as HTMLElement;
}

async function findButton(text: string, root: ParentNode = document): Promise<HTMLButtonElement> {
  let result: HTMLButtonElement | null = null;
  await waitFor(() => {
    const candidate = Array.from(root.querySelectorAll("button")).find(
      (button) => button.textContent?.trim() === text,
    );
    result = candidate instanceof HTMLButtonElement ? candidate : null;
    return result !== null;
  }, "button " + text);
  return result as HTMLButtonElement;
}

async function waitForOpenSurface(id: string): Promise<HTMLElement> {
  let surface: HTMLElement | null = null;
  await waitFor(() => {
    const candidate = document.getElementById(id);
    if (!(candidate instanceof HTMLElement)) return false;
    if (candidate.dataset.overlayState && candidate.dataset.overlayState !== "open") return false;
    if (candidate.style.visibility === "hidden") return false;
    surface = candidate;
    return true;
  }, "open surface #" + id);
  return surface as HTMLElement;
}

function dialogSelector(id: string, titleId: string): string {
  return "#" + id + ', [aria-labelledby="' + titleId + '"]';
}

async function waitForDialog(id: string, titleId: string): Promise<HTMLElement> {
  return await findElement(dialogSelector(id, titleId));
}

function currentDialog(id: string, titleId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(dialogSelector(id, titleId));
}

async function waitForDialogRemoval(id: string, titleId: string): Promise<void> {
  await waitFor(
    () => currentDialog(id, titleId) === null,
    "removal of " + dialogSelector(id, titleId),
  );
}

function setInputValue(input: HTMLInputElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (!setter) throw new Error("Native input value setter is unavailable");
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function ContextScenario({ control }: { control: ContextControl }) {
  const overlays = useOverlayStack();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement | null>(null);

  control.activeRootId = () => overlays.activeRootId;
  control.open = () => {
    overlays.openRoot("context-menu");
    setOpen(true);
  };

  const dismiss = () => setOpen(false);
  const close = () => {
    setOpen(false);
    overlays.closeRoot("context-menu");
  };

  return (
    <>
      <button id="context-trigger" ref={triggerRef} onClick={control.open}>Open menu</button>
      {open ? (
        <ContextMenuCompat
          ariaLabel="Fixture actions"
          items={[
            { id: "alpha", label: "Alpha", onSelect: () => { control.selected = "alpha"; close(); } },
            { id: "separator", separator: true },
            { id: "disabled", label: "Disabled", disabled: true },
            { id: "omega", label: "Omega", onSelect: () => { control.selected = "omega"; close(); } },
          ]}
          onClose={close}
          onDismiss={dismiss}
          restoreFocusTarget={triggerRef.current}
          x={796}
          y={596}
        />
      ) : null}
    </>
  );
}

async function runContextMenuScenario() {
  const control: ContextControl = {
    activeRootId: () => null,
    open: () => undefined,
    selected: "",
  };
  const mounted = await mountScenario(
    <OverlayProvider><ContextScenario control={control} /></OverlayProvider>,
  );
  const trigger = await findButton("Open menu");
  trigger.focus();
  await act(async () => trigger.click());
  const surface = await findElement("#context-menu-root, .context-menu");
  await settle(40);
  const first = await findButton("Alpha", surface);
  await waitFor(() => document.activeElement === first, "context menu autofocus");
  const firstFocused = true;
  await requestNativeKey("Down");
  const omega = await findButton("Omega", surface);
  const focusedAcrossSeparator = firstFocused && document.activeElement === omega;
  await requestNativeKey("Enter");
  await settle(30);
  const bounds = surface.getBoundingClientRect();
  const result = {
    activatedItem: control.selected,
    clampedToViewport:
      bounds.left >= 0 && bounds.top >= 0 &&
      bounds.right <= window.innerWidth && bounds.bottom <= window.innerHeight,
    focusedAcrossSeparator,
    restoredFocus: document.activeElement === trigger,
    sharedRoot:
      surface.dataset.overlayLayerKind === "root" &&
      control.activeRootId() === null,
    singleMenuRole:
      Number(surface.matches('[role="menu"]')) + surface.querySelectorAll('[role="menu"]').length === 1,
  };
  await unmountScenario(mounted);
  return result;
}

function ShortcutScenario({
  action,
  control,
}: {
  action: "delete" | "rename";
  control: ShortcutControl;
}) {
  const overlays = useOverlayStack();
  const [contextOpen, setContextOpen] = useState(false);
  const contextSourceRef = useRef<HTMLButtonElement | null>(null);
  const shortcutSourceRef = useRef<HTMLButtonElement | null>(null);
  const fileOperationRestoreFocusRef = useRef<HTMLElement | null>(null);

  control.rootAtOpen = overlays.activeRootId;

  const dismissContextMenu = () => {
    setContextOpen(false);
    fileOperationRestoreFocusRef.current = null;
  };
  const recordOpen = () => {
    control.ownerAtOpen = fileOperationRestoreFocusRef.current?.id ?? null;
  };
  const fileOps = {
    closeAll: () => undefined,
    move: { busy: false, serverError: null, serverErrorCode: null, target: null },
    openDelete: recordOpen,
    openMove: () => undefined,
    openRename: recordOpen,
    remove: { busy: false, permanent: false, serverError: null, target: null },
    rename: { busy: false, serverError: null, target: null },
    submitDelete: async () => undefined,
    submitMove: async () => undefined,
    submitRename: async () => undefined,
  };
  const library = {
    selectedMedia: media,
    selectedMediaId: media.id,
    setSelectedMediaId: () => undefined,
  };

  useShortcutsCompat({
    fileOperationRestoreFocusRef,
    fileOps,
    library,
    onClosePreview: () => undefined,
    onDismissContextMenu: dismissContextMenu,
    onPreviewNext: () => undefined,
    onPreviewPrevious: () => undefined,
    onToggleSidebars: () => undefined,
    previewOpen: false,
  });

  control.staleContextOwnerCleared = !contextOpen && fileOperationRestoreFocusRef.current === null;

  return (
    <>
      <button
        id={action + "-context-source"}
        onClick={() => {
          fileOperationRestoreFocusRef.current = contextSourceRef.current;
          setContextOpen(true);
        }}
        ref={contextSourceRef}
      >
        Open stale context
      </button>
      <button id={action + "-shortcut-source"} ref={shortcutSourceRef}>
        Shortcut source
      </button>
      {contextOpen ? (
        <ContextMenuCompat
          ariaLabel="Stale context actions"
          items={[{ id: "noop", label: "No operation" }]}
          onDismiss={dismissContextMenu}
          restoreFocusTarget={contextSourceRef.current}
          x={40}
          y={40}
        />
      ) : null}
    </>
  );
}

async function runShortcutAction(action: "delete" | "rename") {
  const control: ShortcutControl = {
    ownerAtOpen: null,
    rootAtOpen: null,
    staleContextOwnerCleared: false,
  };
  const mounted = await mountScenario(
    <OverlayProvider><ShortcutScenario action={action} control={control} /></OverlayProvider>,
  );
  await requestNativeClick(await findButton("Open stale context"));
  await waitForOpenSurface("context-menu-root");
  await requestNativeKey("Escape");
  await waitFor(() => document.getElementById("context-menu-root") === null, "stale context dismissal");
  await settle(30);
  const staleContextOwnerCleared = control.staleContextOwnerCleared;
  const shortcutSource = document.getElementById(action + "-shortcut-source");
  if (!(shortcutSource instanceof HTMLButtonElement)) {
    throw new Error("Shortcut source is unavailable");
  }
  shortcutSource.focus();
  await requestNativeKey(action === "rename" ? "F2" : "Delete");
  await settle(30);
  const result = {
    ownerReplaced: control.ownerAtOpen === shortcutSource.id,
    rootOwnedBeforeOpen: control.rootAtOpen === "file-operation",
    staleContextOwnerCleared,
  };
  await unmountScenario(mounted);
  return result;
}

async function runShortcutScenario() {
  const rename = await runShortcutAction("rename");
  const remove = await runShortcutAction("delete");
  return {
    deleteOwnerReplaced: remove.ownerReplaced,
    deleteRootOwnedBeforeOpen: remove.rootOwnedBeforeOpen,
    renameOwnerReplaced: rename.ownerReplaced,
    renameRootOwnedBeforeOpen: rename.rootOwnedBeforeOpen,
    staleContextOwnerCleared:
      rename.staleContextOwnerCleared && remove.staleContextOwnerCleared,
  };
}

function createLibrary(control: OperationControl) {
  return {
    deleteItems: (request: { fileIds: number[]; folderIds: number[]; permanent: boolean }) => {
      const pending = deferred<OperationResult>();
      control.deleteCalls.push(request);
      control.deleteRequests.push(pending);
      return pending.promise;
    },
    expandedFolderIds: new Set<number>([1]),
    folderChildrenByParent: {
      1: [movedFolder],
      2: [movedDescendant],
      3: [],
    },
    folders: [movedFolder, movedDescendant],
    moveItems: (request: { fileIds: number[]; folderIds: number[]; targetFolderId: number }) => {
      const pending = deferred<OperationResult>();
      control.moveCalls.push(request);
      control.moveRequests.push(pending);
      return pending.promise;
    },
    renameFile: (id: number, name: string) => {
      const pending = deferred<OperationResult>();
      control.renameCalls.push({ id, name });
      control.renameRequests.push(pending);
      return pending.promise;
    },
    renameFolder: async () => ({ ok: true }),
    roots: [{ displayName: "Library root", id: 1, rootFolderId: 1 }],
    toggleFolderExpanded: () => undefined,
  };
}

function OperationScenario({ control }: { control: OperationControl }) {
  const overlays = useOverlayStack();
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const libraryRef = useRef<ReturnType<typeof createLibrary> | null>(null);
  if (!libraryRef.current) libraryRef.current = createLibrary(control);
  const library = libraryRef.current;
  const fileOps = useFileOpsController(library as never);
  const fileOperationOpen = Boolean(
    fileOps.rename.target || fileOps.move.target || fileOps.remove.target,
  );
  const previousOpenRef = useRef(false);

  useEffect(() => {
    if (previousOpenRef.current && !fileOperationOpen) {
      overlays.closeRoot("file-operation");
    }
    previousOpenRef.current = fileOperationOpen;
  }, [fileOperationOpen, overlays]);

  const begin = (source: HTMLElement, openOperation: () => void) => {
    restoreFocusRef.current = source;
    overlays.openRoot("file-operation");
    openOperation();
  };

  control.activeRootId = () => overlays.activeRootId;
  control.openRename = () => {
    const source = document.getElementById("rename-trigger");
    if (source instanceof HTMLElement) begin(source, () => fileOps.openRename({ kind: "file", file: media as never }));
  };
  control.openMove = () => {
    const source = document.getElementById("move-trigger");
    if (source instanceof HTMLElement) begin(source, () => fileOps.openMove({ kind: "folder", folder: movedFolder as never }));
  };
  control.openDelete = (permanent) => {
    const id = permanent ? "permanent-trigger" : "recycle-trigger";
    const source = document.getElementById(id);
    if (source instanceof HTMLElement) begin(source, () => fileOps.openDelete({ kind: "file", file: media as never }, permanent));
  };

  return (
    <>
      <button id="rename-trigger" onClick={control.openRename}>Open rename</button>
      <button id="move-trigger" onClick={control.openMove}>Open move</button>
      <button id="permanent-trigger" onClick={() => control.openDelete(true)}>Open permanent</button>
      <button id="recycle-trigger" onClick={() => control.openDelete(false)}>Open recycle</button>
      <ShellOverlayHostCompat
        contextMenu={null}
        contextMenuReturnFocusRef={restoreFocusRef as RefObject<HTMLElement | null>}
        fileOps={fileOps}
        library={library}
        notificationAnchor={null}
        notificationAutoFocus={false}
        notificationModel={{}}
        notificationsOpen={false}
        onCloseContextMenu={() => undefined}
        onDismissContextMenu={() => undefined}
        onOpenNotificationCenter={() => undefined}
      />
    </>
  );
}

async function mountOperationScenario() {
  const control: OperationControl = {
    activeRootId: () => null,
    deleteCalls: [],
    deleteRequests: [],
    moveCalls: [],
    moveRequests: [],
    openDelete: () => undefined,
    openMove: () => undefined,
    openRename: () => undefined,
    renameCalls: [],
    renameRequests: [],
  };
  const mounted = await mountScenario(
    <OverlayProvider><OperationScenario control={control} /></OverlayProvider>,
  );
  return { control, mounted };
}

async function runRenameScenario() {
  const { control, mounted } = await mountOperationScenario();
  const trigger = await findButton("Open rename");
  await requestNativeClick(trigger);
  const dialog = await waitForDialog("rename-dialog", "rename-dialog-title");
  await settle(30);
  const input = await findElement("input", dialog) as HTMLInputElement;
  const selectedStem =
    document.activeElement === input && input.selectionStart === 0 && input.selectionEnd === 5;
  await act(async () => setInputValue(input, "CON"));
  await settle();
  const renameButton = await findButton("Rename", dialog);
  const invalidBlocked = renameButton.disabled && dialog.querySelector('[role="alert"]') !== null;
  const errorId = input.getAttribute("aria-describedby");
  const fieldErrorLinked =
    input.getAttribute("aria-invalid") === "true" &&
    Boolean(errorId && document.getElementById(errorId)?.textContent?.includes("reserved"));
  await act(async () => setInputValue(input, "renamed.jpg"));
  await requestNativeClick(renameButton);
  await settle(20);
  const sharedModal =
    dialog.dataset.overlayLayerKind === "modal" && control.activeRootId() === "file-operation";
  const cancel = await findButton("Cancel", dialog);
  await requestNativeKey("Escape");
  await requestNativePoint(4, 4);
  await act(async () => cancel.click());
  await settle(20);
  const busyBlockedDismissal = currentDialog("rename-dialog", "rename-dialog-title") !== null;
  control.renameRequests[0]?.resolve({ ok: false, message: "Rename denied" });
  await settle(40);
  const failureVisible =
    currentDialog("rename-dialog", "rename-dialog-title")
      ?.querySelector('[role="alert"]')?.textContent === "Rename denied";
  await requestNativeClick(await findButton(
    "Cancel",
    currentDialog("rename-dialog", "rename-dialog-title") ?? document,
  ));
  await waitForDialogRemoval("rename-dialog", "rename-dialog-title");
  const result = {
    busyBlockedDismissal,
    callCount: control.renameCalls.length,
    failureVisible,
    fieldErrorLinked,
    invalidBlocked,
    restoredFocus: document.activeElement === trigger,
    selectedStem,
    sharedModal,
  };
  await unmountScenario(mounted);
  return result;
}

async function runMoveScenario() {
  const { control, mounted } = await mountOperationScenario();
  await requestNativeClick(await findButton("Open move"));
  const dialog = await waitForDialog("move-dialog", "move-dialog-title");
  const sharedModal =
    dialog.dataset.overlayLayerKind === "modal" && control.activeRootId() === "file-operation";
  const tree = await findElement('[role="tree"]', dialog);
  const treeItems = () => Array.from(tree.querySelectorAll<HTMLElement>('[role="treeitem"]'));
  const itemNamed = (name: string) =>
    treeItems().find((item) => item.textContent?.trim() === name) ?? null;
  const rootItem = itemNamed("Library root");
  const selfItem = itemNamed("Moved folder");
  if (!rootItem || !selfItem) throw new Error("Move tree rows are unavailable");
  const accessibleTreeModel =
    rootItem instanceof HTMLButtonElement &&
    selfItem instanceof HTMLButtonElement &&
    rootItem.getAttribute("aria-expanded") === "true" &&
    rootItem.getAttribute("aria-level") === "1";
  const rovingTabStop =
    treeItems().filter((item) => item.tabIndex === 0).length === 1 &&
    treeItems().every((item) => item.tabIndex === 0 || item.tabIndex === -1);
  const selfExcluded =
    selfItem.getAttribute("aria-disabled") === "true" ||
    Boolean(selfItem.querySelector<HTMLButtonElement>(".move-label")?.disabled);
  let descendantExcluded = false;
  let keyboardNavigation = false;

  if (accessibleTreeModel) {
    rootItem.focus();
    await requestNativeKey("Down");
    const movedFocused = document.activeElement === selfItem;
    await requestNativeKey("Right");
    await settle(20);
    const expandedMoved = itemNamed("Moved folder");
    await requestNativeKey("Down");
    const descendantItem = itemNamed("Moved descendant");
    descendantExcluded = descendantItem?.getAttribute("aria-disabled") === "true";
    const descendantFocused = document.activeElement === descendantItem;
    await requestNativeKey("Left");
    const parentFocused = document.activeElement === expandedMoved;
    await requestNativeKey("Left");
    const parentCollapsed = expandedMoved?.getAttribute("aria-expanded") === "false";
    await requestNativeKey("Home");
    const homeFocused = document.activeElement === rootItem;
    await requestNativeKey("End");
    const endFocused = document.activeElement === itemNamed("Moved folder");
    keyboardNavigation = Boolean(
      movedFocused &&
      descendantFocused &&
      parentFocused &&
      parentCollapsed &&
      homeFocused &&
      endFocused
    );
    await requestNativeKey("Home");
    await requestNativeKey("Enter");
  } else {
    const self = await findButton("Moved folder", dialog);
    await requestNativeClick(await findElement(
      'button[aria-label="Expand"]',
      self.closest(".move-row") ?? dialog,
    ));
    const descendant = await findButton("Moved descendant", dialog);
    descendantExcluded = descendant.disabled;
    await requestNativeClick(await findButton("Library root", dialog));
  }
  await requestNativeClick(await findButton("Move here", dialog));
  await settle(20);
  control.moveRequests[0]?.resolve({ ok: false, code: "cross_root", message: "Choose the original root" });
  await settle(40);
  const activeMoveDialog = currentDialog("move-dialog", "move-dialog-title");
  const result = {
    accessibleTreeModel,
    callCount: control.moveCalls.length,
    crossRootFailureVisible:
      activeMoveDialog?.querySelector('[role="alert"]')?.textContent === "Choose the original root",
    descendantExcluded,
    keyboardNavigation,
    rovingTabStop,
    selectedTarget: control.moveCalls[0]?.targetFolderId ?? null,
    selfExcluded,
    sharedModal,
  };
  await requestNativeClick(await findButton("Cancel", activeMoveDialog ?? document));
  await waitForDialogRemoval("move-dialog", "move-dialog-title");
  await unmountScenario(mounted);
  return result;
}

function pagedFolder(index: number) {
  return {
    hasChildren: false,
    id: 2_000 + index,
    mediaCount: 0,
    name: "Paged folder " + (index + 1),
    parentId: 1_000,
    rootId: 1,
    status: "ready",
  };
}

function MovePaginationScenario({ control }: { control: MovePaginationControl }) {
  const overlays = useOverlayStack();
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState(() =>
    Array.from({ length: 500 }, (_, index) => pagedFolder(index))
  );
  const [nextCursor, setNextCursor] = useState<string | null>("page-2");
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const library = {
    expandedFolderIds: new Set<number>([1_000]),
    folderChildNextCursorByParent: { 1_000: nextCursor },
    folderChildrenByParent: { 1_000: children },
    loadMoreFolderChildren: async () => {
      control.calls += 1;
      const request = deferred<void>();
      control.requests.push(request);
      try {
        await request.promise;
      } catch {
        // Production reports the failure globally and resolves the request.
        return;
      }
      setChildren((current) => [...current, pagedFolder(500)]);
      setNextCursor(null);
    },
    loadingMoreFolderIds: new Set<number>(),
    roots: [{ displayName: "Paged root", id: 1, rootFolderId: 1_000 }],
    toggleFolderExpanded: () => undefined,
  };

  return (
    <>
      <button
        onClick={(event) => {
          restoreFocusRef.current = event.currentTarget;
          overlays.openRoot("file-operation");
          setOpen(true);
        }}
      >
        Open paginated move
      </button>
      <MoveDialogCompat
        busy={false}
        fileIds={[41]}
        folderIds={[]}
        library={library}
        onCancel={() => setOpen(false)}
        onSubmit={(folderId: number) => {
          control.selectedTarget = folderId;
        }}
        open={open}
        restoreFocusRef={restoreFocusRef}
        serverError={null}
        serverErrorCode={null}
      />
    </>
  );
}

async function runMovePaginationScenario() {
  const control: MovePaginationControl = {
    calls: 0,
    requests: [],
    selectedTarget: null,
  };
  const mounted = await mountScenario(
    <OverlayProvider><MovePaginationScenario control={control} /></OverlayProvider>,
  );
  await requestNativeClick(await findButton("Open paginated move"));
  const dialog = await waitForDialog("move-dialog", "move-dialog-title");
  await settle(40);
  const tree = await findElement('[role="tree"]', dialog);
  const loadMore = Array.from(tree.querySelectorAll<HTMLElement>('[role="treeitem"]')).find(
    (item) => item.textContent?.includes("Load more children"),
  );
  if (!(loadMore instanceof HTMLButtonElement)) {
    const result = {
      busyVisible: false,
      duplicateBlocked: false,
      failureVisible: false,
      loadedBeyondFirstPage: false,
      selectedBeyondFirstPage: false,
    };
    await unmountScenario(mounted);
    return result;
  }

  const firstItem = tree.querySelector<HTMLElement>('[role="treeitem"]');
  firstItem?.focus();
  await requestNativeKey("End");
  const loadMoreKeyboardFocused = document.activeElement === loadMore;
  await requestNativeKey("Enter");
  await requestNativeKey("Enter");
  await settle(30);
  const busyVisible =
    loadMoreKeyboardFocused &&
    loadMore.getAttribute("aria-disabled") === "true" &&
    loadMore.textContent?.includes("Loading") === true;
  const duplicateBlocked = control.calls === 1;
  control.requests[0]?.reject(new Error("Page denied"));
  await settle(40);
  const failureVisible =
    dialog.querySelector<HTMLElement>('[role="alert"]')?.textContent?.includes("Could not load") === true;
  const retry = Array.from(tree.querySelectorAll<HTMLButtonElement>('[role="treeitem"]')).find(
    (item) => item.textContent?.includes("Load more children"),
  );
  retry?.focus();
  await requestNativeKey("Enter");
  await settle(20);
  control.requests[1]?.resolve();
  await settle(50);
  const pageItem = Array.from(tree.querySelectorAll<HTMLButtonElement>('[role="treeitem"]')).find(
    (item) => item.textContent?.trim() === "Paged folder 501",
  );
  const loadedBeyondFirstPage = pageItem instanceof HTMLButtonElement;
  pageItem?.focus();
  await requestNativeKey("Enter");
  await settle(20);
  await requestNativeClick(await findButton("Move here", dialog));
  const result = {
    busyVisible,
    duplicateBlocked,
    failureVisible,
    loadedBeyondFirstPage,
    selectedBeyondFirstPage: control.selectedTarget === 2_500,
  };
  await unmountScenario(mounted);
  return result;
}

async function runDeleteScenario() {
  const { control, mounted } = await mountOperationScenario();
  const permanentTrigger = await findButton("Open permanent");
  await requestNativeClick(permanentTrigger);
  const dialog = await waitForDialog("delete-dialog", "delete-dialog-title");
  await settle(30);
  const input = await findElement("input", dialog) as HTMLInputElement;
  const permanentInputFocused = document.activeElement === input;
  const deleteButton = await findButton("Delete permanently", dialog);
  const disabledBefore = deleteButton.disabled;
  await act(async () => setInputValue(input, " 1 "));
  await settle();
  const typedConfirmationRequired = disabledBefore && !deleteButton.disabled;
  await requestNativeClick(deleteButton);
  await requestNativeClick(deleteButton);
  await settle(20);
  const sharedAlertDialog =
    dialog.dataset.overlayLayerKind === "modal" && dialog.getAttribute("role") === "alertdialog";
  const cancel = await findButton("Cancel", dialog);
  await requestNativeKey("Escape");
  await requestNativePoint(4, 4);
  await act(async () => cancel.click());
  await settle(20);
  const busyBlockedDismissal = currentDialog("delete-dialog", "delete-dialog-title") !== null;
  const duplicateBlocked = control.deleteCalls.length === 1;
  control.deleteRequests[0]?.resolve({ ok: false, message: "Delete denied" });
  await settle(40);
  const failureVisible =
    currentDialog("delete-dialog", "delete-dialog-title")
      ?.querySelector('[role="alert"]')?.textContent === "Delete denied";
  await requestNativeClick(await findButton(
    "Cancel",
    currentDialog("delete-dialog", "delete-dialog-title") ?? document,
  ));
  await waitForDialogRemoval("delete-dialog", "delete-dialog-title");
  const restoredFocus = document.activeElement === permanentTrigger;

  const recycleTrigger = await findButton("Open recycle");
  await requestNativeClick(recycleTrigger);
  const recycleDialog = await waitForDialog("delete-dialog", "delete-dialog-title");
  await settle(30);
  const recycleCancel = await findButton("Cancel", recycleDialog);
  const recycleCancelFocused = document.activeElement === recycleCancel;
  await requestNativeClick(await findButton("Move to recycle bin", recycleDialog));
  await settle(20);
  control.deleteRequests[1]?.resolve({ ok: true });
  await waitForDialogRemoval("delete-dialog", "delete-dialog-title");
  const recycleClosedOnSuccess = currentDialog("delete-dialog", "delete-dialog-title") === null;

  const result = {
    busyBlockedDismissal,
    callCount: control.deleteCalls.length,
    duplicateBlocked,
    failureVisible,
    permanentInputFocused,
    recycleCancelFocused,
    recycleClosedOnSuccess,
    restoredFocus,
    sharedAlertDialog,
    typedConfirmationRequired,
  };
  await unmountScenario(mounted);
  return result;
}

async function run(): Promise<void> {
  try {
    lifecycleWindow.__overlayLifecycleResult = {
      contextMenu: await runContextMenuScenario(),
      move: await runMoveScenario(),
      movePagination: await runMovePaginationScenario(),
      ok: true,
      remove: await runDeleteScenario(),
      rename: await runRenameScenario(),
      shortcuts: await runShortcutScenario(),
    };
  } catch (error) {
    lifecycleWindow.__overlayLifecycleResult = {
      error: error instanceof Error ? error.stack ?? error.message : String(error),
      ok: false,
    };
  }
}

void run();
`;

async function runFixture(url: string): Promise<FileOperationOverlayResult> {
  const electronModule = await import("electron");
  const electronPath = electronModule.default as unknown as string;
  const environment = { ...process.env };
  delete environment.ELECTRON_RUN_AS_NODE;

  return await new Promise<FileOperationOverlayResult>((resolve, reject) => {
    const child = spawn(electronPath, [electronRunner, url], {
      cwd: webRoot,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill();
      reject(new Error(`file-operation overlay fixture timed out\n${stderr}`));
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
            `file-operation overlay fixture exited ${code} without a result\n${stdout}\n${stderr}`,
          ),
        );
        return;
      }
      const payload = stdout.slice(markerIndex + resultPrefix.length).trim();
      resolve(JSON.parse(payload) as FileOperationOverlayResult);
    });
  });
}

before(async () => {
  mkdirSync(fixtureDirectory, { recursive: true });
  writeFileSync(
    fixtureHtmlPath,
    '<!doctype html><html><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1.0"><title>File operation overlays</title></head><body><script type="module" src="/\.tmp/file-operation-overlays/fixture.tsx"></script></body></html>',
  );
  writeFileSync(fixtureSourcePath, fixtureSource);
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
  mountedResult = await runFixture(
    `http://127.0.0.1:${address.port}/.tmp/file-operation-overlays/fixture.html`,
  );
  assert.equal(mountedResult.ok, true, mountedResult.error);
});

after(async () => {
  await server?.close();
  if (dirname(fixtureSourcePath) === fixtureDirectory) {
    rmSync(fixtureDirectory, { recursive: true, force: true });
  }
});

describe("file-operation overlay migration", () => {
  const appSource = read("src/app/App.tsx");
  const shellSource = read("src/app-shell/ShellOverlayHost.tsx");
  const contextSource = read("src/features/file-ops/ContextMenu.tsx");
  const renameSource = read("src/features/file-ops/RenameDialog.tsx");
  const moveSource = read("src/features/file-ops/MoveDialog.tsx");
  const deleteSource = read("src/features/file-ops/DeleteConfirm.tsx");
  const shortcutsSource = read("src/features/shortcuts/useShortcuts.ts");
  const overlayStyles = read("src/design/overlays/overlay.css");

  it("moves the context menu onto the shared point-anchored root", () => {
    assert.match(contextSource, /pointOverlayAnchor/);
    assert.match(contextSource, /AnchoredOverlay/);
    assert.match(contextSource, /OverlayMenuList/);
    assert.doesNotMatch(contextSource, /window\.addEventListener\("mousedown"/);
    assert.match(appSource, /openRoot\("context-menu"\)/);
    assert.match(appSource, /contextMenuReturnFocusRef/);
    assert.match(shellSource, /restoreFocusTarget=\{contextMenuReturnFocusRef\.current\}/);
  });

  it("switches context-menu ownership to one file-operation root", () => {
    assert.match(appSource, /dismissContextMenuState/);
    assert.match(appSource, /beginFileOperation/);
    assert.match(shortcutsSource, /openRoot\("file-operation"\)/);
    assert.match(appSource, /previousFileOperationOpenRef/);
    assert.match(shellSource, /onDismissContextMenu/);
  });

  it("routes production rename and delete shortcuts through shared root ownership", () => {
    assert.match(appSource, /useShortcuts\(\{[\s\S]*fileOperationRestoreFocusRef/);
    assert.match(
      shortcutsSource,
      /openRoot\("file-operation"\)[\s\S]*onDismissContextMenu\(\)[\s\S]*openOperation\(\)/,
    );
    assert.match(shortcutsSource, /fileOperationRestoreFocusRef\.current/);
    assert.match(shortcutsSource, /onDismissContextMenu/);
  });

  it("uses shared modal, field, status, and focus primitives", () => {
    for (const source of [renameSource, moveSource, deleteSource]) {
      assert.match(source, /ModalSurface/);
      assert.match(source, /rootId="file-operation"/);
      assert.match(source, /restoreFocusRef/);
      assert.doesNotMatch(source, /\.\/useFocusTrap/);
    }
    assert.match(renameSource, /OverlayField/);
    assert.match(renameSource, /error=\{displayedError\}/);
    assert.match(renameSource + moveSource + deleteSource, /OverlayInlineStatus/);
    assert.doesNotMatch(
      renameSource + moveSource + deleteSource,
      /className="dialog-backdrop"/,
    );
  });

  it("preserves destructive confirmation and move exclusions", () => {
    assert.match(deleteSource, /confirmText\.trim\(\) === expected/);
    assert.match(deleteSource, /busy=\{busy\}/);
    assert.match(deleteSource, /role="alertdialog"/);
    assert.match(moveSource, /excludedFolderIds/);
    assert.match(moveSource, /serverErrorCode === "cross_root"/);
  });

  it("implements paginated ARIA tree navigation for move targets", () => {
    assert.match(moveSource, /folderChildNextCursorByParent/);
    assert.match(moveSource, /loadMoreFolderChildren/);
    assert.match(moveSource, /loadingMoreFolderIds/);
    assert.match(moveSource, /aria-expanded/);
    assert.match(moveSource, /aria-level/);
    assert.match(moveSource, /tabIndex/);
    assert.match(moveSource, /ArrowDown/);
    assert.match(moveSource, /ArrowRight/);
    assert.match(moveSource, /Home/);
    assert.match(moveSource, /End/);
  });

  it("owns menu and dialog styling without local hover fills", () => {
    const section = overlayStyles.split("/* Context menus and file dialogs */")[1] ?? "";
    assert.notEqual(section, "");
    assert.match(section, /\.context-menu/);
    assert.match(section, /\.overlay-dialog-wide/);
    assert.match(section, /\.overlay-field-error/);
    assert.match(section, /\.move-tree/);
    assert.doesNotMatch(section, /:hover[^\{]*\{[^}]*\b(?:background|border(?:-color)?)\s*:/s);
  });
});

describe("mounted file-operation overlays", () => {
  it("clamps the shared context root and maps keyboard actions across separators", () => {
    assert.deepEqual(mountedResult.contextMenu, {
      activatedItem: "omega",
      clampedToViewport: true,
      focusedAcrossSeparator: true,
      restoredFocus: true,
      sharedRoot: true,
      singleMenuRole: true,
    });
  });

  it("keeps rename validation, busy ownership, failure, and focus behavior", () => {
    assert.deepEqual(mountedResult.rename, {
      busyBlockedDismissal: true,
      callCount: 1,
      failureVisible: true,
      fieldErrorLinked: true,
      invalidBlocked: true,
      restoredFocus: true,
      selectedStem: true,
      sharedModal: true,
    });
  });

  it("keeps move exclusions, selected target, and cross-root failure behavior", () => {
    assert.deepEqual(mountedResult.move, {
      accessibleTreeModel: true,
      callCount: 1,
      crossRootFailureVisible: true,
      descendantExcluded: true,
      keyboardNavigation: true,
      rovingTabStop: true,
      selectedTarget: 1,
      selfExcluded: true,
      sharedModal: true,
    });
  });

  it("loads, retries, and selects folder targets beyond the first 500-item page", () => {
    assert.deepEqual(mountedResult.movePagination, {
      busyVisible: true,
      duplicateBlocked: true,
      failureVisible: true,
      loadedBeyondFirstPage: true,
      selectedBeyondFirstPage: true,
    });
  });

  it("keeps permanent and recycle deletion busy, deduplicated, and recoverable", () => {
    assert.deepEqual(mountedResult.remove, {
      busyBlockedDismissal: true,
      callCount: 2,
      duplicateBlocked: true,
      failureVisible: true,
      permanentInputFocused: true,
      recycleCancelFocused: true,
      recycleClosedOnSuccess: true,
      restoredFocus: true,
      sharedAlertDialog: true,
      typedConfirmationRequired: true,
    });
  });

  it("opens F2 and Delete through the production shortcut root with fresh focus ownership", () => {
    assert.deepEqual(mountedResult.shortcuts, {
      deleteOwnerReplaced: true,
      deleteRootOwnedBeforeOpen: true,
      renameOwnerReplaced: true,
      renameRootOwnedBeforeOpen: true,
      staleContextOwnerCleared: true,
    });
  });
});

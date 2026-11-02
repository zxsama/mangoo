import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { AnchoredOverlay, OverlayProvider } from "../../src/design/overlays";
import { useOverlayScrollbars } from "../../src/app-shell/useOverlayScrollbars";
import { resolveOverlayScrollbarOwner } from "../../src/design/overlays/overlayScrollbarOwner";
import "../../src/styles.css";

const bridge = window as unknown as {
  __overlayLifecycleInputRequest?: Record<string, unknown>;
  __overlayLifecycleInputResponseId?: number;
  __overlayLifecycleResult?: unknown;
};
let inputId = 0;
const delay = (ms = 40) => new Promise<void>((resolve) => setTimeout(resolve, ms));
async function input(payload: Record<string, unknown>) {
  const id = ++inputId;
  bridge.__overlayLifecycleInputRequest = { ...payload, id };
  for (let i = 0; i < 150; i++) {
    if (bridge.__overlayLifecycleInputResponseId === id) return;
    await delay(10);
  }
  throw new Error(`Native input ${id} timed out`);
}
function check(value: unknown, message: string): void {
  if (!value) throw new Error(message);
}
function get(selector: string) {
  const element = document.querySelector<HTMLElement>(selector);
  if (!element) throw new Error(`Missing ${selector}`);
  return element;
}
function Controller() { useOverlayScrollbars(); return null; }
let rootDismissals = 0;
let childDismissals = 0;
let setController: (enabled: boolean) => void;
let openChild: () => void;
function Scenario() {
  const [controller, updateController] = useState(true);
  const [parent, setParent] = useState(false);
  const [child, setChild] = useState(false);
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [childAnchor, setChildAnchor] = useState<HTMLButtonElement | null>(null);
  setController = updateController;
  openChild = () => setChild(true);
  const viewport = (id: string) => <div id={id} style={{ overflow: "auto", width: 180, height: 130 }}>
    <div style={{ width: 400, height: 900 }}>Scrollable content</div>
  </div>;
  return <>
    {controller && <Controller />}
    <button id="launcher" ref={setAnchor} onClick={() => setParent(true)} style={{ position: "fixed", left: 40, top: 20 }}>Launcher</button>
    <button id="outside" style={{ position: "fixed", left: 650, top: 400 }}>Outside</button>
    <div style={{ position: "fixed", left: 540, top: 60 }}>{viewport("background-scroll")}</div>
    <AnchoredOverlay anchor={anchor} ariaLabel="Parent" id="parent" rootId="library" open={parent}
      preferredWidth={220} onDismiss={() => { rootDismissals++; setParent(false); }}>
      <button id="child-launcher" ref={setChildAnchor}>Child launcher</button>
      {viewport("parent-scroll")}
    </AnchoredOverlay>
    <AnchoredOverlay anchor={childAnchor} ariaLabel="Child" id="child" rootId="library" parentId="parent"
      placement="right-start" open={child} preferredWidth={220}
      onDismiss={() => { childDismissals++; setChild(false); }}>
      {viewport("child-scroll")}
    </AnchoredOverlay>
  </>;
}

async function activate(owner: HTMLElement, axis = "vertical") {
  owner.scrollTop = 1;
  owner.scrollLeft = 1;
  owner.dispatchEvent(new Event("scroll"));
  await delay();
  const rail = get(`.mangoo-overlay-scrollbar-${axis}`);
  rail.dispatchEvent(new PointerEvent("pointerenter"));
  await delay();
  check(rail.parentElement === document.body, "Regression must use the real body-mounted rail");
  check(rail.dataset.scrollbarVisible === "true", `Expected visible scrollbar: ${JSON.stringify({ id: owner.id, connected: owner.isConnected, height: owner.clientHeight, scrollHeight: owner.scrollHeight, rects: owner.getClientRects().length, visibility: getComputedStyle(owner).visibility, overflow: getComputedStyle(owner).overflowY, rail: rail.outerHTML })}`);
  return rail;
}
async function drag(owner: HTMLElement, axis: string, onThumb: boolean) {
  const rail = await activate(owner, axis);
  const thumb = get(`.mangoo-overlay-scrollbar-${axis} .mangoo-overlay-scrollbar-thumb`);
  const rect = (onThumb ? thumb : rail).getBoundingClientRect();
  const x = Math.round(rect.left + (onThumb ? rect.width / 2 : 1));
  const y = Math.round(rect.top + (onThumb ? rect.height / 2 : 1));
  check(document.elementFromPoint(x, y) === (onThumb ? thumb : rail), "Native pointer must hit the requested thumb or rail");
  const before = axis === "vertical" ? owner.scrollTop : owner.scrollLeft;
  const focused = document.activeElement;
  await input({ type: "mouse-drag", x, y, endX: x + (axis === "horizontal" ? 35 : 0), endY: y + (axis === "vertical" ? 35 : 0) });
  check(rootDismissals === 0 && childDismissals === 0, "Owned scrollbar pointerdown dismissed its popup");
  check((axis === "vertical" ? owner.scrollTop : owner.scrollLeft) > before + 20, "Scrollbar drag must change its owner's scroll offset");
  check(document.activeElement === focused, "Dragging must preserve focus");
  check(rail.dataset.scrollbarDragging === "false", "Drag capture must end on pointerup");
  rail.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
  check(rootDismissals === 0 && childDismissals === 0, "Owned scrollbar context menu must preserve its popup");
}

async function clickRail(rail: HTMLElement) {
  const rect = rail.getBoundingClientRect();
  const x = Math.round(rect.left + 1);
  const y = Math.round(rect.top + 30);
  check(document.elementFromPoint(x, y) === rail, "Outside comparison must hit the actual rail");
  await input({ type: "mouse-click", x, y });
}

async function run() {
  // Let the shared Electron runner finish its initial window focus first.
  await delay(250);
  const root = createRoot(get("#root"));
  try {
    await act(async () => root.render(<OverlayProvider><Scenario /></OverlayProvider>));
    await act(async () => get("#launcher").click());
    await delay(150);
    for (let i = 0; i < 50 && getComputedStyle(get("#parent-scroll")).visibility === "hidden"; i++) await delay();
    get("#child-launcher").focus();
    await drag(get("#parent-scroll"), "vertical", true);
    await drag(get("#parent-scroll"), "vertical", false);
    await drag(get("#parent-scroll"), "horizontal", true);
    await drag(get("#parent-scroll"), "horizontal", false);
    await act(async () => openChild());
    await delay(150);
    await drag(get("#child-scroll"), "vertical", true);
    await drag(get("#child-scroll"), "vertical", false);
    // The parent's rail is outside the child, but still inside its parent family.
    const parentRail = await activate(get("#parent-scroll"));
    await clickRail(parentRail);
    await delay(150);
    check(childDismissals === 1 && rootDismissals === 0, "Parent rail must dismiss only the child");
    document.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
    const backgroundRail = await activate(get("#background-scroll"));
    await clickRail(backgroundRail);
    await delay(150);
    check(rootDismissals === 1, "An unrelated scrollbar must dismiss the popup");
    document.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
    const owner = get("#background-scroll");
    const imitation = backgroundRail.cloneNode(true) as HTMLElement;
    document.body.append(imitation);
    check(resolveOverlayScrollbarOwner(imitation) === null, "Copied DOM attributes must not grant scrollbar ownership");
    imitation.remove();
    owner.style.visibility = "hidden";
    check(resolveOverlayScrollbarOwner(backgroundRail) === null, "Hidden owner must not retain containment");
    owner.remove();
    check(resolveOverlayScrollbarOwner(backgroundRail) === null, "Disconnected owner must not retain containment");
    await act(async () => setController(false));
    check(document.querySelectorAll(".mangoo-overlay-scrollbar").length === 0, "Controller disposal must remove both rails");
    document.body.append(backgroundRail);
    check(resolveOverlayScrollbarOwner(backgroundRail) === null, "Disposed rail must not retain registration");
    backgroundRail.remove();
    // A fresh mount must still dismiss on a normal outside pointer.
    await act(async () => root.render(<OverlayProvider key="fresh"><Scenario /></OverlayProvider>));
    await act(async () => get("#launcher").click());
    await delay(150);
    const outside = get("#outside").getBoundingClientRect();
    await input({ type: "mouse-click", x: Math.round(outside.left + 10), y: Math.round(outside.top + 10) });
    check(rootDismissals === 2, "Normal outside clicks must dismiss");
    bridge.__overlayLifecycleResult = { ok: true };
  } catch (error) {
    bridge.__overlayLifecycleResult = { ok: false, error: error instanceof Error ? error.stack : String(error) };
  } finally {
    await act(async () => root.unmount());
  }
}
void run();

import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import { ModalSurface, OverlayDialogBody, OverlayDialogHeader, OverlayProvider, OverlaySelect } from "../../src/design/overlays";
import { OverlaySelect as SdkSelect } from "../../../../packages/plugin-sdk/src/design";
import { useOverlayScrollbars } from "../../src/app-shell/useOverlayScrollbars";
import { applyInterfaceStyleVariables, DEFAULT_INTERFACE_STYLE } from "../../src/features/settings/interfaceStyle";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

type Input = { type: "key"; key: string; shift?: boolean } | { type: "click" | "move"; x: number; y: number } | { type: "capture"; name: string } | { type: "motion"; reduced: boolean };
const host = window as typeof window & {
  integratedSelectInput?: Input & { id: number };
  integratedSelectAck?: number;
  integratedSelectResult?: unknown;
};
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const settle = async (ms = 80) => act(async () => { await wait(ms); });
let serial = 0;
async function input(payload: Input) {
  const id = ++serial;
  host.integratedSelectInput = { ...payload, id };
  await act(async () => {
    for (let i = 0; i < 300; i++) { if (host.integratedSelectAck === id) return; await wait(10); }
    throw new Error(`Input timeout: ${JSON.stringify(payload)}`);
  });
  await settle();
}
const key = (key: string, shift = false) => input({ type: "key", key, shift });
const capture = (name: string) => input({ type: "capture", name });
function get<T extends HTMLElement = HTMLElement>(selector: string): T {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`Missing ${selector}`);
  return node;
}
function check(value: unknown, message: string): void { if (!value) throw new Error(message); }
async function pointer(node: HTMLElement, type: "click" | "move" = "click") {
  const rect = node.getBoundingClientRect();
  await input({ type, x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) });
}
function options() { return Array.from(document.querySelectorAll<HTMLElement>('[role="option"]')); }
function option(text: string) { const node = options().find((node) => node.textContent?.includes(text)); if (!node) throw new Error(`Missing option ${text}`); return node; }
function popup() { return get<HTMLElement>('.overlay-select-popup'); }
const cases: string[] = [];
const evidence: Record<string, unknown> = {};
const changes: { value: string; target: boolean; currentTarget: boolean; name: string }[] = [];
function paintedOpacity(node: HTMLElement | null): number {
  if (!node || !node.isConnected) return 0;
  const rect = node.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0 || getComputedStyle(node).visibility !== "visible") return 0;
  let opacity = 1;
  for (let current: HTMLElement | null = node; current; current = current.parentElement) {
    const style = getComputedStyle(current);
    if (style.display === "none" || style.contentVisibility === "hidden") return 0;
    opacity *= Number(style.opacity);
  }
  return opacity;
}
async function openingFrames(select: HTMLSelectElement, name: string) {
  const frames: { expanded: boolean; controlOpacity: number; popupVisible: boolean; phase?: string; headerDelta?: number; headerHeight?: number; rowHeight?: number; rowPadding?: string; listPadding?: string }[] = [];
  let frameId = 0;
  const record = () => {
    const control = select.closest<HTMLElement>(".overlay-select-control")!;
    const surface = document.getElementById(`${select.id}-popup`);
    const header = surface?.querySelector<HTMLElement>(".overlay-select-header");
    const row = surface?.querySelector<HTMLElement>(".overlay-select-option");
    const list = surface?.querySelector<HTMLElement>(".overlay-select-options");
    frames.push({ expanded: select.getAttribute("aria-expanded") === "true", controlOpacity: paintedOpacity(control),
      popupVisible: paintedOpacity(surface) > 0 && paintedOpacity(header ?? null) > 0, phase: surface?.dataset.overlayState,
      headerDelta: header ? header.getBoundingClientRect().top - select.getBoundingClientRect().top : undefined,
      headerHeight: header?.getBoundingClientRect().height, rowHeight: row?.getBoundingClientRect().height,
      rowPadding: row ? getComputedStyle(row).padding : undefined, listPadding: list ? getComputedStyle(list).padding : undefined });
    frameId = requestAnimationFrame(record);
  };
  frameId = requestAnimationFrame(record);
  try {
    await pointer(select);
    await settle(180);
  } finally {
    cancelAnimationFrame(frameId);
    evidence[name] = frames;
  }
  check(frames.some((frame) => frame.expanded && frame.popupVisible), `${name}: no visible opening frames captured`);
  check(frames.every((frame) => frame.controlOpacity === 1 || frame.popupVisible), `${name}: opening left both trigger and popup invisible`);
  check(frames.every((frame) => !frame.popupVisible || (frame.controlOpacity === 0 && Math.abs(frame.headerDelta ?? Infinity) < 1 && frame.headerHeight === select.getBoundingClientRect().height)), `${name}: first visible header detached or duplicated the trigger`);
  check(frames.filter((frame) => frame.popupVisible).every((frame) => frame.rowHeight === 24 && frame.rowPadding === "3px 8px" && frame.listPadding === "2px 4px"), `${name}: option spacing is not compact`);
  await key("Escape"); await settle(180);
  return frames;
}
let update: (action: "dynamic" | "modal" | "unmount" | "disable" | "enable") => void;

function Scenario() {
  useOverlayScrollbars();
  const [controlled, setControlled] = useState("alpha");
  const [dynamic, setDynamic] = useState(false);
  const [modal, setModal] = useState(false);
  const [mounted, setMounted] = useState(true);
  const [disabled, setDisabled] = useState(false);
  update = (action) => {
    if (action === "dynamic") setDynamic(true);
    if (action === "modal") setModal(true);
    if (action === "unmount") setMounted(false);
    if (action === "disable") setDisabled(true);
    if (action === "enable") setDisabled(false);
  };
  return <LiquidGlassLayer><main style={{ padding: 32, height: "100vh", background: "linear-gradient(135deg, #202e40, #57495f)", color: "white" }}>
    <h1>Shared integrated dropdown</h1>
    <form id="select-form" style={{ display: "grid", gap: 14, width: 280 }}>
      <input id="before" aria-label="Before" defaultValue="Previous field" />
      <OverlaySelect id="controlled" name="controlled" label="Controlled choice" value={controlled} disabled={disabled} onChange={(event) => {
        changes.push({ value: event.currentTarget.value, target: event.target instanceof HTMLSelectElement, currentTarget: event.currentTarget instanceof HTMLSelectElement, name: event.currentTarget.name });
        setControlled(event.target.value);
      }}>
        <option value="">No choice</option><optgroup label="Available"><option value="alpha">Alpha</option><option value="beta" disabled>Beta disabled</option><option value="charlie">Charlie</option></optgroup>
        <optgroup label="Disabled group" disabled><option value="delta">Delta disabled group</option></optgroup>
        {dynamic ? <option value="echo">Echo added</option> : <option value="zulu">Zulu</option>}
      </OverlaySelect>
      <input id="after" aria-label="After" defaultValue="Following field" />
      <OverlaySelect id="uncontrolled" name="uncontrolled" label="Uncontrolled" defaultValue="charlie"><option value="alpha">Alpha</option><option value="charlie">Charlie</option></OverlaySelect>
      <OverlaySelect id="empty" name="empty" label="Empty default" defaultValue=""><option value="">Choose a value</option><option value="alpha">Alpha</option></OverlaySelect>
      <button type="reset" id="reset">Reset form</button><button type="button" id="outside">Outside action</button>
    </form>
    <div style={{ position: "fixed", right: 24, top: 32, width: 260, display: "grid", gap: 14 }}>
      <OverlaySelect id="no-options" label="No options" />
      <OverlaySelect id="all-disabled" label="Unavailable choices"><option disabled>Not available</option></OverlaySelect>
      <OverlaySelect id="pinned" label="Controlled owner keeps value" value="alpha" onChange={() => {}}><option value="alpha">Alpha</option><option value="charlie">Charlie</option></OverlaySelect>
    </div>
    {mounted ? <div style={{ position: "fixed", right: 16, bottom: 16, width: 260 }}>
      <SdkSelect id="edge" label="SDK long choices" defaultValue="1">{Array.from({ length: 45 }, (_, index) => <option key={index} value={index + 1}>Choice {String(index + 1).padStart(2, "0")}</option>)}</SdkSelect>
    </div> : null}
    <ModalSurface id="plugin-choice-modal" rootId="plugin-view" ariaLabelledBy="plugin-title" open={modal} onDismiss={() => setModal(false)}>
      <OverlayDialogHeader title="Plugin configuration" titleId="plugin-title" />
      <OverlayDialogBody style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 14 }}><input id="modal-before" style={{ gridColumn: 1 }} aria-label="Application name" defaultValue="Sample plugin" />
        <div style={{ gridColumn: 1 }}>
        <OverlaySelect id="modal-select" label="Account type" defaultValue="personal"><option value="personal">Personal account</option><option value="business">Organization account</option><option disabled value="unavailable">Unavailable account</option></OverlaySelect>
        </div>
        <input id="modal-after" style={{ gridColumn: 1 }} aria-label="Application ID" defaultValue="Next configuration field" />
        <div style={{ gridColumn: 2, gridRow: 2 }}>
        <OverlaySelect id="modal-sibling" label="Region" defaultValue="local"><option value="local">Local region</option><option value="remote">Remote region</option></OverlaySelect>
        </div>
      </OverlayDialogBody>
    </ModalSurface>
  </main></LiquidGlassLayer>;
}

async function run() {
  applyInterfaceStyleVariables(DEFAULT_INTERFACE_STYLE);
  const root = createRoot(get("#root"));
  await act(async () => root.render(<OverlayProvider><Scenario /></OverlayProvider>));
  await settle(200);
  const select = get<HTMLSelectElement>("#controlled");
  await input({ type: "motion", reduced: false });
  await openingFrames(select, "opening-below-first");
  await openingFrames(select, "opening-below-repeat");
  await openingFrames(get<HTMLSelectElement>("#edge"), "opening-above");
  await input({ type: "motion", reduced: true });
  await openingFrames(select, "opening-reduced-below");
  await openingFrames(get<HTMLSelectElement>("#edge"), "opening-reduced-above");
  await input({ type: "motion", reduced: false });
  cases.push("stable first visible frames and compact spacing in both motion modes");
  check(get<HTMLSelectElement>("#uncontrolled").value === "charlie" && get<HTMLSelectElement>("#empty").value === "", "Default values lost");
  check(new FormData(get<HTMLFormElement>("#select-form")).get("controlled") === "alpha", "Native form value missing");
  check(select.labels?.[0]?.textContent === "Controlled choice", "Native label association lost");
  cases.push("native forms, labels and defaults");

  for (const id of ["no-options", "all-disabled"]) {
    const target = get<HTMLSelectElement>(`#${id}`);
    const previousValue = target.value;
    await pointer(target); await key("Home"); await key("End"); await key("ArrowDown"); await key("Enter"); await key("Escape");
    check(target.value === previousValue, "Empty/disabled choices unexpectedly committed");
  }
  const pinned = get<HTMLSelectElement>("#pinned");
  await pointer(pinned); await pointer(option("Charlie"));
  check(pinned.value === "alpha", "Controlled owner refusal did not restore value");
  cases.push("empty choices, all-disabled choices and controlled ownership");

  await pointer(select);
  check(select.getAttribute("aria-expanded") === "true" && document.activeElement === select, "Pointer opening must preserve select focus");
  check(options().length === 6 && option("Beta").getAttribute("aria-disabled") === "true" && option("Delta").getAttribute("aria-disabled") === "true", "Disabled option/group projection wrong");
  await pointer(option("Beta"));
  check(select.value === "alpha" && changes.length === 0, "Disabled option committed");
  await pointer(option("Charlie"));
  check(select.value === "charlie" && changes.length === 1 && changes[0].target && changes[0].currentTarget && changes[0].name === "controlled", "Change callback is not a real select event");
  cases.push("controlled selection and real HTMLSelectElement change event");
  cases.push("disabled options and optgroups");

  await pointer(select);
  await key("Home"); await key("ArrowDown"); await key("ArrowDown"); await key("Enter");
  check(select.value === "charlie", "Arrows did not skip disabled options/group");
  await pointer(select); await key("End"); await key("Enter");
  check(select.value === "zulu", "End did not commit last choice");
  await pointer(select); await key("a"); await key("Enter");
  check(select.value === "alpha", "Typeahead did not choose Alpha");
  await pointer(select); await key("End"); await key("Escape");
  check(select.value === "alpha" && select.getAttribute("aria-expanded") === "false", "Escape committed or left popup open");
  cases.push("arrows, Home, End, typeahead, Enter and Escape");

  await pointer(select); await key("Tab");
  check(document.activeElement?.id === "after" && select.getAttribute("aria-expanded") === "false", `Tab focus is ${document.activeElement?.id}`);
  select.focus(); await pointer(select); await key("Tab", true);
  check(document.activeElement?.id === "before", `Shift Tab focus is ${document.activeElement?.id}`);
  await pointer(select); await pointer(get("#outside"));
  check(select.getAttribute("aria-expanded") === "false", "Outside pointer did not dismiss");
  cases.push("Tab, Shift Tab and outside dismissal");

  await pointer(select); await act(async () => update("dynamic")); await settle();
  check(options().some((node) => node.textContent?.includes("Echo")) && !options().some((node) => node.textContent === "Zulu"), "Open popup has stale dynamic options");
  await pointer(option("Echo")); check(select.value === "echo", "Dynamic option cannot commit");
  await act(async () => update("disable")); await settle(); await pointer(select);
  check(select.getAttribute("aria-expanded") !== "true" && select.disabled, "Disabled select opened");
  await act(async () => update("enable")); await settle();
  cases.push("dynamic options and disabled control");

  const uncontrolled = get<HTMLSelectElement>("#uncontrolled");
  await pointer(uncontrolled); await pointer(option("Alpha"));
  check(uncontrolled.value === "alpha", "Uncontrolled choice did not commit");
  const empty = get<HTMLSelectElement>("#empty");
  await pointer(empty); await pointer(option("Alpha"));
  await pointer(get("#reset"));
  check(uncontrolled.value === "charlie" && empty.value === "", "Form reset did not restore native defaults");
  check(uncontrolled.parentElement?.textContent?.includes("Charlie") && empty.parentElement?.textContent?.includes("Choose a value"), "Form reset display stale");
  cases.push("uncontrolled selection, empty value and form reset");

  await pointer(select); await pointer(pinned);
  check(select.getAttribute("aria-expanded") === "false" && pinned.getAttribute("aria-expanded") === "true", "Sibling click did not transfer open dropdown");
  await settle(250);
  check(document.querySelectorAll('.overlay-select-popup').length === 1, "Sibling transfer leaves two popup surfaces");
  await key("Escape");
  await pointer(select);
  await act(async () => pinned.click()); await settle(250);
  check(select.getAttribute("aria-expanded") === "false" && pinned.getAttribute("aria-expanded") === "true" && document.activeElement === pinned && document.querySelectorAll('.overlay-select-popup').length === 1, "Click activation without pointerdown did not transfer sibling ownership/focus");
  await key("Escape");
  await pointer(get('label[for="controlled"]'));
  check(document.activeElement === select && select.getAttribute("aria-expanded") === "true", "Native label activation did not focus/open select");
  await key("Escape");

  await pointer(select); await pointer(option("Alpha"), "move"); await settle(160);
  const joined = popup();
  const control = select.closest<HTMLElement>(".overlay-select-control"); if (!control) throw new Error("Missing control shell");
  const joinedRect = joined.getBoundingClientRect(), controlRect = control.getBoundingClientRect();
  check(Math.abs(joinedRect.top - controlRect.top) < 2 && Math.abs(joinedRect.left - controlRect.left) < 2, "Below popup does not start over trigger");
  check(joined.matches('[data-interactive-pointer-owner="true"][data-interactive-pointer-target]'), "Joined popup not claimed by global pointer driver");
  check(joined.querySelectorAll('[data-interactive-pointer-target]').length === 0, "Rows/header have independent pointer targets");
  check(control.dataset.pointerGlow === "off" && Number(getComputedStyle(control).getPropertyValue("--interactive-pointer-opacity")) === 0 && getComputedStyle(control).boxShadow === "none", `Covered control still paints a focus shadow or pointer ring: ${control.outerHTML}; shadow=${getComputedStyle(control).boxShadow}`);
  check(options().every((node) => getComputedStyle(node).outlineStyle === "none" && getComputedStyle(node).borderTopWidth === "0px"), "Rows have individual rings");
  evidence.joined = { control: controlRect.toJSON(), popup: joinedRect.toJSON(), borderRadius: getComputedStyle(joined).borderRadius, pointerOwner: true };
  await capture("01-integrated-below"); await key("Escape");
  cases.push("joined geometry and single global pointer owner");

  const edge = get<HTMLSelectElement>("#edge"); await pointer(edge);
  const edgeRect = edge.closest<HTMLElement>(".overlay-select-control")!.getBoundingClientRect();
  const upRect = popup().getBoundingClientRect();
  check(upRect.top < edgeRect.top && Math.abs(upRect.bottom - edgeRect.bottom) < 2 && upRect.top >= 10 && upRect.right <= innerWidth - 10, "Edge popup failed viewport-aware upward alignment");
  const upHeader = popup().querySelector<HTMLElement>(".overlay-select-header")!.getBoundingClientRect();
  check(Math.abs(upHeader.top - edgeRect.top) < 2 && Math.abs(upHeader.height - edgeRect.height) < 2, `Upward popup moved header away from original trigger: header=${JSON.stringify(upHeader)}, trigger=${JSON.stringify(edgeRect)}`);
  await key("End");
  const last = option("Choice 45").getBoundingClientRect();
  check(last.top >= upRect.top && last.bottom <= upRect.bottom + 1, "Long options keyboard navigation failed to scroll active choice into view");
  evidence.edge = { popup: upRect.toJSON(), trigger: edgeRect.toJSON(), last: last.toJSON(), sdkSameComponent: SdkSelect === OverlaySelect };
  check(SdkSelect === OverlaySelect, "SDK exports different implementation");
  await capture("02-sdk-edge-above"); await key("Enter"); check(edge.value === "45", "Long list end commit failed");
  await pointer(edge); await act(async () => update("unmount")); await settle(250);
  check(!document.querySelector('.overlay-select-popup'), "Unmount leaked popup");
  cases.push("SDK parity, long scrolling list, upward placement and unmount");

  await act(async () => update("modal")); await settle(250);
  const modalSelect = get<HTMLSelectElement>("#modal-select");
  await openingFrames(modalSelect, "opening-modal");
  await pointer(modalSelect);
  await openingFrames(get<HTMLSelectElement>("#modal-sibling"), "opening-modal-sibling");
  await pointer(modalSelect);
  check(document.querySelector('#plugin-choice-modal [aria-expanded="true"]') && document.activeElement === modalSelect, "Opening select closed modal or stole focus");
  const hit = option("Organization").getBoundingClientRect();
  check(popup().contains(document.elementFromPoint(hit.left + 15, hit.top + hit.height / 2)), "Popup is behind modal");
  await pointer(option("Organization"), "move"); await capture("03-plugin-choice-modal");
  await key("Escape"); await settle(250);
  check(document.querySelector('#plugin-choice-modal[aria-modal="true"]') && modalSelect.getAttribute("aria-expanded") === "false", "Child Escape dismissed parent modal");
  await pointer(modalSelect); await key("Tab");
  check(document.activeElement?.id === "modal-after", `Modal Tab trapped at ${document.activeElement?.id}`);
  await pointer(modalSelect); await key("Tab", true);
  check(document.activeElement?.id === "modal-before", `Modal Shift Tab trapped at ${document.activeElement?.id}`);
  await pointer(modalSelect); await pointer(option("Organization"));
  check(modalSelect.value === "business" && document.querySelector('#plugin-choice-modal'), "Modal selection closed parent");
  await pointer(modalSelect); await pointer(get("#modal-sibling")); await settle(250);
  check(document.querySelector('#plugin-choice-modal') && modalSelect.getAttribute("aria-expanded") === "false" && get("#modal-sibling").getAttribute("aria-expanded") === "true" && document.querySelectorAll('.overlay-select-popup').length === 1, "Modal sibling transfer closed parent or duplicated popup");
  await key("Escape");
  cases.push("modal parent preservation, stacking, Escape and native Tab order");
  cases.push("sibling dropdown transfer outside and inside modal");
  await key("Escape"); await settle(250);
  check(!document.querySelector('#plugin-choice-modal'), "Parent Escape after child close failed");
  await pointer(select); await key("Escape"); await pointer(select); check(select.getAttribute("aria-expanded") === "true", "Reopen failed");
  await act(async () => root.unmount()); await settle(250);
  check(!document.querySelector('.overlay-select-popup'), "Root unmount leaked popup");
  cases.push("reopening and final cleanup");
  host.integratedSelectResult = { ok: true, cases, changes, evidence };
}
run().catch((error) => { host.integratedSelectResult = { ok: false, cases, evidence, error: String(error.stack ?? error), active: document.activeElement?.outerHTML, body: document.body.innerText.slice(-3000) }; });

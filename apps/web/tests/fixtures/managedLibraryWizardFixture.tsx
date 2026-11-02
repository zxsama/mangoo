import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { ManagedLibraryWizard } from "../../src/features/library/ManagedLibraryWizard";
import { OverlayProvider } from "../../src/design/overlays";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";
import type { MangooDesktopBridge } from "../../src/core/desktop";

type PendingResponse = { resolve: (value: Response) => void; reject: (reason: Error) => void };
const calls: { path: string; body: unknown }[] = [];
const requests: PendingResponse[] = [];
const pickers: ((path: string | null) => void)[] = [];
let pauseResolution = false;
const resolutions: { resolve: (value: { databasePath: string; defaultContentPath: string }) => void; reject: (reason: Error) => void; value: { databasePath: string; defaultContentPath: string } }[] = [];
const events = { created: 0, closed: 0, failures: [] as string[], pickerNames: [] as string[] };
const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
let changeOpen: (open: boolean) => void = () => {};
let changeFirstRun: (firstRun: boolean) => void = () => {};
let resetWizard: () => void = () => {};
window.mangooDesktop = {
  coreUrl: "http://wizard-fixture.test/api", sessionToken: "wizard-fixture",
  resolveLibraryCreationPaths: async ({ name, databasePath }) => {
    const base = databasePath ? databasePath.slice(0, Math.max(databasePath.lastIndexOf("/"), databasePath.lastIndexOf("\\"))) : `C:/Documents/Mangoo/Libraries/${name}`;
    const value = { databasePath: databasePath ?? `${base}/Database`, defaultContentPath: `${base}/Files` };
    return pauseResolution ? await new Promise<typeof value>((resolve, reject) => resolutions.push({ resolve, reject, value })) : value;
  },
  pickFolder: async () => {
    events.pickerNames.push("exact-directory");
    return await new Promise<string | null>((resolve) => pickers.push(resolve));
  }
} as MangooDesktopBridge;
window.fetch = async (input, init) => {
  const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
  calls.push({ path: url.pathname, body: JSON.parse(String(init?.body ?? "null")) });
  return await new Promise<Response>((resolve, reject) => requests.push({ resolve, reject }));
};
const fixture = {
  calls, events,
  useWebBridge: () => { delete window.mangooDesktop?.pickFolder; delete window.mangooDesktop?.resolveLibraryCreationPaths; resetWizard(); },
  pauseResolution: (pause: boolean) => { pauseResolution = pause; },
  resolveAt: (index: number) => { const [request] = resolutions.splice(index, 1); if (!request) throw new Error("No pending resolution"); request.resolve(request.value); },
  rejectResolutionAt: (index: number) => { const [request] = resolutions.splice(index, 1); if (!request) throw new Error("No pending resolution"); request.reject(new Error("无法解析数据库位置，请重新选择")); },
  unmount: () => root.unmount(),
  open: (open: boolean, firstRun = false) => { changeFirstRun(firstRun); changeOpen(open); },
  pick: (path: string | null) => { const resolve = pickers.shift(); if (!resolve) throw new Error("No pending picker"); resolve(path); },
  respond: (body: unknown) => { const request = requests.shift(); if (!request) throw new Error("No pending request"); request.resolve(json(body)); },
  reject: () => { const request = requests.shift(); if (!request) throw new Error("No pending request"); request.reject(new Error("暂时无法创建，请重试")); },
};
declare global { interface Window { wizardFixture: typeof fixture } }
window.wizardFixture = fixture;

function Fixture() {
  const [wizardKey, setWizardKey] = useState(0);
  resetWizard = () => setWizardKey((value) => value + 1);
  const [open, setOpen] = useState(true);
  const [firstRun, setFirstRun] = useState(true);
  changeOpen = setOpen;
  changeFirstRun = setFirstRun;
  const trigger = useRef<HTMLButtonElement>(null);
  return <OverlayProvider><LiquidGlassLayer><button id="wizard-launcher" ref={trigger} onClick={() => setOpen(true)}>创建资源库</button><ManagedLibraryWizard key={wizardKey} firstRun={firstRun} open={open} onClose={() => { events.closed++; setOpen(false); }} onCreated={() => { events.created++; }} onFailure={(cause) => { events.failures.push(String(cause)); }} restoreFocusRef={trigger} /></LiquidGlassLayer></OverlayProvider>;
}
document.body.style.background = "linear-gradient(125deg, #18242d, #242026 48%, #142329)";
const root = createRoot(document.getElementById("root")!);
root.render(<Fixture />);

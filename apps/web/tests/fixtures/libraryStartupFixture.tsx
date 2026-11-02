import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { ActiveLibraryResponse } from "@mangoo/core-client";
import type { LibraryLoadPhase } from "../../src/core/libraryTypes";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import { createLibrary, setLibraryIcon, switchLibrary, subscribeLibraryPhase } from "../../src/core/desktop";
import { QueryClientProvider } from "@tanstack/react-query";
import { queryClient } from "../../src/core/queryClient";
import { OverlayProvider } from "../../src/design/overlays";
import { App } from "../../src/app/App";
import "../../src/styles.css";

const fixtureWindow = window as typeof window & {
  __overlayLifecycleResult?: { ok: boolean; error?: string; scenarios?: string[] };
  __libraryFixtureFetch?: typeof fetch;
};
const listeners = new Set<(phase: LibraryLoadPhase) => void>();
const requests: Array<{ path: string; revision: number; header: string | null }> = [];
const reports: string[] = [];
let snapshot: ActiveLibraryResponse = { library: null, contextRevision: 0, runtimeGeneration: 0 };
let library: LibraryState;
let discoveryBlocked = true;
let releaseDiscovery: (() => void) | null = null;
let rootConflict = false;
let discoveryFails = false;
let createFailure: "request" | "activation" | null = null;
let holdCreate = false;
let releaseCreate: (() => void) | null = null;
const registry = new Map<string, NonNullable<ActiveLibraryResponse["library"]>>();
let holdRegistry = false;
const registryReplies: Array<() => void> = [];
let holdAppearance = false;
let releaseAppearance: (() => void) | null = null;
function finishCreate() { releaseCreate?.(); releaseCreate = null; }
const mutationPhases: LibraryLoadPhase[] = [];
const unsubscribeMutationPhases = subscribeLibraryPhase((value) => mutationPhases.push(value));

window.mangooDesktop = {
  coreUrl: `${location.origin}/api`,
  onLibraryPhase: (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
  notifications: {
    reportError: async (report: { code: string }) => { reports.push(report.code); return true; }
  } as unknown as NonNullable<NonNullable<typeof window.mangooDesktop>["notifications"]>
};

function json(body: unknown, revision: number, status = 200) {
  return new Response(JSON.stringify(body), {
    status, headers: { "content-type": "application/json", "x-mangoo-context-revision": String(revision) }
  });
}
globalThis.fetch = async (input, init) => {
  const path = new URL(String(input)).pathname;
  const responseSnapshot = snapshot;
  if (path === "/api/plugins") return json({ items: [] }, snapshot.contextRevision);
  if (path === "/api/libraries" && init?.method === "POST") {
    const body = JSON.parse(String(init.body));
    if (holdCreate) await new Promise<void>((resolve) => { releaseCreate = resolve; });
    if (createFailure === "request") return json({ code: "create_failed", error: "Creation failed" }, snapshot.contextRevision, 400);
    const created = { libraryUuid: body.name, name: body.name, active: false };
    registry.set(body.name, created as NonNullable<ActiveLibraryResponse["library"]>);
    const activated = body.activate !== false && createFailure !== "activation";
    if (activated) setActive(body.name, snapshot.contextRevision + 1);
    return json({ library: created, activated, activationError: activated ? null : "Activation failed" }, snapshot.contextRevision);
  }
  if (path === "/api/libraries") {
    const response = json({ items: [...registry.values()].map((entry) => ({ ...entry, active: entry.libraryUuid === snapshot.library?.libraryUuid })) }, snapshot.contextRevision);
    if (holdRegistry) await new Promise<void>((resolve) => registryReplies.push(resolve));
    return response;
  }
  if (path.startsWith("/api/libraries/") && init?.method === "PATCH") {
    const id = path.split("/").at(-1)!;
    const entry = registry.get(id);
    check(entry, "appearance target exists");
    const updated = { ...entry, ...JSON.parse(String(init.body)) };
    registry.set(id, updated);
    if (snapshot.library?.libraryUuid === id) snapshot = { ...snapshot, library: updated };
    if (holdAppearance) await new Promise<void>((resolve) => { releaseAppearance = resolve; });
    return json(updated, snapshot.contextRevision);
  }
  if (path.endsWith("/activate")) {
    setActive(path.split("/").at(-2)!, snapshot.contextRevision + 1);
    return json(snapshot, snapshot.contextRevision);
  }
  if (path === "/api/libraries/active") {
    if (discoveryFails) return json({ code: "discovery_unavailable", error: "Temporary discovery failure" }, snapshot.contextRevision, 503);
    if (discoveryBlocked) await new Promise<void>((resolve) => {
      const previous = releaseDiscovery;
      releaseDiscovery = () => { previous?.(); resolve(); };
    });
    return json(responseSnapshot, responseSnapshot.contextRevision);
  }
  requests.push({ path, revision: snapshot.contextRevision, header: new Headers(init?.headers).get("x-mangoo-context-revision") });
  if (!snapshot.library) return json({ code: "no_active_library", error: "No active Library" }, snapshot.contextRevision, 409);
  if (path === "/api/roots" && rootConflict) return json({ code: "fixture_conflict", error: "Real conflict" }, snapshot.contextRevision, 409);
  if (path === "/api/tags") return json({ items: [{ id: 1, name: `tag-${snapshot.contextRevision}` }] }, snapshot.contextRevision);
  if (path === "/api/thumbnails/cache/cancel-bulk") return json({ cancelled: 0 }, snapshot.contextRevision);
  if (path === "/api/thumbnails/cache/stats") return json({ total: 0, ready: 0, pending: 0, failed: 0 }, snapshot.contextRevision);
  return json({ items: [] }, snapshot.contextRevision);
};

function Probe() {
  library = useLibraryData({
    // Intentionally not tied to discovery: App's sidebar identity follows ready.
    activeLibraryId: "report-context",
    activeLibraryName: "Fixture",
    persistentThumbnailCacheAutoRefresh: false
  });
  return <span>{library.loading ? "Loading" : "Settled"}</span>;
}
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function waitFor(predicate: () => boolean, message: string) {
  const deadline = Date.now() + 4000;
  while (!predicate() && Date.now() < deadline) await delay(10);
  check(predicate(), message);
}
function phase(value: LibraryLoadPhase) { for (const listener of [...listeners]) listener(value); }
function setActive(id: string, revision: number) {
  if (!registry.has(id)) registry.set(id, { libraryUuid: id, name: id } as NonNullable<ActiveLibraryResponse["library"]>);
  snapshot = {
    library: registry.get(id)!,
    contextRevision: revision, runtimeGeneration: revision
  };
}
function activate(id: string, revision: number) { setActive(id, revision); phase("loading-media"); }
const create = (name: string, activate = true) => createLibrary({ name, activate, databasePath: `D:/fixture/${name}/Database`, contentRoots: [{ path: `D:/fixture/${name}/Files`, createIfMissing: true }], initialMode: "local-only" });

async function run() {
  let root = createRoot(document.getElementById("root")!);
  try {
    root.render(<StrictMode><Probe /></StrictMode>);
    await waitFor(() => releaseDiscovery !== null, "discovery started");
    await delay(100);
    check(requests.length === 0, `no Catalog requests while discovery is pending: ${JSON.stringify(requests)}`);
    discoveryBlocked = false;
    releaseDiscovery?.();
    await waitFor(() => library && !library.loading, "empty Host settles loading");
    await library.refreshThumbnailCacheStats();
    await delay(100);
    check(requests.length === 0, "no Catalog requests for confirmed empty Host");

    await create("a");
    await waitFor(() => library.tags[0]?.name === "tag-1" && requests.some((request) => request.path.endsWith("cancel-bulk")), "activation loads tags and cancels bulk");
    check(requests.some((request) => request.path === "/api/roots"), "activation loads roots");
    check(requests.filter((request) => request.path.endsWith("cancel-bulk")).every((request) => request.header === "1"), "mutations use the active revision");

    phase("starting-core");
    await delay(50);
    const before = requests.length;
    await library.refreshThumbnailCacheStats();
    await delay(100);
    check(requests.length === before, "switch teardown closes Catalog admission");
    await switchLibrary("b");
    await waitFor(() => library.tags[0]?.name === "tag-2" && requests.some((request) => request.path.endsWith("cancel-bulk") && request.header === "2"), "switch reloads the new Library");

    activate("b", 3);
    await waitFor(() => library.tags[0]?.name === "tag-3" && requests.some((request) => request.path.endsWith("cancel-bulk") && request.header === "3"), "same Library revision refreshes data and bulk policy");
    rootConflict = true;
    await library.refresh();
    await waitFor(() => reports.includes("fixture_conflict"), "real 409 remains reported");
    root.unmount();

    rootConflict = false;
    discoveryFails = true;
    requests.length = 0;
    check(window.mangooDesktop, "fixture bridge remains installed");
    window.mangooDesktop.onLibraryPhase = undefined;
    root = createRoot(document.getElementById("root")!);
    root.render(<StrictMode><Probe /></StrictMode>);
    await waitFor(() => reports.includes("discovery_unavailable"), "initial discovery failure remains reported");
    check(requests.length === 0, "failed discovery does not dispatch Catalog requests");
    discoveryFails = false;
    snapshot = { ...snapshot, contextRevision: 4, runtimeGeneration: 4 };
    await library.refresh();
    await waitFor(() => library.tags[0]?.name === "tag-4" && !library.loading, "explicit refresh recovers without a Desktop phase producer");

    const unchangedPhaseCount = mutationPhases.length;
    await create("inactive", false);
    check(mutationPhases.length === unchangedPhaseCount && snapshot.library?.libraryUuid === "b", "nonactivating creation preserves context and sends no transition");
    for (const failure of ["request", "activation"] as const) {
      createFailure = failure;
      let rejected = false;
      try { await create(`failed-${failure}`); } catch (error) {
        rejected = true;
        if (failure === "activation") {
          check(error instanceof Error
            && error.message.includes("资源库已创建，但未能打开。请从资源库列表重试打开。")
            && error.message.includes("Activation failed"), "partial activation preserves creation status, recovery action and Core cause");
        }
      }
      check(rejected, `${failure} must reject without false success`);
      await waitFor(() => library.tags[0]?.name === "tag-4" && !library.loading && !library.switching, `${failure} restores prior authoritative context`);
      check(snapshot.library?.libraryUuid === "b", `${failure} does not publish requested identity`);
    }
    createFailure = null;

    holdCreate = true;
    const pendingCreate = create("late-create");
    await waitFor(() => releaseCreate !== null, "create is in flight");
    const requestCount = requests.length;
    phase("ready"); // A late Desktop startup notification cannot reopen admission.
    await switchLibrary("winner");
    await library.refreshThumbnailCacheStats();
    await delay(100);
    check(requests.length === requestCount, "overlapping switch completion cannot admit Catalog while create is pending");
    holdCreate = false;
    finishCreate();
    await pendingCreate;
    await waitFor(() => library.tags[0]?.name === "tag-6" && !library.loading, "last settlement reloads actual late-create context");
    check(String(snapshot.library?.libraryUuid) === "late-create", "Core completion order owns identity");
    root.unmount();

    // Mount the production App: no copied caption logic and no manufactured phase
    // event after the real mutation adapter. Also cover the browser-only adapter.
    window.mangooDesktop = undefined;
    snapshot = { library: null, contextRevision: 0, runtimeGeneration: 0 };
    registry.clear();
    requests.length = 0;
    root = createRoot(document.getElementById("root")!);
    root.render(<StrictMode><QueryClientProvider client={queryClient}><OverlayProvider><App /></OverlayProvider></QueryClientProvider></StrictMode>);
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "Library", "App mounts empty");
    await delay(100);
    check(requests.length === 0, "production App empty Host sends no Catalog requests");
    await create("App-created");
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "App-created", "real App caption follows created Library");
    check(requests.some((request) => request.path === "/api/roots") && requests.some((request) => request.path === "/api/tags"), "production App loads created Catalog without fixture phase");

    fixtureWindow.__libraryFixtureFetch = globalThis.fetch;
    const peer = document.createElement("iframe");
    peer.srcdoc = `<script type="module">
      import { createLibrary, setLibraryIcon } from '/src/core/desktop.ts';
      globalThis.fetch = (...args) => parent.__libraryFixtureFetch(...args);
      window.createFixtureLibrary = (name) => createLibrary({ name, databasePath: 'D:/fixture/' + name + '/Database', contentRoots: [{ path: 'D:/fixture/' + name + '/Files', createIfMissing: true }], activate: true, initialMode: 'local-only' });
      window.setFixtureAppearance = (id, icon, color) => setLibraryIcon(id, icon, color);
    </script>`;
    document.body.append(peer);
    const peerWindow = () => peer.contentWindow as Window & { createFixtureLibrary?: (name: string) => Promise<unknown>; setFixtureAppearance?: (id: string, icon: string, color: string) => Promise<unknown> };
    await waitFor(() => !!peerWindow().createFixtureLibrary, "second renderer loads mutation adapter");
    const beforePeerRequests = requests.length;
    await peerWindow().createFixtureLibrary!("Settings-created");
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "Settings-created", "second renderer mutation refreshes main App caption");
    check(requests.slice(beforePeerRequests).some((request) => request.path === "/api/roots" && request.revision === 2), "second renderer mutation refreshes main Catalog");
    const titleGlyph = () => document.querySelector<SVGElement>(".shell-library-switcher-button svg");
    const titleColor = () => titleGlyph() ? getComputedStyle(titleGlyph()!).color : "missing";
    await delay(100);
    const beforeAppearancePhases = mutationPhases.length;
    const beforeAppearanceCatalog = requests.length;
    const initialGlyph = titleGlyph()?.innerHTML;
    await peerWindow().setFixtureAppearance!("Settings-created", "camera", "#f06595");
    await waitFor(() => titleColor() === "rgb(240, 101, 149)", "cross-window metadata updates actual App titlebar color");
    check(titleGlyph()?.innerHTML !== initialGlyph, "cross-window metadata updates actual App glyph");
    check(mutationPhases.length === beforeAppearancePhases && requests.length === beforeAppearanceCatalog, "appearance notification does not reload Catalog or publish media phases");

    holdCreate = true;
    releaseCreate = null;
    const departingCreate = peerWindow().createFixtureLibrary!("departing");
    await waitFor(() => releaseCreate !== null, "departing publisher mutation starts");
    peer.remove();
    await delay(100);
    check(mutationPhases.at(-1) === "loading-media", "departed publisher cannot strand main in a pending phase");
    createFailure = "request";
    holdCreate = false;
    finishCreate();
    await departingCreate.catch(() => undefined);
    createFailure = null;
    await create("After-departure");
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "After-departure", "main remains operable after publisher departure");
    root.unmount();
    root = createRoot(document.getElementById("root")!);
    root.render(<StrictMode><QueryClientProvider client={queryClient}><OverlayProvider><App /></OverlayProvider></QueryClientProvider></StrictMode>);
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "After-departure", "late subscriber discovers current authoritative identity");

    await setLibraryIcon("After-departure", "camera", "#f06595");
    await waitFor(() => titleColor() === "rgb(240, 101, 149)", "same-window metadata updates actual App titlebar");
    const cameraGlyph = titleGlyph()?.innerHTML;
    await setLibraryIcon("Settings-created", "folder", "#123456");
    await delay(100);
    check(titleColor() === "rgb(240, 101, 149)" && titleGlyph()?.innerHTML === cameraGlyph, "inactive metadata never paints onto active titlebar");

    // Both the production App hook and switcher read the registry. Delay every
    // older reply while allowing a newer invalidation to settle first.
    holdRegistry = true;
    await setLibraryIcon("After-departure", "film", "#f59f00");
    await waitFor(() => registryReplies.length >= 2, "old authoritative registry reads are held");
    holdRegistry = false;
    // Release the child switcher's request so the shared navigation lane can
    // carry the new invalidation while the App's older identity stays delayed.
    registryReplies.shift()!();
    await setLibraryIcon("After-departure", "star", "#51cf66");
    await waitFor(() => titleColor() === "rgb(81, 207, 102)", "latest metadata read paints");
    const latestGlyph = titleGlyph()?.innerHTML;
    registryReplies.splice(0).forEach((release) => release());
    await delay(100);
    check(titleColor() === "rgb(81, 207, 102)" && titleGlyph()?.innerHTML === latestGlyph, "late older list cannot overwrite latest pair");

    holdRegistry = true;
    await setLibraryIcon("After-departure", "camera", "#f06595");
    await waitFor(() => registryReplies.length >= 2, "old Library reads are held before switching");
    holdRegistry = false;
    registryReplies.shift()!();
    await switchLibrary("Settings-created");
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "Settings-created" && titleColor() === "rgb(18, 52, 86)", "switch paints authoritative new Library");
    registryReplies.splice(0).forEach((release) => release());
    await delay(100);
    check(titleColor() === "rgb(18, 52, 86)", "old list cannot repaint switched Library");

    holdAppearance = true;
    const pendingAppearance = setLibraryIcon("After-departure", "image", "#ff6b6b");
    await waitFor(() => releaseAppearance !== null, "appearance response remains pending");
    await switchLibrary("App-created");
    await waitFor(() => document.querySelector(".shell-library-switcher-name")?.textContent === "App-created", "switch settles while old appearance response remains pending");
    holdAppearance = false;
    releaseAppearance?.();
    await pendingAppearance;
    await delay(100);
    check(document.querySelector(".shell-library-switcher-name")?.textContent === "App-created" && titleColor() === "rgb(255, 255, 255)", "old pending save never repaints switched Library");
    await switchLibrary("Settings-created");
    await waitFor(() => titleColor() === "rgb(18, 52, 86)", "return to retained custom appearance");

    holdRegistry = true;
    await setLibraryIcon("Settings-created", "star", "#f06595");
    await waitFor(() => registryReplies.length >= 2, "registry reads are held before unmount");
    root.unmount();
    holdRegistry = false;
    registryReplies.shift()!();
    await setLibraryIcon("Settings-created", "folder", "#123456");
    root = createRoot(document.getElementById("root")!);
    root.render(<StrictMode><QueryClientProvider client={queryClient}><OverlayProvider><App /></OverlayProvider></QueryClientProvider></StrictMode>);
    await waitFor(() => titleColor() === "rgb(18, 52, 86)", "remounted App reads current metadata");
    registryReplies.splice(0).forEach((release) => release());
    await delay(100);
    check(titleColor() === "rgb(18, 52, 86)", "unmounted consumer cannot paint its late response");
    root.unmount();
    unsubscribeMutationPhases();
    fixtureWindow.__overlayLifecycleResult = { ok: true, scenarios: ["pending", "empty", "activate", "switch", "revision", "conflict", "discovery-retry", "nonactivating-create", "failed-create", "failed-activation", "overlapping-mutations", "browser-app-create", "cross-window-create", "publisher-departure", "late-subscriber", "appearance-cross-window", "appearance-no-teardown", "appearance-same-window", "appearance-inactive", "appearance-latest-response", "appearance-switch-pending", "appearance-unmount"] };
  } catch (error) {
    root.unmount();
    fixtureWindow.__overlayLifecycleResult = { ok: false, error: error instanceof Error ? error.stack : String(error) };
  }
}
void run();

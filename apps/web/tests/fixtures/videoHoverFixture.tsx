import { act } from "react";
import { createRoot } from "react-dom/client";
import type { MediaRecord } from "@mangoo/core-client";
import { MediaGrid } from "../../src/features/media-grid/MediaGrid";
import { DEFAULT_LIBRARY_GRID_PREFERENCES } from "../../src/features/media-grid/gridPreferences";
import type { LibraryLayoutMode } from "../../src/features/media-grid/layoutMode";
import { resetMediaCachesForLibrarySwitch } from "../../src/core/mediaResources";
import "../../src/styles.css";

Reflect.set(window, "IS_REACT_ACT_ENVIRONMENT", true);
window.mangooDesktop = { coreUrl: `${location.origin}/api`, sessionToken: "fixture-session" };
const media: MediaRecord = { id: 1, rootId: 1, folderId: 2, name: "Hover.mp4", ext: "mp4", size: 1024, mtime: 1, kind: "video", width: 320, height: 180, thumbnailState: "failed" };
const root = createRoot(document.getElementById("root")!);
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const lifecycleTrace: object[] = [];
function trace(event: string, detail: object = {}) {
  lifecycleTrace.push({ event, at: performance.now(), focus: document.hasFocus(), hidden: document.hidden, ...detail,
    players: [...document.querySelectorAll("video")].map((player) => ({ time: player.currentTime, paused: player.paused,
      seeking: player.seeking, ready: player.readyState, connected: player.isConnected, src: player.currentSrc })) });
  if (lifecycleTrace.length > 50) lifecycleTrace.shift();
}
const traceEvents = ["blur", "focus", "scroll", "resize", "visibilitychange", "playing", "pause", "seeking", "seeked", "error", "emptied"];
const traceEvent = (event: Event) => trace(event.type, { trusted: event.isTrusted, target: (event.target as HTMLElement | null)?.className });
for (const event of traceEvents) window.addEventListener(event, traceEvent, true);
function ensure(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`${message}; lifecycle=${JSON.stringify(lifecycleTrace)}`);
}
let requests = 0, revoked = 0, opened = 0, selected = 0, located = 0, context = 0;
let createDelay = 0;
let renewalRemaining = 30_000;
let renewals = 0;
const originalFetch = window.fetch;
window.fetch = async (input, options) => {
  const url = String(input);
  if (options?.method === "DELETE") { revoked++; return new Response(null, { status: 204 }); }
  if (url.endsWith("/renew")) {
    renewals++;
    const id = url.split("/").at(-2)!;
    return new Response(JSON.stringify({ id, src: `/playback/${"a".repeat(40)}${id.padStart(3, "0")}`, expiresAt: Date.now() + 60_000, renewAfterMs: 9_000 }), { headers: { "Content-Type": "application/json" } });
  }
  if (url.endsWith("/playback-sessions")) {
    requests++;
    const id = String(requests);
    await delay(createDelay);
    return new Response(JSON.stringify({ id, src: `/playback/${"a".repeat(40)}${id.padStart(3, "0")}`, expiresAt: Date.now() + 60_000, renewAfterMs: renewalRemaining }), { status: 201, headers: { "Content-Type": "application/json" } });
  }
  throw new Error(`Unexpected request ${url}`);
};

function Fixture({ mode, enabled = true, item = media }: { mode: LibraryLayoutMode; enabled?: boolean; item?: MediaRecord }) {
  return <div style={{ height: 390, width: 620, position: "relative" }}><MediaGrid hoverPlaybackEnabled={enabled}
    gridPreferences={DEFAULT_LIBRARY_GRID_PREFERENCES} items={[item, { ...item, id: 2 }]} layoutMode={mode} loading={false} loadingMore={false}
    selectedMediaId={1} onSelect={() => selected++} onOpenPreview={() => opened++} onLocateFolder={() => located++}
    onContextMenu={() => context++} onRequestThumbnailStates={() => {}} thumbnailStatesByMediaId={{}} /></div>;
}

function shells() { return [...document.querySelectorAll<HTMLElement>(".media-tile-thumb-shell")]; }
function pointer(element: HTMLElement, type: string, x?: number, y?: number, pointerType = "mouse") {
  const box = element.getBoundingClientRect();
  trace(type, { x: x ?? box.left + box.width / 2, y: y ?? box.top + box.height / 2, pointerType, bounds: box.toJSON() });
  element.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerType, clientX: x ?? box.left + box.width / 2, clientY: y ?? box.top + box.height / 2 }));
}
async function waitFor(check: () => boolean, message: string, timeout = 2200) {
  const end = performance.now() + timeout;
  while (!check() && performance.now() < end) await act(async () => { await delay(30); });
  ensure(check(), `${message}; player=${JSON.stringify([...document.querySelectorAll("video")].map((v) => ({ paused: v.paused, ready: v.readyState, error: v.error?.message, src: v.currentSrc, time: v.currentTime })))}`);
}
async function enter(shell: HTMLElement) {
  const before = requests;
  await act(async () => {
    pointer(shell, "pointerenter");
    ensure(requests === before + 1, "eligible enter requests its grant synchronously without a dwell timer");
  });
  await waitFor(() => { const player = shell.querySelector("video"); return !!player && player.currentTime > 0.05 && !player.paused; }, "native time advances");
  return shell.querySelector("video")!;
}

function assertIndicator(shell: HTMLElement, ratio: number, label: string) {
  const fill = shell.querySelector<HTMLElement>(".video-hover-progress > span")!;
  ensure(Math.abs(new DOMMatrix(getComputedStyle(fill).transform).a - ratio) < 0.001, `${label}: pointer progress`);
  ensure(shell.querySelector(".video-hover-time")!.textContent === `0:${String(Math.floor(ratio * 12)).padStart(2, "0")} / 0:12`, `${label}: pointer time`);
  for (const overlay of [fill, shell.querySelector<HTMLElement>(".video-hover-time")!]) {
    ensure(overlay.getBoundingClientRect().width > 0 && overlay.getBoundingClientRect().height > 0, `${label}: visible HUD geometry`);
    for (let element: HTMLElement | null = overlay; element && element !== shell; element = element.parentElement) {
      const style = getComputedStyle(element);
      ensure(style.display !== "none" && style.visibility === "visible" && Number(style.opacity) > 0, `${label}: HUD is painted independently of decoder readiness`);
    }
  }
}

// Keep the real mounted player/transport, but deliberately delay seek completion
// so presentation must not accidentally depend on a fast local decoder.
async function delayedSeek(shell: HTMLElement, player: HTMLVideoElement, mode: string) {
  const bounds = shell.getBoundingClientRect();
  let decodedTime = player.currentTime, seeking = false;
  const targets: number[] = [];
  Object.defineProperty(player, "currentTime", { configurable: true, get: () => decodedTime,
    set: (target: number) => { targets.push(target); seeking = true; } });
  Object.defineProperty(player, "seeking", { configurable: true, get: () => seeking });
  try {
    await act(async () => pointer(shell, "pointermove", bounds.left + bounds.width * 0.85));
    assertIndicator(shell, 0.85, `${mode}: before decoded frame`);
    await waitFor(() => targets.length === 1, "one initial seek submitted");
    await act(async () => {
      pointer(shell, "pointermove", bounds.left + bounds.width * 0.1);
      pointer(shell, "pointermove", bounds.left + bounds.width * 0.75);
      pointer(shell, "pointermove", bounds.left + bounds.width * 0.5);
      player.dispatchEvent(new Event("timeupdate"));
      player.dispatchEvent(new Event("seeked"));
    });
    assertIndicator(shell, 0.5, `${mode}: rapid reversals beat stale media events`);
    await act(async () => { await delay(160); });
    ensure(targets.length === 1 && player.paused, `${mode}: pending seek coalesces and blocks early resume`);
    assertIndicator(shell, 0.5, `${mode}: waiting for decoder retains target`);
    await act(async () => {
      decodedTime = targets[0]; seeking = false;
      player.dispatchEvent(new Event("seeked"));
    });
    ensure(Number(targets.length) === 2 && Math.abs(targets[1] - 6) < 0.01, `${mode}: only latest queued seek is submitted`);
    await act(async () => {
      seeking = false; // A stale completion at the previous decoded position.
      player.dispatchEvent(new Event("seeked"));
      player.dispatchEvent(new Event("timeupdate"));
    });
    ensure(player.paused, `${mode}: stale seek completion cannot resume`);
    assertIndicator(shell, 0.5, `${mode}: stale completion cannot snap backwards`);
  } finally {
    Reflect.deleteProperty(player, "currentTime");
    Reflect.deleteProperty(player, "seeking");
  }
  player.currentTime = targets.at(-1)!;
  await waitFor(() => !player.paused && player.currentTime >= 5.8 && player.currentTime < 7, `${mode}: real seek completes and resumes`);
}
async function assertStopped(player: HTMLVideoElement, label: string) {
  await waitFor(() => !document.querySelector("video"), `${label}: unmounted`);
  ensure(player.paused && !player.hasAttribute("src"), `${label}: detached player paused and released`);
}

async function run() {
  const modes: string[] = [];
  try {
    for (const mode of ["adaptive", "waterfall", "grid", "list"] as const) {
      await act(async () => { root.render(<Fixture key={mode} mode={mode} />); await delay(100); });
      const shell = shells()[0]; ensure(shell, `${mode}: shell`);
      const bounds = shell.getBoundingClientRect();
      await act(async () => pointer(shell, "pointerenter", undefined, undefined, "touch"));
      await act(async () => { await delay(170); });
      ensure(!shell.querySelector("video"), "touch is inert");
      if (mode === "adaptive") {
        const beforeInactive = requests;
        Object.defineProperty(document, "hasFocus", { configurable: true, value: () => false });
        await act(async () => { pointer(shell, "pointerenter"); await delay(170); });
        ensure(requests === beforeInactive && !shell.querySelector("video"), "inactive window cannot begin another hover");
        Reflect.deleteProperty(document, "hasFocus");
      }
      // Hold back the readiness event, while native media still loads, to prove
      // the HUD does not share the video image's loadeddata visibility gate.
      const holdLoadedData = (event: Event) => event.stopImmediatePropagation();
      if (mode === "adaptive") document.addEventListener("loadeddata", holdLoadedData, true);
      let player: HTMLVideoElement;
      try { player = await enter(shell); }
      finally { document.removeEventListener("loadeddata", holdLoadedData, true); }
      if (mode === "adaptive") ensure(!shell.querySelector(".video-hover")!.hasAttribute("data-playing"), "video readiness gate is still closed");
      ensure(player.muted && player.volume === 0 && player.playbackRate === 1 && player.loop, `${mode}: mute/rate/loop`);
      ensure(player.duration > 11 && player.duration < 13, `${mode}: actual 12s clip decoded`);
      const before = player.currentTime;
      trace("vertical-before", { cachedBounds: bounds.toJSON(), bounds: shell.getBoundingClientRect().toJSON() });
      await act(async () => { pointer(shell, "pointermove", bounds.left + bounds.width / 2, bounds.top + bounds.height * 0.7); await delay(180); });
      trace("vertical-after", { before });
      ensure(player.currentTime > before && player.currentTime < before + 1, `${mode}: vertical does not seek`);
      await delayedSeek(shell, player, mode);
      if (mode === "adaptive") await act(async () => player.dispatchEvent(new Event("loadeddata")));
      await act(async () => { pointer(shell, "pointermove", bounds.left + bounds.width * 0.85); pointer(shell, "pointermove", bounds.left + bounds.width * 0.1); pointer(shell, "pointermove", bounds.left + bounds.width * 0.5); });
      await waitFor(() => player.currentTime >= 5.8 && player.currentTime < 7 && !player.paused, `${mode}: latest middle seek and resume`);
      const resumed = player.currentTime;
      await act(async () => { await delay(180); });
      ensure(player.currentTime > resumed, `${mode}: stationary resumes`);
      if (mode === "grid") {
        Object.defineProperty(player, "readyState", { configurable: true, value: 0 });
        Object.defineProperty(player, "duration", { configurable: true, value: Number.NaN });
        await act(async () => pointer(shell, "pointermove", bounds.left + bounds.width * 0.7));
        ensure(Math.abs(new DOMMatrix(getComputedStyle(shell.querySelector(".video-hover-progress > span")!).transform).a - 0.7) < 0.001, "progress geometry does not wait for metadata");
        ensure(shell.querySelector(".video-hover-time")!.textContent === "0:00 / 0:00", "unknown duration does not invent a time");
        await act(async () => { await delay(160); });
        Reflect.deleteProperty(player, "readyState");
        Reflect.deleteProperty(player, "duration");
        await act(async () => player.dispatchEvent(new Event("loadedmetadata")));
        await waitFor(() => !player.paused && player.currentTime > 8 && player.currentTime < 9.5, "latest pre-metadata intent seeks and resumes when metadata arrives");
        for (const duration of [Number.NaN, 0, Number.POSITIVE_INFINITY]) {
          // Give the next horizontal move a distinct previous x even after
          // the pre-metadata case ended at the same target.
          await act(async () => pointer(shell, "pointermove", bounds.left + bounds.width * 0.5));
          await waitFor(() => !player.paused, "known duration reset resumes");
          Object.defineProperty(player, "duration", { configurable: true, value: duration });
          await act(async () => pointer(shell, "pointermove", bounds.left + bounds.width * 0.7));
          await waitFor(() => !player.paused, "unknown duration does not block stationary playback");
          Reflect.deleteProperty(player, "duration");
          await act(async () => pointer(shell, "pointermove", bounds.left + bounds.width * 0.5));
          await waitFor(() => !player.paused && player.currentTime > 5.8 && player.currentTime < 7, "known duration seek resumes");
        }
      }
      await act(async () => pointer(shell, "pointermove", bounds.left + 0.1));
      await waitFor(() => player.currentTime < 1 && !player.paused, `${mode}: left edge`);
      await act(async () => pointer(shell, "pointermove", bounds.right - 0.1));
      await waitFor(() => player.currentTime > 10.5 || player.currentTime < 0.5, `${mode}: right edge/loop`);
      ensure(getComputedStyle(shell.querySelector(".video-hover-progress")!).height === "2px", `${mode}: 2px rail`);
      ensure(getComputedStyle(shell.querySelector(".video-hover")!).pointerEvents === "none", `${mode}: passive overlay`);
      ensure(shell.querySelector(".file-format-badge"), `${mode}: badge retained`);
      const badge = getComputedStyle(shell.querySelector(".file-format-badge")!);
      const timeLabel = getComputedStyle(shell.querySelector(".video-hover-time")!);
      ensure(Math.abs(parseFloat(badge.fontSize) * new DOMMatrix(badge.transform).a - parseFloat(timeLabel.fontSize)) < 0.001, `${mode}: time matches visual badge font size`);
      ensure(badge.opacity === "0.8" && timeLabel.opacity === "0.9", `${mode}: independent badge/time opacity`);
      await act(async () => pointer(shell, "pointerleave")); await assertStopped(player, "leave");
      const next = await enter(shell);
      await act(async () => shell.closest(".media-tile")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
      await assertStopped(next, "preview");
      await act(async () => shell.querySelector<HTMLButtonElement>(".media-tile-locate-button")!.click());
      ensure(opened === modes.length + 1 && located === modes.length + 1, `${mode}: original actions`);
      modes.push(mode);
    }
    const shell = shells()[0];
    for (const reason of ["scroll", "blur", "switch", "preview-cover", "content", "error"] as const) {
      const player = await enter(shells()[0]);
      await act(async () => {
        if (reason === "switch") resetMediaCachesForLibrarySwitch();
        else if (reason === "preview-cover") root.render(<Fixture key="list" mode="list" enabled={false} />);
        else if (reason === "content") root.render(<Fixture key="list" mode="list" item={{ ...media, mtime: 2 }} />);
        else if (reason === "error") player.dispatchEvent(new Event("error"));
        else window.dispatchEvent(new Event(reason));
      });
      await assertStopped(player, reason);
      await act(async () => { root.render(<Fixture key="list" mode="list" />); await delay(30); });
    }
    const first = await enter(shells()[0]);
    await act(async () => pointer(shells()[1], "pointerenter"));
    await waitFor(() => first.paused && !first.hasAttribute("src"), "crossing stops previous immediately");
    await waitFor(() => document.querySelectorAll("video").length === 1 && !!shells()[1].querySelector("video"), "single active player");
    await act(async () => pointer(shells()[1], "pointerleave"));
    const hidden = await enter(shells()[0]);
    Reflect.set(window, "__overlayLifecycleInputRequest", { id: 1, type: "window-hidden-cycle" });
    await waitFor(() => Reflect.get(window, "__overlayLifecycleInputResponseId") === 1, "native hidden cycle acknowledged");
    await assertStopped(hidden, "native window hidden");
    renewalRemaining = 250;
    const renewStart = requests;
    await enter(shells()[0]);
    renewalRemaining = 30_000;
    await waitFor(() => renewals > 0 && requests > renewStart + 1, "absolute-lifetime replacement grant");
    await waitFor(() => { const player = shells()[0].querySelector("video"); return !!player && !player.paused; }, "replacement resumes");
    await act(async () => pointer(shells()[0], "pointerleave"));
    createDelay = 220;
    const beforeQuickLeave = requests;
    await act(async () => { pointer(shell, "pointerenter"); pointer(shell, "pointerleave"); await delay(280); });
    ensure(requests === beforeQuickLeave + 1, "even a quick crossing begins immediately");
    ensure(!document.querySelector("video"), "late grant never activates");
    const beforeReentry = requests;
    await act(async () => {
      pointer(shells()[0], "pointerenter");
      pointer(shells()[0], "pointerleave");
      pointer(shells()[1], "pointerenter");
      pointer(shells()[0], "pointerenter");
      ensure(requests === beforeReentry + 3, `rapid re-entry starts each grant immediately: before=${beforeReentry}, after=${requests}, focus=${document.hasFocus()}, hidden=${document.hidden}`);
      await delay(280);
    });
    ensure(document.querySelectorAll("video").length === 1 && !shells()[1].querySelector("video"), "pending cross-tile grants retain only the latest owner");
    const latest = shells()[0].querySelector("video")!;
    ensure(latest.getAttribute("src")!.endsWith(String(requests).padStart(3, "0")), "latest re-entry owns the source");
    await waitFor(() => !latest.paused && latest.currentTime > 0.05, "latest re-entry plays");
    const latestBounds = shells()[0].getBoundingClientRect();
    Object.defineProperty(latest, "seeking", { configurable: true, value: true });
    await act(async () => pointer(shells()[0], "pointermove", latestBounds.left + latestBounds.width * 0.8));
    assertIndicator(shells()[0], 0.8, "pending scrub before leave");
    await act(async () => pointer(shells()[0], "pointerleave"));
    await assertStopped(latest, "leave with pending scrub");
    createDelay = 0;
    const reopened = await enter(shells()[0]);
    await act(async () => {
      latest.dispatchEvent(new Event("seeked"));
      latest.dispatchEvent(new Event("timeupdate"));
      await delay(160);
    });
    ensure(reopened.currentTime < 1 && !reopened.paused && document.querySelectorAll("video").length === 1, "detached seek events cannot restore prior intent after re-entry");
    await act(async () => pointer(shells()[0], "pointerleave"));
    ensure(context === 0, "no unintended context action");
    Reflect.set(window, "__overlayLifecycleResult", { ok: true, modes, requests, revoked, selected });
  } catch (error) { Reflect.set(window, "__overlayLifecycleResult", { ok: false, error: error instanceof Error ? error.stack : String(error) }); }
  finally {
    for (const event of traceEvents) window.removeEventListener(event, traceEvent, true);
    await act(async () => root.unmount()); window.fetch = originalFetch; resetMediaCachesForLibrarySwitch();
  }
}
void run();

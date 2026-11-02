import { act } from "react";
import { createRoot } from "react-dom/client";
import type { MediaRecord } from "@mangoo/core-client";
import { MediaGrid } from "../../src/features/media-grid/MediaGrid";
import { DEFAULT_LIBRARY_GRID_PREFERENCES } from "../../src/features/media-grid/gridPreferences";
import type { LibraryLayoutMode } from "../../src/features/media-grid/layoutMode";
import { resetMediaCachesForLibrarySwitch } from "../../src/core/mediaResources";
import "../../src/styles.css";
Reflect.set(window, "IS_REACT_ACT_ENVIRONMENT", true);
const root = createRoot(document.getElementById("root")!);
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
function ensure(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function waitFor(check: () => boolean, message: string) {
  const end = performance.now() + 8000;
  while (!check() && performance.now() < end) await act(async () => { await delay(30); });
  ensure(check(), message);
}
async function run() {
  try {
    const data: { media: MediaRecord[]; token: string } = await fetch("/hover-fixture-data").then((r) => r.json());
    window.mangooDesktop = { coreUrl: `${location.origin}/api`, sessionToken: data.token };
    // Populate the transport's normal active-Library revision observation.
    const { createCoreClient } = await import("../../src/core/client");
    await createCoreClient().getActiveLibrary();
    const large = data.media.find((item) => item.name === "large.mp4")!;
    ensure(large && large.size > 512 * 1024 * 1024, "large real video indexed");
    const short = data.media.find((item) => item.name === "short.mp4")!;
    let opened = 0, selected = 0, located = 0, menus = 0;
    async function render(mode: LibraryLayoutMode, enabled = true, item = large) {
      await act(async () => { root.render(<div style={{ width: 620, height: 390 }}><MediaGrid items={[item, short]} layoutMode={mode}
        hoverPlaybackEnabled={enabled} gridPreferences={DEFAULT_LIBRARY_GRID_PREFERENCES} loading={false} loadingMore={false} selectedMediaId={null}
        onSelect={() => selected++} onOpenPreview={() => opened++} onLocateFolder={() => located++} onContextMenu={() => menus++}
        onRequestThumbnailStates={() => {}} thumbnailStatesByMediaId={{}} /></div>); await delay(70); });
    }
    const modes = [];
    for (const mode of ["adaptive", "waterfall", "grid", "list"] as const) {
      await render(mode);
      const target = shell(), beforeBounds = target.getBoundingClientRect();
      const { player, firstFrameMs } = await enter(target);
      ensure(player.muted && player.volume === 0 && player.playbackRate === 1 && player.loop, `${mode}: mute/rate/loop`);
      const before = player.currentTime;
      await act(async () => { pointer(target, "pointermove", 0.5, 0.7); await delay(160); });
      ensure(player.currentTime > before && player.currentTime < before + 1, `${mode}: vertical motion does not seek`);
      await act(async () => { pointer(target, "pointermove", 0.1); pointer(target, "pointermove", 0.8); });
      await waitFor(() => player.currentTime > 9.4 && player.currentTime < 10.7 && !player.paused, `${mode}: latest distant seek resumes`);
      const seekTime = player.currentTime;
      await native("screenshot", mode);
      const afterBounds = target.getBoundingClientRect();
      ensure(beforeBounds.width === afterBounds.width && beforeBounds.height === afterBounds.height, `${mode}: stable geometry`);
      ensure(target.querySelector(".file-format-badge") && target.querySelector(".video-hover-time"), `${mode}: badge and clock`);
      ensure(getComputedStyle(target.querySelector(".video-hover-progress")!).height === "2px", `${mode}: progress rail`);
      await act(async () => pointer(target, "pointermove", 0.001));
      await waitFor(() => player.currentTime < 1 && !player.paused, `${mode}: start seek resumes`);
      await act(async () => pointer(target, "pointerleave"));
      await stopped(player, "leave");
      const shortPlayback = await enter(shell(1));
      await act(async () => pointer(shell(1), "pointerleave"));
      await stopped(shortPlayback.player, "short clip leave");
      modes.push({ mode, firstFrameMs, seekTime, shortFirstFrameMs: shortPlayback.firstFrameMs });
    }
    const exits = [];
    for (const reason of ["scroll", "blur", "hidden", "preview", "content", "offscreen", "context", "locate", "keyboard", "doubleclick", "error"] as const) {
      const { player } = await enter();
      const target = shell(), tile = target.closest<HTMLElement>(".media-tile")!;
      if (reason === "hidden") await native("hidden");
      else if (reason === "preview") await render("list", false);
      else if (reason === "content") await render("list", true, { ...large, mtime: large.mtime + 1 });
      else await act(async () => {
        if (reason === "offscreen") target.style.transform = "translateY(2000px)";
        else if (reason === "context") target.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
        else if (reason === "locate") target.querySelector<HTMLButtonElement>(".media-tile-locate-button")!.click();
        else if (reason === "keyboard") tile.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Enter" }));
        else if (reason === "doubleclick") tile.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
        else if (reason === "error") player.dispatchEvent(new Event("error"));
        else window.dispatchEvent(new Event(reason));
      });
      await stopped(player, reason);
      target.style.transform = "";
      await render("list");
      exits.push(reason);
    }
    const first = await enter();
    const second = await enter(shell(1));
    ensure(first.player.paused && !first.player.hasAttribute("src") && document.querySelectorAll("video").length === 1, "crossing has one active player");
    await act(async () => pointer(shell(1), "pointerleave"));
    await stopped(second.player, "crossing leave");
    ensure(opened === 2 && selected >= 3 && located === 1 && menus === 1, "existing card operations preserved");
    await fetch("/hover-fixture-corrupt-short", { method: "POST" }).then((response) => ensure(response.ok, "isolated clip changed"));
    let nativeDecodeErrors = 0;
    const decodeError = (event: Event) => { if (event.target instanceof HTMLVideoElement) nativeDecodeErrors++; };
    document.addEventListener("error", decodeError, true);
    await act(async () => pointer(shell(1), "pointerenter"));
    await waitFor(() => nativeDecodeErrors > 0, "actual corrupt video decode fails");
    await waitFor(() => !shell(1).querySelector("video"), "corrupt video restores static fallback");
    await act(async () => { await delay(300); });
    ensure(nativeDecodeErrors === 1 && shell(1).querySelector(".tile-thumb"), "decode failure keeps cover without retry loop");
    document.removeEventListener("error", decodeError, true);
    await act(async () => pointer(shell(1), "pointerleave"));
    await fetch("/hover-fixture-remove-short", { method: "POST" }).then((response) => ensure(response.ok, "isolated source removed"));
    await act(async () => { pointer(shell(1), "pointerenter"); await delay(500); });
    ensure(!shell(1).querySelector("video") && shell(1).querySelector(".tile-thumb"), "missing file keeps static fallback");
    await act(async () => pointer(shell(1), "pointerleave"));
    const switching = await enter();
    await fetch("/hover-fixture-switch", { method: "POST" }).then((response) => ensure(response.ok, "actual Library activation"));
    await act(async () => resetMediaCachesForLibrarySwitch());
    await stopped(switching.player, "Library activation");
    await delay(250);
    Reflect.set(window, "__videoHoverResult", { ok: true, modes, exits: [...exits, "crossing", "decode-failure", "missing-source", "library-activation"], opened, selected, located, menus, nativeDecodeErrors });
  } catch (error) { Reflect.set(window, "__videoHoverResult", { ok: false, error: error instanceof Error ? error.stack : String(error) }); }
  finally { await act(async () => root.unmount()); }
}
let inputId = 0;
async function native(type: string, name?: string) {
  const id = ++inputId;
  Reflect.set(window, "__videoHoverInput", { id, type, name });
  await waitFor(() => Reflect.get(window, "__videoHoverInputDone") === id, `${type} acknowledged`);
}
function shell(index = 0) { return document.querySelectorAll<HTMLElement>(".media-tile-thumb-shell")[index]; }
function pointer(target: HTMLElement, type: string, ratio = 0.5, vertical = 0.5) {
  const bounds = target.getBoundingClientRect();
  target.dispatchEvent(new PointerEvent(type, { pointerType: "mouse", clientX: bounds.left + bounds.width * ratio, clientY: bounds.top + bounds.height * vertical }));
}
async function enter(target = shell()) {
  const started = performance.now();
  await act(async () => pointer(target, "pointerenter"));
  await waitFor(() => { const video = target.querySelector("video"); return !!video && !video.paused && video.currentTime > 0.05; }, "Core native video starts");
  return { player: target.querySelector("video")!, firstFrameMs: performance.now() - started };
}
async function stopped(player: HTMLVideoElement, reason: string) {
  await waitFor(() => !document.querySelector("video"), `${reason}: player removed`);
  ensure(player.paused && !player.hasAttribute("src"), `${reason}: source released`);
}

void run();

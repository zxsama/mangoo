import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import type { MediaRecord, ThumbnailResponse } from "../../src/core/types";
import { MediaGrid } from "../../src/features/media-grid/MediaGrid";
import { MediaPreview } from "../../src/features/preview/MediaPreview";
import { PreviewPanel } from "../../src/features/preview/PreviewPanel";
import { SubfolderCard } from "../../src/features/library/SubfolderStrip";
import { DEFAULT_LIBRARY_GRID_PREFERENCES } from "../../src/features/media-grid/gridPreferences";
import type { LibraryLayoutMode } from "../../src/features/media-grid/layoutMode";
import { requestOriginalPreviewBlob, requestThumbnailState, resetMediaCachesForLibrarySwitch, mediaContentSignature, mediaFileContentSignature } from "../../src/core/mediaResources";
import { useLibraryData, type LibraryState } from "../../src/core/useLibraryData";
import { useFolderCovers } from "../../src/features/library/useFolderCovers";
import { broadcastPluginChange } from "../../src/features/plugins/pluginQuerySync";
import "../../src/styles.css";

const state = window as typeof window & { IS_REACT_ACT_ENVIRONMENT?: boolean };
state.IS_REACT_ACT_ENVIRONMENT = true;
// The fixture's compact Electron window must still mount a visible desktop inspector.
const fixtureStyle = document.createElement("style");
fixtureStyle.textContent = ".inspector-panel.preview-panel { display: grid; }";
document.head.append(fixtureStyle);
const file: MediaRecord = { id: 11, rootId: 1, folderId: 2, name: "Sample.unusualsuffix", ext: "unusualsuffix", size: 7, mtime: 10, kind: null, width: null, height: null, presentation: { previewKind: "static-thumbnail", thumbnail: "unavailable", generation: 1 } };
const folder = { id: 2, rootId: 1, parentId: null, name: "Samples", relpath: "", mediaCount: 0, fileCount: 1, status: "ok", hasChildren: false, icon: null, color: null };
let selected = 0, opened = 0, located = 0;
const requested: string[] = [];
const originalFetch = window.fetch;
const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aMqoAAAAASUVORK5CYII=";
const bytes = Uint8Array.from(atob(png), (c) => c.charCodeAt(0));
window.fetch = async (input) => {
  const url = String(input);
  requested.push(url);
  if (url.includes("/thumbnail/blob")) return new Response(bytes, { headers: { "Content-Type": "image/png" } });
  if (url.includes("/preview")) return new Response(bytes, { headers: { "Content-Type": "image/png" } });
  throw new Error(`Unexpected resource request ${url}`);
};
const root = createRoot(document.getElementById("root")!);
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
function ensure(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }

const badgeMetrics: Array<Record<string, unknown>> = [];
async function captureBadgeGeometry(name: string) {
  const metrics = Array.from(document.querySelectorAll<HTMLElement>(".file-format-badge")).map((badge) => {
    const stage = badge.closest(".preview-stage");
    const image = stage?.querySelector<HTMLImageElement>("img");
    const artwork = stage?.querySelector<HTMLElement>(".file-format-artwork");
    const rect = badge.getBoundingClientRect();
    const content = image?.getBoundingClientRect() ?? artwork?.getBoundingClientRect();
    ensure(rect.width > 0 && rect.height > 0, `${name}: badge is painted`);
    if (content) ensure(content.width > 0 && content.height > 0, `${name}: content is painted`);
    if (image && content) ensure(Math.abs(content.width / content.height - image.naturalWidth / image.naturalHeight) < 0.01, `${name}: image element bounds match painted content, without internal letterboxing`);
    const style = getComputedStyle(badge);
    if (stage && badge.textContent === "UNUSUALSUFFIX") {
      const baselineClippedWidth = stage.getBoundingClientRect().width - 34;
      ensure(Math.abs(rect.width - baselineClippedWidth * 0.9) < 0.02, `${name}: narrow inspector preserves baseline clipping before 90% scale`);
    }
    return { label: badge.textContent, rect: rect.toJSON(), content: content?.toJSON(),
      naturalWidth: image?.naturalWidth, naturalHeight: image?.naturalHeight,
      opacity: style.opacity, transform: style.transform, borderColor: style.borderTopColor,
      layoutWidth: Number.parseFloat(style.width), layoutHeight: Number.parseFloat(style.height),
      insetX: content ? rect.left - content.left : null, insetY: content ? rect.top - content.top : null,
      inside: !content || (rect.left >= content.left && rect.top >= content.top && rect.right <= content.right && rect.bottom <= content.bottom) };
  });
  badgeMetrics.push({ name, metrics });
  Reflect.set(state, "__fileFormatBadgeMetrics", badgeMetrics);
  if (new URLSearchParams(location.search).has("capture")) {
    Reflect.set(state, "__fileFormatCapture", name);
    for (let attempt = 0; attempt < 200 && Reflect.get(state, "__fileFormatCaptured") !== name; attempt++) await delay(20);
    ensure(Reflect.get(state, "__fileFormatCaptured") === name, `capture completed: ${name}`);
  }
  return metrics;
}

async function verifyInspectorBadgeGeometry() {
  const resourceFetch = window.fetch;
  let imageBytes: number[] = [];
  const imageData = (width: number, height: number) => {
    const canvas = document.createElement("canvas"); canvas.width = width; canvas.height = height;
    const context = canvas.getContext("2d")!;
    context.fillStyle = "#385465"; context.fillRect(0, 0, width, height);
    context.fillStyle = "#c5d7df"; context.font = "24px sans-serif"; context.fillText(`${width} × ${height}`, 24, 50);
    return Array.from(atob(canvas.toDataURL("image/webp").split(",")[1]), (c) => c.charCodeAt(0));
  };
  window.fetch = async () => new Response(new Uint8Array(imageBytes), { headers: { "Content-Type": "image/webp" } });
  const secondary: MediaRecord = { ...file, id: 90, name: "Portrait.gif", ext: "gif", previewPlaceholder: imageData(120, 240), previewPlaceholderFormat: "image/webp" };
  let media: MediaRecord = { ...file, id: 91, name: "Landscape.mp4", ext: "mp4", kind: "video", width: 640, height: 360, thumbnailState: "ready", presentation: { previewKind: "original-video", thumbnail: "available", generation: 1 } };
  let thumbnail: ThumbnailResponse = { fileId: media.id, state: "ready", target: "grid_320", updatedAt: 100, shortSidePx: 320, outputFormat: "image/webp", width: 640, height: 360, byteSize: 1, servedBy: "db_blob", asset: null, error: null };
  const results: Awaited<ReturnType<typeof captureBadgeGeometry>> = [];
  const render = async (width = 240) => {
    await act(async () => root.render(<div style={{ display: "flex", gap: 24, padding: 20 }}>
      <div id="geometry-primary" style={{ width }}><PreviewPanel selectedFolder={null} selectedFolderCoverMedia={[]} selectedMedia={media} thumbnail={thumbnail} /></div>
      <div id="geometry-secondary" style={{ width: 160 }}><PreviewPanel selectedFolder={null} selectedFolderCoverMedia={[]} selectedMedia={secondary} /></div>
    </div>));
    for (let attempt = 0; attempt < 80; attempt++) {
      await act(async () => { await delay(20); });
      const image = document.querySelector<HTMLImageElement>("#geometry-primary img");
      if (image?.complete && image.naturalWidth === thumbnail.width && image.naturalHeight === thumbnail.height) break;
    }
    await act(async () => { await delay(40); });
  };
  try {
    for (const [name, width, height] of [["landscape-video", 640, 360], ["portrait", 180, 360], ["square", 320, 320]] as const) {
      imageBytes = imageData(width, height);
      media = { ...media, width, height, mtime: media.mtime + 1 };
      thumbnail = { ...thumbnail, width, height, updatedAt: thumbnail.updatedAt! + 1 };
      await render();
      results.push(...await captureBadgeGeometry(name));
    }
    // Reuse the same media/component identity with dimensions absent from the record.
    imageBytes = imageData(640, 360);
    media = { ...media, width: null, height: null, mtime: media.mtime + 1 };
    thumbnail = { ...thumbnail, width: 640, height: 360, updatedAt: thumbnail.updatedAt! + 1 };
    await render(); results.push(...await captureBadgeGeometry("source-replaced"));
    await render(180); results.push(...await captureBadgeGeometry("sidebar-resized"));
    await act(async () => document.querySelector<HTMLImageElement>("#geometry-primary img")!.dispatchEvent(new Event("error")));
    results.push(...await captureBadgeGeometry("failed-artwork"));
    for (const ext of ["JPG", "jpeg", "PnG"]) {
      media = { ...file, ext };
      await act(async () => root.render(<PreviewPanel selectedFolder={null} selectedFolderCoverMedia={[]} selectedMedia={media} />));
      ensure(!document.querySelector(".file-format-badge"), `${ext}: badge remains excluded`);
    }
    for (const result of results) {
      ensure(result.inside, `badge is within actual image/artwork: ${JSON.stringify(result)}`);
      ensure(Math.abs(result.insetX! - 5) < 0.1 && Math.abs(result.insetY! - 5) < 0.1, "badge follows content top-left with 5px inset");
    }
    for (const entry of badgeMetrics) {
      for (const metric of entry.metrics as typeof results) {
        ensure(metric.opacity === "0.8", `${entry.name}: whole badge opacity is 0.8`);
        ensure(metric.transform === "matrix(0.9, 0, 0, 0.9, 0, 0)", `${entry.name}: uniform 90% dimensions`);
        ensure(Math.abs(metric.rect.width - metric.layoutWidth * 0.9) < 0.02 && Math.abs(metric.rect.height - metric.layoutHeight * 0.9) < 0.02, `${entry.name}: painted dimensions are 90% of layout dimensions`);
        ensure(metric.borderColor.endsWith(" / 0.4)"), `${entry.name}: weaker 40% format outline`);
      }
    }
  } finally { window.fetch = resourceFetch; }
}
function Fixture({ mode, media = file, thumbnail }: { mode: LibraryLayoutMode; media?: MediaRecord; thumbnail?: ThumbnailResponse }) {
  const [preview, setPreview] = useState(false);
  return <>
    <div style={{ height: 360, width: 620, position: "relative" }}><MediaGrid gridPreferences={DEFAULT_LIBRARY_GRID_PREFERENCES} items={[media]} layoutMode={mode} loading={false} loadingMore={false} selectedMediaId={media.id} onSelect={() => selected++} onOpenPreview={() => { opened++; setPreview(true); }} onLocateFolder={() => located++} onRequestThumbnailStates={() => {}} thumbnailStatesByMediaId={thumbnail ? { [media.id]: thumbnail } : {}} /></div>
    <div id="inspector" style={{ height: 120, width: 120 }}><PreviewPanel selectedFolder={null} selectedFolderCoverMedia={[]} selectedMedia={media} thumbnail={thumbnail} /></div>
    <div id="cover" style={{ height: 100, width: 100 }}><SubfolderCard coverLoaded coverMedia={[media]} folder={folder} selected={false} onOpenFolder={() => {}} onSelectFolder={() => {}} /></div>
    {preview && <div id="central" style={{ width: 200, height: 120 }}><MediaPreview media={media} source="original" thumbnail={thumbnail} /></div>}
  </>;
}

let liveLibrary: LibraryState | null = null;
const coverFolders = [folder];
function PresentationFixture() {
  const library = useLibraryData({ persistentThumbnailCacheAutoRefresh: false });
  const covers = useFolderCovers(coverFolders);
  liveLibrary = library;
  return <><div style={{ height: 360, width: 620, position: "relative" }}><MediaGrid gridPreferences={DEFAULT_LIBRARY_GRID_PREFERENCES} items={library.media} mediaSlots={library.mediaSlots} totalCount={library.mediaTotalCount} layoutMode="list" loading={library.loading} loadingMore={library.loadingMoreMedia} selectedMediaId={library.selectedMediaId} onSelect={library.setSelectedMediaId} onOpenPreview={() => {}} onLocateFolder={() => {}} onRequestMediaWindow={library.requestMediaWindow} onRequestThumbnailStates={() => {}} thumbnailStatesByMediaId={{}} /></div><output id="presentation" data-media={library.media[0]?.presentation?.generation}
    data-unselected={library.media.find((item) => item.id === 12)?.presentation?.generation} data-cover={covers.get(folder.id)?.[0]?.presentation?.generation}
    data-selected={library.selectedMedia?.presentation?.generation} /></>;
}

async function verifyPresentationInvalidation() {
  const originalBridge = window.mangooDesktop;
  // Production Desktop finishes its phase stream at loading-media. App's
  // later ready state is local and is not published back through this bridge.
  window.mangooDesktop = { ...originalBridge, getLibraryPhase: async () => "loading-media" };
  let generation = 1;
  let runtimeState = "starting";
  let pluginReads = 0;
  let pendingStatus: AbortSignal | null = null;
  let stopStatusResponse = false;
  let delayedCover: { resolve: (response: Response) => void; response: Response; signal?: AbortSignal | null } | null = null;
  const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
  window.fetch = async (input, init) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
    const media = { ...file, presentation: { ...file.presentation!, generation } };
    if (url.pathname === "/api/libraries/active") return json({ library: { libraryUuid: "fixture", name: "Fixture" }, contextRevision: 1, runtimeGeneration: 1 });
    if (url.pathname === "/api/plugins") {
      pluginReads++;
      if (stopStatusResponse) {
        pendingStatus = init?.signal ?? null;
        return new Promise<Response>((_resolve, reject) => init?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")), { once: true }));
      }
      return json({ items: [{ pluginId: "com.example.decoder", selectedVersion: "1", lifecycleEpoch: 1,
        capabilities: ["file-decoder.v1"], enabledIntent: true, runtimeState, permissionRequired: false, grantedPermissions: [] }] });
    }
    if (url.pathname === "/api/roots") return json({ items: [{ id: 1, path: "C:/Fixture", displayName: "Fixture", enabled: true, createdAt: 1, lastScanAt: null, mediaCount: 0, fileCount: generation === 1 ? 2 : 3, reachable: true, rootFolderId: null }], nextCursor: null });
    if (url.pathname === "/api/media") {
      const rootItems = [media, { ...media, id: 12, name: "Other.unusualsuffix" }];
      if (generation > 1) rootItems.push({ ...media, id: 13, name: "Watched.unusualsuffix" });
      const response = json({ items: url.searchParams.get("folderId") === "2" ? [media] : rootItems, nextCursor: null, totalCount: rootItems.length });
      if (generation === 4 && url.searchParams.get("folderId") === "2") {
        return new Promise<Response>((resolve) => { delayedCover = { resolve, response, signal: init?.signal }; });
      }
      return response;
    }
    if (url.pathname === `/api/media/${file.id}`) return json(media);
    if (url.pathname.endsWith("/metadata")) return json({ fileId: file.id, rating: null, favorite: false, note: null, tags: [] });
    return json({ items: [], nextCursor: null, totalCount: 0 });
  };
  const waitFor = async (test: () => boolean, message: string) => {
    for (let attempt = 0; attempt < 200 && !test(); attempt++) await act(async () => { await delay(20); });
    ensure(test(), message);
  };
  const isCurrent = (value: number) => {
    const output = document.querySelector<HTMLElement>("#presentation");
    return output?.dataset.media === String(value) && output.dataset.unselected === String(value) && output.dataset.cover === String(value) && output.dataset.selected === String(value);
  };
  await act(async () => root.render(<PresentationFixture />));
  await waitFor(() => isCurrent(1), "initial media, selected item and folder cover loaded");
  const selectedId = liveLibrary!.selectedMediaId;
  generation = 2;
  await act(async () => broadcastPluginChange());
  await waitFor(() => isCurrent(2), "same-window capability change refreshed all local caches");
  await waitFor(() => liveLibrary!.roots[0]?.fileCount === 3, "fresh all-file total reconciles stale root count without a root scan");
  ensure(liveLibrary!.selectedMediaId === selectedId && liveLibrary!.mediaTotalCount === 3, "refresh preserves selection and updates count");
  generation = 3;
  const remote = new BroadcastChannel("mangoo-plugin-state");
  remote.postMessage({ kind: "changed", source: "other-window-fixture" });
  await waitFor(() => isCurrent(3), "other-window capability change refreshed all local caches");
  remote.close();
  generation = 4;
  await act(async () => broadcastPluginChange());
  await waitFor(() => delayedCover !== null, "old folder-cover query started");
  generation = 5;
  await act(async () => broadcastPluginChange());
  await waitFor(() => isCurrent(5), "new capability superseded old folder-cover query");
  const readDelayedCover = () => delayedCover;
  const oldCover = readDelayedCover();
  ensure(oldCover, "old folder-cover request retained for late completion");
  ensure(oldCover.signal?.aborted, "old folder-cover transport aborted");
  oldCover.resolve(oldCover.response);
  await act(async () => { await delay(40); });
  ensure(isCurrent(5), "old folder-cover completion cannot overwrite new capability");
  // There is no plugin window mounted here. Its enable broadcast can precede
  // process registration; the shared Host monitor must observe later readiness.
  generation = 6;
  runtimeState = "ready";
  await waitFor(() => isCurrent(6), "delayed decoder readiness refreshes after plugin window closes");
  ensure(pluginReads <= 3, "multiple cache subscribers share one bounded status monitor");
  stopStatusResponse = true;
  await waitFor(() => pendingStatus !== null, "next bounded status read started");
  await act(async () => root.render(null));
  window.mangooDesktop = originalBridge;
  const readPendingStatus = () => pendingStatus;
  ensure(readPendingStatus()?.aborted, "last subscriber cancels pending Host read");
}

async function run() {
  const modes: string[] = [];
  try {
    resetMediaCachesForLibrarySwitch();
    for (const mode of ["adaptive", "waterfall", "grid", "list"] as const) {
      await act(async () => { root.render(<Fixture key={mode} mode={mode} />); await delay(80); });
      const tile = document.querySelector<HTMLElement>(".media-tile");
      ensure(tile, `${mode}: tile mounted`);
      ensure(tile.querySelector(".file-format-artwork"), `${mode}: immediate artwork`);
      const badge = tile.querySelector<HTMLElement>(".file-format-badge")!;
      const shell = tile.querySelector<HTMLElement>(".media-tile-thumb-shell")!;
      ensure(getComputedStyle(badge).pointerEvents === "none", `${mode}: passive metadata`);
      ensure(getComputedStyle(badge).borderTopWidth === "1px", `${mode}: outline`);
      ensure(badge.getBoundingClientRect().right <= shell.getBoundingClientRect().right - 20, `${mode}: locate reserved`);
      await act(async () => { tile.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })); await delay(20); });
      ensure(document.querySelector("#central .file-format-artwork"), `${mode}: static double click`);
      ensure(!document.querySelector("#central img, #central video, #central canvas"), `${mode}: no raw player`);
      ensure(document.querySelector("#inspector .file-format-artwork"), `${mode}: inspector art`);
      ensure(document.querySelector("#cover .file-format-artwork"), `${mode}: cover art`);
      await captureBadgeGeometry(mode);
      await act(async () => tile.querySelector<HTMLButtonElement>(".media-tile-locate-button")!.click());
      modes.push(mode);
    }
    ensure(requested.length === 0, "unknown files never request raw bytes or unavailable thumbnails");
    await requestOriginalPreviewBlob(file).then(() => { throw new Error("raw request allowed"); }, () => {});
    ensure((await requestThumbnailState(file)).state === "unavailable", "unavailable terminal");
    const changed = { ...file, presentation: { ...file.presentation!, generation: 2, thumbnail: "available" as const } };
    ensure(mediaContentSignature(file) !== mediaContentSignature(changed), "capability re-evaluation signature");
    ensure(mediaFileContentSignature(file) === mediaFileContentSignature(changed), "source byte identity retained");
    const thumbnail: ThumbnailResponse = { fileId: file.id, state: "ready", target: "grid_320", updatedAt: 42, shortSidePx: 320, outputFormat: "image/webp", width: 1, height: 1, byteSize: bytes.length, servedBy: "db_blob", asset: null, error: null };
    await act(async () => { root.render(<Fixture mode="grid" media={{ ...changed, thumbnailState: "ready" }} thumbnail={thumbnail} />); await delay(120); });
    await act(async () => { await delay(150); });
    ensure(document.querySelector(".media-tile img"), "ready thumbnail upgrades artwork");
    ensure(document.querySelector(".media-tile.selected .file-format-badge"), "upgrade preserves selection and format");
    ensure(requested.every((url) => url.includes("/thumbnail")), "small surfaces use only thumbnails");
    await act(async () => { document.querySelector(".media-tile img")!.dispatchEvent(new Event("error")); });
    ensure(document.querySelector(".media-tile .file-format-artwork"), "bad loaded thumbnail returns to artwork");
    await act(async () => { root.render(<MediaPreview media={changed} source="original" thumbnail={thumbnail} />); await delay(100); });
    await act(async () => { await delay(100); });
    ensure(requested.some((url) => url.includes(`/media/${file.id}/preview`)), "available decoder uses authenticated preview endpoint");
    ensure(document.querySelector('[data-preview-source="original"] img'), "decoded preview renders as image");
    ensure(!document.querySelector("video, canvas"), "static decoder never enters raw video or canvas lane");
    const previewRequests = requested.filter((url) => url.includes("/preview")).length;
    await requestOriginalPreviewBlob({ ...changed, thumbnailState: "ready" });
    ensure(requested.filter((url) => url.includes("/preview")).length === previewRequests, "thumbnail polling retains decoded preview bytes");
    await requestOriginalPreviewBlob({ ...changed, presentation: { ...changed.presentation, generation: 3 } });
    ensure(requested.filter((url) => url.includes("/preview")).length === previewRequests + 1, "decoder generation changes invalidate decoded preview bytes");
    await verifyInspectorBadgeGeometry();
    await verifyPresentationInvalidation();
    Reflect.set(state, "__overlayLifecycleResult", { ok: true, modes, selected, opened, located, requests: requested.length, presentationInvalidation: true, badgeGeometry: true });
  } catch (error) { Reflect.set(state, "__overlayLifecycleResult", { ok: false, error: error instanceof Error ? error.stack : String(error) }); }
  finally { await act(async () => root.unmount()); window.fetch = originalFetch; resetMediaCachesForLibrarySwitch(); }
}
void run();

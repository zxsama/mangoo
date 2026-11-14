import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CoreApiError, createCoreClient } from "@mangoo/web/plugin-host";
import type { ImageViewerFolderEntry } from "@mangoo/core-client";

import { extensionOf, splitPath } from "../shared/formats.ts";
import {
  collapseRawPairs,
  indexAfterRemoval,
  indexOfPath,
  samePath,
  sortEntries,
  stepIndex,
  type ImageEntry,
  type ImageSortDirection,
  type ImageSortKey
} from "../shared/navigation.ts";

export interface ViewerSessionState {
  entries: ImageEntry[];
  index: number;
  directory: string | null;
  /** "folder": browsing a whole folder. "selection": the files the user opened. */
  mode: "folder" | "selection";
  loading: boolean;
  error: string | null;
  truncated: boolean;
}

const EMPTY: ViewerSessionState = {
  entries: [],
  index: -1,
  directory: null,
  mode: "folder",
  loading: false,
  error: null,
  truncated: false
};

export function describeError(error: unknown): string {
  if (error instanceof CoreApiError) {
    const body = error.body as { error?: string; code?: string } | undefined;
    if (body?.code === "image_viewer_disabled") return "看图插件未启用。请在插件管理中启用 Mangoo 看图。";
    if (body?.code === "unsupported_image_format") return "Mangoo 暂不支持这种图片格式。";
    if (body?.code === "image_decode_failed") return "这张图片无法显示，文件可能已损坏，或使用了不支持的编码。";
    if (body?.code === "invalid_image_path") return "无法识别这个文件路径。";
    if (error.status === 404) return "找不到这个文件，它可能已被移动或删除。";
    return `读取图片失败（错误 ${error.status}）。`;
  }
  // Decode failures surface from the browser (createImageBitmap / <img>) in
  // English and without detail the user can act on.
  const message = error instanceof Error ? error.message : String(error);
  if (/decod|image|bitmap|显示/i.test(message)) return "这张图片无法显示，文件可能已损坏，或使用了不支持的编码。";
  return message;
}

function toEntry(entry: ImageViewerFolderEntry): ImageEntry {
  return {
    path: entry.path,
    name: entry.name,
    ext: entry.ext,
    sizeBytes: entry.sizeBytes,
    modifiedAtMs: entry.modifiedAtMs,
    cameraRaw: entry.cameraRaw,
    ...(entry.pairPath ? { pairPath: entry.pairPath } : {})
  };
}

function fallbackEntry(path: string): ImageEntry {
  return { path, name: splitPath(path).name, ext: extensionOf(path), sizeBytes: 0, modifiedAtMs: 0 };
}

export { hiddenPairOf } from "../shared/navigation.ts";

export function useViewerSession(sortKey: ImageSortKey, sortDirection: ImageSortDirection, loop: boolean, pairRawJpeg = true) {
  const [state, setState] = useState<ViewerSessionState>(EMPTY);
  const requestToken = useRef(0);
  const stateRef = useRef(state);
  stateRef.current = state;

  const sortRef = useRef({ sortKey, sortDirection, pairRawJpeg });
  sortRef.current = { sortKey, sortDirection, pairRawJpeg };
  /** Every entry of the folder, before pairing (pairing is a view on it). */
  const allEntries = useRef<ImageEntry[]>([]);

  /** The file the user explicitly opened: it is never hidden behind its pair. */
  const openedPath = useRef<string | null>(null);

  const apply = useCallback((entries: ImageEntry[], focusPath: string | null, extra: Partial<ViewerSessionState>) => {
    allEntries.current = entries;
    const mode = extra.mode ?? stateRef.current.mode;
    const visible = sortRef.current.pairRawJpeg && mode === "folder" ? collapseRawPairs(entries, openedPath.current) : entries;
    const sorted = sortEntries(visible, sortRef.current.sortKey, sortRef.current.sortDirection);
    let found = focusPath ? indexOfPath(sorted, focusPath) : -1;
    if (found < 0 && focusPath) {
      // The focused file is a hidden pair member: focus its RAW.
      const hidden = entries.find((entry) => samePath(entry.path, focusPath));
      if (hidden?.pairPath) found = indexOfPath(sorted, hidden.pairPath);
    }
    setState((current) => ({
      ...current,
      ...extra,
      entries: sorted,
      index: sorted.length === 0 ? -1 : found >= 0 ? found : 0,
      loading: false,
      error: null
    }));
  }, []);

  /** Open a request: one file (its folder), one folder, or a set of files. */
  const open = useCallback(async (paths: readonly string[]) => {
    if (paths.length === 0) return;
    const token = ++requestToken.current;
    setState((current) => ({ ...current, loading: true, error: null }));
    const client = createCoreClient();
    try {
      openedPath.current = null;
      if (paths.length === 1) {
        const listing = await client.getImageViewerFolder(paths[0]);
        if (token !== requestToken.current) return;
        const entries = listing.entries.map(toEntry);
        const isFile = indexOfPath(entries, paths[0]) >= 0 || extensionOf(paths[0]) !== "";
        openedPath.current = isFile ? paths[0] : null;
        if (isFile && indexOfPath(entries, paths[0]) < 0) entries.push(fallbackEntry(paths[0]));
        apply(entries, isFile ? paths[0] : null, { directory: listing.directory, mode: "folder", truncated: listing.truncated });
        return;
      }
      const byDirectory = new Map<string, string[]>();
      for (const path of paths) {
        const { directory } = splitPath(path);
        byDirectory.set(directory, [...(byDirectory.get(directory) ?? []), path]);
      }
      const entries: ImageEntry[] = [];
      for (const [directory, selected] of byDirectory) {
        try {
          const listing = await client.getImageViewerFolder(directory);
          for (const path of selected) {
            const match = listing.entries.find((entry) => samePath(entry.path, path));
            entries.push(match ? toEntry(match) : fallbackEntry(path));
          }
        } catch {
          entries.push(...selected.map(fallbackEntry));
        }
      }
      if (token !== requestToken.current) return;
      apply(entries, paths[0], { directory: byDirectory.size === 1 ? [...byDirectory.keys()][0] : null, mode: "selection", truncated: false });
    } catch (error) {
      if (token !== requestToken.current) return;
      setState((current) => ({ ...current, loading: false, error: describeError(error) }));
    }
  }, [apply]);

  /** Re-list the folder (files added/removed outside), keeping the current picture. */
  const refresh = useCallback(async () => {
    const current = stateRef.current;
    if (current.mode !== "folder" || !current.directory || current.loading) return;
    const focus = current.entries[current.index]?.path ?? null;
    try {
      const listing = await createCoreClient().getImageViewerFolder(current.directory, { requestPriority: "background" });
      const entries = listing.entries.map(toEntry);
      const known = allEntries.current;
      const unchanged = entries.length === known.length
        && entries.every((entry) => known.some((other) => samePath(other.path, entry.path) && other.modifiedAtMs === entry.modifiedAtMs));
      if (unchanged) return;
      if (focus && indexOfPath(entries, focus) < 0) {
        // The picture on screen vanished: keep the position, like deleting.
        allEntries.current = entries;
        const visible = sortRef.current.pairRawJpeg && current.mode === "folder" ? collapseRawPairs(entries, openedPath.current) : entries;
        const sorted = sortEntries(visible, sortRef.current.sortKey, sortRef.current.sortDirection);
        setState((value) => ({ ...value, entries: sorted, index: indexAfterRemoval(sorted.length, value.index), truncated: listing.truncated }));
        return;
      }
      apply(entries, focus, { truncated: listing.truncated });
    } catch {
      // Folder temporarily unreachable (NAS blip): keep what is shown.
    }
  }, [apply]);

  // Re-sort / re-pair when those preferences change.
  useEffect(() => {
    const current = stateRef.current;
    if (current.entries.length === 0) return;
    apply(allEntries.current.length ? allEntries.current : current.entries, current.entries[current.index]?.path ?? null, {});
  }, [apply, sortKey, sortDirection, pairRawJpeg]);

  const go = useCallback((delta: number) => {
    setState((current) => {
      if (current.entries.length === 0) return current;
      const index = stepIndex(current.entries.length, current.index, delta, loop);
      return index === current.index ? current : { ...current, index };
    });
  }, [loop]);

  const goTo = useCallback((index: number) => {
    setState((current) => (index >= 0 && index < current.entries.length ? { ...current, index } : current));
  }, []);

  /** Forget files that left the folder (deleted), keeping both lists in step. */
  const removePaths = useCallback((paths: readonly string[]) => {
    const gone = (path: string) => paths.some((other) => samePath(other, path));
    allEntries.current = allEntries.current.filter((entry) => !gone(entry.path));
    setState((current) => {
      if (current.index < 0) return current;
      const entries = current.entries.filter((entry) => !gone(entry.path));
      return { ...current, entries, index: indexAfterRemoval(entries.length, current.index) };
    });
  }, []);

  const removeCurrent = useCallback(() => {
    const current = stateRef.current;
    const entry = current.entries[current.index];
    if (entry) removePaths([entry.path]);
  }, [removePaths]);

  /** Renamed files (`from` -> `to`), applied to every entry list. */
  const renamePaths = useCallback((renames: ReadonlyArray<{ from: string; to: string }>) => {
    const target = (path: string) => renames.find((rename) => samePath(rename.from, path))?.to ?? null;
    const rewrite = (entry: ImageEntry): ImageEntry => {
      const to = target(entry.path);
      const pair = entry.pairPath ? target(entry.pairPath) ?? entry.pairPath : entry.pairPath;
      if (!to) return pair === entry.pairPath ? entry : { ...entry, pairPath: pair };
      return { ...entry, path: to, name: splitPath(to).name, ext: extensionOf(to), ...(pair ? { pairPath: pair } : {}) };
    };
    const current = stateRef.current;
    const focus = current.entries[current.index]?.path ?? null;
    // The explicitly opened file keeps its identity through a rename.
    if (openedPath.current) openedPath.current = target(openedPath.current) ?? openedPath.current;
    const base = allEntries.current.length ? allEntries.current : current.entries;
    apply(base.map(rewrite), focus ? target(focus) ?? focus : null, {});
  }, [apply]);

  const replaceCurrent = useCallback((path: string) => {
    const current = stateRef.current;
    const entry = current.entries[current.index];
    if (entry) renamePaths([{ from: entry.path, to: path }]);
  }, [renamePaths]);

  const current = state.index >= 0 ? state.entries[state.index] ?? null : null;
  return useMemo(
    () => ({ state, current, open, refresh, go, goTo, removeCurrent, removePaths, replaceCurrent, renamePaths }),
    [state, current, open, refresh, go, goTo, removeCurrent, removePaths, replaceCurrent, renamePaths]
  );
}

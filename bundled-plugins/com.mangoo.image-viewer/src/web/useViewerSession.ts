import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CoreApiError, createCoreClient } from "@mangoo/web/plugin-host";
import type { ImageViewerFolderEntry } from "@mangoo/core-client";

import { extensionOf, splitPath } from "../shared/formats.ts";
import {
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
  return { path: entry.path, name: entry.name, ext: entry.ext, sizeBytes: entry.sizeBytes, modifiedAtMs: entry.modifiedAtMs };
}

function fallbackEntry(path: string): ImageEntry {
  return { path, name: splitPath(path).name, ext: extensionOf(path), sizeBytes: 0, modifiedAtMs: 0 };
}

export function useViewerSession(sortKey: ImageSortKey, sortDirection: ImageSortDirection, loop: boolean) {
  const [state, setState] = useState<ViewerSessionState>(EMPTY);
  const requestToken = useRef(0);
  const stateRef = useRef(state);
  stateRef.current = state;

  const sortRef = useRef({ sortKey, sortDirection });
  sortRef.current = { sortKey, sortDirection };

  const apply = useCallback((entries: ImageEntry[], focusPath: string | null, extra: Partial<ViewerSessionState>) => {
    const sorted = sortEntries(entries, sortRef.current.sortKey, sortRef.current.sortDirection);
    const found = focusPath ? indexOfPath(sorted, focusPath) : -1;
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
      if (paths.length === 1) {
        const listing = await client.getImageViewerFolder(paths[0]);
        if (token !== requestToken.current) return;
        const entries = listing.entries.map(toEntry);
        const isFile = indexOfPath(entries, paths[0]) >= 0 || extensionOf(paths[0]) !== "";
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
      const unchanged = entries.length === current.entries.length
        && entries.every((entry) => current.entries.some((other) => samePath(other.path, entry.path) && other.modifiedAtMs === entry.modifiedAtMs));
      if (unchanged) return;
      if (focus && indexOfPath(entries, focus) < 0) {
        // The picture on screen vanished: keep the position, like deleting.
        const sorted = sortEntries(entries, sortRef.current.sortKey, sortRef.current.sortDirection);
        setState((value) => ({ ...value, entries: sorted, index: indexAfterRemoval(sorted.length, value.index), truncated: listing.truncated }));
        return;
      }
      apply(entries, focus, { truncated: listing.truncated });
    } catch {
      // Folder temporarily unreachable (NAS blip): keep what is shown.
    }
  }, [apply]);

  // Re-sort when the sort preference changes.
  useEffect(() => {
    const current = stateRef.current;
    if (current.entries.length === 0) return;
    apply(current.entries, current.entries[current.index]?.path ?? null, {});
  }, [apply, sortKey, sortDirection]);

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

  const removeCurrent = useCallback(() => {
    setState((current) => {
      if (current.index < 0) return current;
      const entries = current.entries.filter((_, index) => index !== current.index);
      return { ...current, entries, index: indexAfterRemoval(entries.length, current.index) };
    });
  }, []);

  const replaceCurrent = useCallback((path: string) => {
    const current = stateRef.current;
    const entry = current.entries[current.index];
    if (!entry) return;
    const renamed: ImageEntry = { ...entry, path, name: splitPath(path).name, ext: extensionOf(path) };
    apply(current.entries.map((item, index) => (index === current.index ? renamed : item)), path, {});
  }, [apply]);

  const current = state.index >= 0 ? state.entries[state.index] ?? null : null;
  return useMemo(
    () => ({ state, current, open, refresh, go, goTo, removeCurrent, replaceCurrent }),
    [state, current, open, refresh, go, goTo, removeCurrent, replaceCurrent]
  );
}

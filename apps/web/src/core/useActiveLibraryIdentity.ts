import { useEffect, useRef, useState } from "react";
import { createCoreClient } from "./client";
import { listLibraries, subscribeLibraryRegistry } from "./desktop";
import type { LibrarySummary, LibraryLoadPhase } from "./libraryTypes";

/** Registry-only metadata refreshes never initiate a catalog/context transition. */
export function useActiveLibraryIdentity(phase: LibraryLoadPhase, notebookEnabled = false) {
  const [context, setContext] = useState<{ library: LibrarySummary | null; contextRevision: number } | null>(null);
  const active = context?.library;
  const loaded = useRef(false);
  useEffect(() => {
    if (loaded.current && phase !== "ready") return;
    loaded.current = true;
    let cancelled = false;
    let generation = 0;
    const reload = () => {
      const owner = ++generation;
      void listLibraries().then(async libraries => {
        const entry = libraries.find(library => library.active) ?? null;
        // The registry owns appearance. Only notebook operations also need an
        // immutable activation revision from the runtime that will receive them.
        const snapshot = entry?.kind === "notes" ? await createCoreClient().getActiveLibrary() : null;
        if (cancelled || owner !== generation || (snapshot && snapshot.library?.libraryUuid !== entry?.libraryUuid)) return;
        setContext({ library: entry, contextRevision: snapshot?.contextRevision ?? 0 });
      }).catch(() => { /* Retain the last confirmed identity; the next registry event retries. */ });
    };
    reload();
    const unsubscribe = subscribeLibraryRegistry(reload);
    return () => { cancelled = true; unsubscribe(); };
  }, [phase, notebookEnabled]);
  return {
    activeLibraryId: phase === "loading-media" ? null : active?.libraryUuid ?? null,
    activeContextRevision: context?.contextRevision ?? 0,
    activeLibraryKind: active?.kind ?? "media",
    activeLibraryName: active?.name ?? "Library",
    activeLibraryIcon: active?.icon ?? undefined,
    activeLibraryColor: active?.color ?? undefined,
  };
}

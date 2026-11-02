import { useEffect, useRef, useState } from "react";
import { listLibraries, subscribeLibraryRegistry } from "./desktop";
import type { LibraryLoadPhase, LibrarySummary } from "./libraryTypes";

/** Registry-only metadata refreshes never initiate a catalog/context transition. */
export function useActiveLibraryIdentity(phase: LibraryLoadPhase) {
  const [active, setActive] = useState<LibrarySummary | null>(null);
  const loaded = useRef(false);
  useEffect(() => {
    if (loaded.current && phase !== "ready") return;
    loaded.current = true;
    let cancelled = false;
    let generation = 0;
    const reload = () => {
      const owner = ++generation;
      void listLibraries().then((libraries) => {
        if (!cancelled && owner === generation) setActive(libraries.find((entry) => entry.active) ?? null);
      }).catch(() => { /* Retain the last confirmed identity; the next registry event retries. */ });
    };
    reload();
    const unsubscribe = subscribeLibraryRegistry(reload);
    return () => { cancelled = true; unsubscribe(); };
  }, [phase]);
  return {
    activeLibraryId: phase === "loading-media" ? null : active?.libraryUuid ?? null,
    activeLibraryName: active?.name ?? "Library",
    activeLibraryIcon: active?.icon ?? undefined,
    activeLibraryColor: active?.color ?? undefined,
  };
}

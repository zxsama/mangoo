import type { LibraryLoadPhase, LibrarySummary } from "./libraryTypes.ts";
import type { NotificationLibraryContext } from "./notificationReporting.ts";

interface LibraryContextTrackerOptions {
  loadLibraries: () => Promise<LibrarySummary[]>;
  onContext: (context: NotificationLibraryContext | null) => void;
}

export interface LibraryContextTracker {
  start(): Promise<void>;
  onPhase(phase: LibraryLoadPhase): Promise<void>;
  dispose(): void;
}

export function createLibraryContextTracker(
  options: LibraryContextTrackerOptions
): LibraryContextTracker {
  let generation = 0;
  let disposed = false;

  const refresh = async (requestGeneration: number) => {
    try {
      const libraries = await options.loadLibraries();
      if (disposed || generation !== requestGeneration) return;
      const active = libraries.find((library) => library.active);
      options.onContext(
        active ? { libraryId: active.libraryUuid, libraryName: active.name } : null
      );
    } catch {
      if (!disposed && generation === requestGeneration) {
        options.onContext(null);
      }
    }
  };

  return {
    start() {
      generation += 1;
      return refresh(generation);
    },
    onPhase(phase) {
      generation += 1;
      const requestGeneration = generation;
      if (disposed) return Promise.resolve();

      // Desktop emits loading-media only after the healthy Core and active registry commit.
      if (phase === "loading-media" || phase === "ready") {
        options.onContext(null);
        return refresh(requestGeneration);
      }

      options.onContext(null);
      return Promise.resolve();
    },
    dispose() {
      disposed = true;
      generation += 1;
    }
  };
}

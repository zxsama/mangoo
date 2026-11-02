import type { PlaybackSession } from "@mangoo/core-client";
import { createCoreClient } from "./client";

export interface VideoPlaybackLease {
  session: PlaybackSession;
  release(): void;
}

/** An abortable session lease, deliberately separate from image/Blob caches. */
export async function createVideoPlaybackLease(fileId: number, signal: AbortSignal, onFailure: () => void, onReplace: (src: string) => void): Promise<VideoPlaybackLease> {
  const client = createCoreClient();
  let session = await client.createPlaybackSession(fileId, { signal });
  let timer: ReturnType<typeof setTimeout> | undefined;
  let released = false;
  const revoke = (id: string) => { void client.revokePlaybackSession(id).catch(() => { /* Server TTL covers a disconnected client. */ }); };
  const release = () => {
    if (released) return;
    released = true;
    clearTimeout(timer);
    signal.removeEventListener("abort", release);
    revoke(session.id);
  };
  if (signal.aborted) { release(); throw new DOMException("Playback cancelled", "AbortError"); }
  signal.addEventListener("abort", release, { once: true });
  const renew = () => {
    timer = setTimeout(() => {
      void client.renewPlaybackSession(session.id, { signal }).then(async (next) => {
        if (released || signal.aborted) return;
        // Close to the absolute deadline, acquire a fresh authenticated grant
        // while retaining the same single renderer owner and playback position.
        if (next.renewAfterMs < 10_000) {
          const replacement = await client.createPlaybackSession(fileId, { signal });
          if (released || signal.aborted) { revoke(replacement.id); return; }
          revoke(session.id);
          session = replacement;
          onReplace(replacement.src);
          renew();
          return;
        }
        session = next;
        renew();
      }).catch(() => { if (!released) { release(); onFailure(); } });
    }, Math.max(1, session.renewAfterMs));
  };
  renew();
  return { session, release };
}

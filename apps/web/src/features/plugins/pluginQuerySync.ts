import { useEffect } from "react";
import type { QueryClient } from "@tanstack/react-query";
import type { HostPluginProjection } from "@mangoo/core-client";
import { createCoreClient } from "../../core/client";
import { subscribeLibraryPhase } from "../../core/desktop";

const CHANNEL_NAME = "mangoo-plugin-state";
const SOURCE_ID = `${Date.now()}-${Math.random()}`;
const subscriptions = new WeakMap<QueryClient, { channel: BroadcastChannel; owners: number }>();
const localChangeListeners = new Set<() => void>();
let stopDecoderMonitor: (() => void) | null = null;

function decoderCapabilityFingerprint(plugins: HostPluginProjection[]): string {
  return JSON.stringify(plugins.filter((plugin) => plugin.capabilities.includes("file-decoder.v1"))
    .map((plugin) => [plugin.pluginId, plugin.selectedVersion, plugin.lifecycleEpoch,
      plugin.enabledIntent, plugin.runtimeState, plugin.permissionRequired,
      [...plugin.grantedPermissions].sort()]).sort((left, right) => String(left[0]).localeCompare(String(right[0]))));
}

function startDecoderMonitor(): () => void {
  const client = createCoreClient();
  let stopped = false;
  let switching = false;
  let fingerprint = "[]";
  let timer: ReturnType<typeof setTimeout> | null = null;
  let controller: AbortController | null = null;
  const pause = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    controller?.abort();
  };
  const refresh = async () => {
    if (stopped || switching || document.hidden || controller) return;
    if (timer !== null) clearTimeout(timer);
    timer = null;
    const request = new AbortController();
    controller = request;
    try {
      const response = await client.listPlugins({ signal: request.signal });
      if (stopped || request.signal.aborted) return;
      const next = decoderCapabilityFingerprint(response.items);
      if (next !== fingerprint) {
        fingerprint = next;
        broadcastPluginChange();
      }
    } catch {
      // A later bounded Host status read reconciles an offline/starting Core.
    } finally {
      controller = null;
      if (!stopped && !switching && !document.hidden) timer = setTimeout(() => void refresh(), 2000);
    }
  };
  const visibilityChanged = () => { if (document.hidden) pause(); else void refresh(); };
  document.addEventListener("visibilitychange", visibilityChanged);
  const unsubscribePhase = subscribeLibraryPhase((phase) => {
    // Desktop's final phase is loading-media: the renderer settles ready
    // locally once its data arrives, without another Desktop phase event.
    switching = phase === "starting-core" || phase === "opening-library";
    if (switching) pause(); else void refresh();
  });
  void refresh();
  return () => {
    stopped = true;
    pause();
    unsubscribePhase();
    document.removeEventListener("visibilitychange", visibilityChanged);
  };
}

function isRemotePluginChange(message: unknown): boolean {
  return Boolean(message && typeof message === "object" && "kind" in message && message.kind === "changed"
    && "source" in message && typeof message.source === "string" && message.source !== SOURCE_ID);
}

// Library media and folder covers are local caches, outside TanStack Query.
// Notify them in the initiating renderer as well as in other desktop windows.
export function subscribePluginChanges(listener: () => void): () => void {
  localChangeListeners.add(listener);
  // Enablement is asynchronous. Keep observing decoder runtime readiness even
  // if its plugin window closes before registration finishes. One Host read
  // per renderer every 2 seconds; no provider or file decoding is requested.
  if (!stopDecoderMonitor) stopDecoderMonitor = startDecoderMonitor();
  const channel = typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel(CHANNEL_NAME);
  if (channel) channel.onmessage = (event: MessageEvent<unknown>) => {
    if (isRemotePluginChange(event.data)) listener();
  };
  return () => {
    localChangeListeners.delete(listener);
    channel?.close();
    if (localChangeListeners.size === 0) {
      stopDecoderMonitor?.();
      stopDecoderMonitor = null;
    }
  };
}

export async function invalidatePluginQueries(queryClient: QueryClient): Promise<void> {
  // Cancel list/config/status reads first so an older response cannot restore stale actions.
  await queryClient.cancelQueries();
  await queryClient.invalidateQueries();
}

export function broadcastPluginChange(): void {
  for (const listener of localChangeListeners) listener();
  if (typeof BroadcastChannel === "undefined") return;
  const channel = new BroadcastChannel(CHANNEL_NAME);
  channel.postMessage({ kind: "changed", source: SOURCE_ID });
  channel.close();
}

export function usePluginQuerySync(queryClient: QueryClient): void {
  useEffect(() => {
    if (typeof BroadcastChannel === "undefined") return;
    let subscription = subscriptions.get(queryClient);
    if (!subscription) {
      const channel = new BroadcastChannel(CHANNEL_NAME);
      channel.onmessage = (event: MessageEvent<unknown>) => {
        if (isRemotePluginChange(event.data)) void invalidatePluginQueries(queryClient);
      };
      subscription = { channel, owners: 0 };
      subscriptions.set(queryClient, subscription);
    }
    subscription.owners++;
    return () => {
      if (--subscription.owners === 0) {
        subscription.channel.close();
        subscriptions.delete(queryClient);
      }
    };
  }, [queryClient]);
}

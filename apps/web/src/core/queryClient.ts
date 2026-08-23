import { QueryClient, focusManager, onlineManager } from "@tanstack/react-query";
import { createCoreClient } from "./client";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 2,
      staleTime: 2_000,
      refetchOnReconnect: true,
      refetchOnWindowFocus: true,
    },
  },
});

let wakeBridgeInstalled = false;

export function installSyncWakeBridge(): void {
  if (wakeBridgeInstalled || typeof window === "undefined") return;
  wakeBridgeInstalled = true;
  let focused = focusManager.isFocused();
  let online = onlineManager.isOnline();
  focusManager.subscribe((next) => {
    if (next && !focused) void createCoreClient().wakeSync("foreground").catch(() => undefined);
    focused = next;
  });
  onlineManager.subscribe((next) => {
    if (next && !online) void createCoreClient().wakeSync("network").catch(() => undefined);
    online = next;
  });
}

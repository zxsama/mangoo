// Cross-window preference sync. The settings UI can run in a separate OS window
// (a second Electron BrowserWindow) that shares this app's origin, so a plain
// BroadcastChannel reaches every window with zero main-process plumbing. Each
// window persists to its own localStorage AND broadcasts the change; peers apply
// the broadcast to their in-memory state so the main window updates live while
// the settings window is dragged around the screen.
//
// Payloads are plain JSON (structuredClone-safe). Every window — including the
// sender — receives its own message, so we tag messages with a per-window source
// id and ignore our own.

export type PreferenceChannelKind = "grid" | "preview" | "interfaceStyle" | "shortcuts";

interface PreferenceMessage {
  source: string;
  kind: PreferenceChannelKind;
  value: unknown;
}

const CHANNEL_NAME = "mangoo-preferences";

// A stable per-window id so a window ignores the echo of its own broadcasts.
const SOURCE_ID = `${Date.now().toString(36)}-${Math.round(performance.now()).toString(36)}`;

function createChannel(): BroadcastChannel | null {
  if (typeof window === "undefined" || typeof BroadcastChannel === "undefined") {
    return null;
  }
  try {
    return new BroadcastChannel(CHANNEL_NAME);
  } catch {
    return null;
  }
}

const channel = createChannel();

export function broadcastPreference(kind: PreferenceChannelKind, value: unknown): void {
  if (!channel) return;
  const message: PreferenceMessage = { source: SOURCE_ID, kind, value };
  try {
    channel.postMessage(message);
  } catch {
    // Non-cloneable payloads should never occur (prefs are plain JSON); ignore.
  }
}

// Subscribe to preference changes from OTHER windows. Returns an unsubscribe fn.
export function subscribePreference(
  kind: PreferenceChannelKind,
  onChange: (value: unknown) => void
): () => void {
  if (!channel) return () => undefined;
  const handler = (event: MessageEvent<PreferenceMessage>) => {
    const message = event.data;
    if (!message || message.source === SOURCE_ID || message.kind !== kind) {
      return;
    }
    onChange(message.value);
  };
  channel.addEventListener("message", handler);
  return () => channel.removeEventListener("message", handler);
}

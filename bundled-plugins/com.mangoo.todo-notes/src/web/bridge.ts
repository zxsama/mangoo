import { getDesktopBridge } from "@mangoo/web/plugin-host";

import type { TodoDesktopBridge, TodoHostWindowBridge } from "../shared/ipc.ts";

type TodoBridgeHost = {
  todoNotes?: TodoDesktopBridge;
  todoNotesHost?: TodoHostWindowBridge;
  todoItemId?: string;
};

/** The TODO / pinned-note window's own Desktop bridge (null in a browser). */
export function todoBridge(): TodoDesktopBridge | null {
  return (getDesktopBridge() as TodoBridgeHost | null)?.todoNotes ?? null;
}

/** Plugin-manager / Library windows: open the notes window, Desktop settings. */
export function todoHostBridge(): TodoHostWindowBridge | null {
  const bridge = getDesktopBridge() as TodoBridgeHost | null;
  return bridge?.todoNotesHost ?? null;
}

/** The pinned item of a sticky window (argv, else the hash route). */
export function stickyItemId(): string | null {
  const fromBridge = (getDesktopBridge() as TodoBridgeHost | null)?.todoItemId;
  if (fromBridge) return fromBridge;
  const match = /#\/todo-sticky\/([0-9a-f-]{36})/i.exec(window.location.hash);
  return match ? match[1] : null;
}

/** Open a link through the Desktop host (validated there) or a new tab. */
export async function openLink(url: string): Promise<void> {
  const bridge = todoBridge();
  if (bridge) {
    await bridge.openLink(url);
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

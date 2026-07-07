export interface DesktopDiagnostics {
  ffmpegAvailable: boolean;
  dbPath: string | null;
  pluginsDir: string | null;
}

export interface DesktopWindowControls {
  minimize: () => Promise<void>;
  maximize: () => Promise<boolean>;
  close: () => Promise<void>;
  isMaximized: () => Promise<boolean>;
  beginDrag: (payload: {
    clientX: number;
    screenX: number;
    screenY: number;
    titlebarOffsetY: number;
    viewportWidth: number;
  }) => Promise<{
    x: number;
    y: number;
    width: number;
    height: number;
    restoredFromMaximized?: boolean;
  } | null>;
  moveDrag: () => Promise<boolean>;
  endDrag: () => Promise<boolean>;
  getBounds: () => Promise<{ x: number; y: number; width: number; height: number } | null>;
  setPosition: (x: number, y: number) => Promise<boolean>;
}

export interface DesktopShellActions {
  revealPath: (path: string) => Promise<boolean>;
  openPath: (path: string) => Promise<boolean>;
  copyText: (text: string) => Promise<boolean>;
}

export interface MegleDesktopBridge {
  coreUrl?: string;
  sessionToken?: string;
  windowRole?: string;
  notifyShellReady?: () => Promise<boolean>;
  pickFolder?: () => Promise<string | null>;
  diagnostics?: () => Promise<DesktopDiagnostics>;
  openSettingsWindow?: () => Promise<boolean>;
  closeSettingsWindow?: () => Promise<boolean>;
  windowControls?: DesktopWindowControls;
  shell?: DesktopShellActions;
}

declare global {
  interface Window {
    megleDesktop?: MegleDesktopBridge;
  }
}

export function getDesktopBridge(): MegleDesktopBridge | null {
  return window.megleDesktop ?? null;
}

let desktopShellReadyNotified = false;

export async function notifyDesktopShellReady(): Promise<boolean> {
  const notifyShellReady = getDesktopBridge()?.notifyShellReady;
  if (!notifyShellReady) {
    return false;
  }
  if (desktopShellReadyNotified) {
    return true;
  }
  desktopShellReadyNotified = true;
  try {
    const didNotify = await notifyShellReady();
    if (!didNotify) {
      desktopShellReadyNotified = false;
    }
    return didNotify;
  } catch {
    desktopShellReadyNotified = false;
    return false;
  }
}

export function canPickNativeFolder(): boolean {
  return typeof getDesktopBridge()?.pickFolder === "function";
}

export async function pickNativeFolder(): Promise<string | null> {
  return (await getDesktopBridge()?.pickFolder?.()) ?? null;
}

export function getWindowControls(): DesktopWindowControls | null {
  return getDesktopBridge()?.windowControls ?? null;
}

export function getDesktopShellActions(): DesktopShellActions | null {
  return getDesktopBridge()?.shell ?? null;
}

export async function revealPath(path: string): Promise<boolean> {
  return (await getDesktopShellActions()?.revealPath(path)) ?? false;
}

export async function openPath(path: string): Promise<boolean> {
  return (await getDesktopShellActions()?.openPath(path)) ?? false;
}

export async function copyText(text: string): Promise<boolean> {
  const desktop = getDesktopShellActions();
  if (desktop) {
    return desktop.copyText(text);
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export async function readDesktopDiagnostics(): Promise<DesktopDiagnostics | null> {
  const bridge = getDesktopBridge();
  if (!bridge?.diagnostics) {
    return null;
  }
  try {
    return await bridge.diagnostics();
  } catch {
    return null;
  }
}

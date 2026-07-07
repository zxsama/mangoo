// CommonJS preload script. Electron's sandboxed preload runtime requires
// CJS, while the rest of the desktop module is ESM (because the workspace
// root sets type: module). We ship this hand-maintained .cjs alongside the
// tsc output so the renderer reliably gets the bridge.

const { contextBridge, ipcRenderer } = require("electron");

function readArg(prefix) {
  const arg = process.argv.find((item) => item.startsWith(prefix));
  return arg ? arg.slice(prefix.length) : undefined;
}

const visualHarnessEnabled = readArg("--megle-visual-harness=") === "1";

contextBridge.exposeInMainWorld("megleDesktop", {
  coreUrl: readArg("--megle-core-url="),
  sessionToken: readArg("--megle-session-token="),
  // Which window this renderer is: "settings" for the separate settings window,
  // undefined/"main" for the primary window. The web entry uses this + the
  // #/settings hash to render the standalone settings surface.
  windowRole: readArg("--megle-window=") ?? "main",
  notifyShellReady: () => ipcRenderer.invoke("megle:shell-ready"),
  pickFolder: () => ipcRenderer.invoke("megle:pick-folder"),
  diagnostics: () => ipcRenderer.invoke("megle:diagnostics"),
  openSettingsWindow: () => ipcRenderer.invoke("megle:open-settings-window"),
  closeSettingsWindow: () => ipcRenderer.invoke("megle:close-settings-window"),
  windowControls: {
    minimize: () => ipcRenderer.invoke("megle:window-minimize"),
    maximize: () => ipcRenderer.invoke("megle:window-maximize"),
    close: () => ipcRenderer.invoke("megle:window-close"),
    isMaximized: () => ipcRenderer.invoke("megle:window-is-maximized"),
    beginDrag: (payload) => ipcRenderer.invoke("megle:window-begin-drag", payload),
    moveDrag: () => ipcRenderer.invoke("megle:window-drag-move"),
    endDrag: () => ipcRenderer.invoke("megle:window-end-drag"),
    getBounds: () => ipcRenderer.invoke("megle:window-get-bounds"),
    setPosition: (x, y) => ipcRenderer.invoke("megle:window-set-position", x, y)
  },
  shell: {
    revealPath: (path) => ipcRenderer.invoke("megle:shell-reveal-path", path),
    openPath: (path) => ipcRenderer.invoke("megle:shell-open-path", path),
    copyText: (text) => ipcRenderer.invoke("megle:clipboard-write-text", text)
  },
  ...(visualHarnessEnabled
    ? {
        visual: {
          capturePage: () => ipcRenderer.invoke("megle:visual-capture-page")
        }
      }
    : {})
});

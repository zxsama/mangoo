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
  // The web entry pairs this role with the matching hash route to select the
  // main shell or a standalone secondary-window surface.
  windowRole: readArg("--megle-window=") ?? "main",
  notifyShellReady: () => ipcRenderer.invoke("megle:shell-ready"),
  pickFolder: () => ipcRenderer.invoke("megle:pick-folder"),
  diagnostics: () => ipcRenderer.invoke("megle:diagnostics"),
  openSettingsWindow: () => ipcRenderer.invoke("megle:open-settings-window"),
  closeSettingsWindow: () => ipcRenderer.invoke("megle:close-settings-window"),
  openPluginsWindow: () => ipcRenderer.invoke("megle:open-plugins-window"),
  closePluginsWindow: () => ipcRenderer.invoke("megle:close-plugins-window"),
  notifySecondaryWindowReady: (role) =>
    ipcRenderer.invoke("megle:secondary-window-ready", role),
  onSecondaryWindowMaterialVisible: (cb) => {
    const listener = (_event, payload) => cb(payload);
    ipcRenderer.on("megle:secondary-window-material-visible", listener);
    return () => ipcRenderer.removeListener("megle:secondary-window-material-visible", listener);
  },
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
  listLibraries: () => ipcRenderer.invoke("megle:list-libraries"),
  getActiveLibraryId: () => ipcRenderer.invoke("megle:get-active-library-id"),
  switchLibrary: (libraryId) => ipcRenderer.invoke("megle:switch-library", libraryId),
  createLibrary: (input) => ipcRenderer.invoke("megle:create-library", input),
  openLibrary: (dbPath) => ipcRenderer.invoke("megle:open-library", dbPath),
  removeLibraryFromList: (libraryId) => ipcRenderer.invoke("megle:remove-library-from-list", libraryId),
  renameLibrary: (libraryId, name) => ipcRenderer.invoke("megle:rename-library", libraryId, name),
  setLibraryIcon: (libraryId, icon, color) => ipcRenderer.invoke("megle:set-library-icon", libraryId, icon, color),
  deleteLibraryFile: (libraryId) => ipcRenderer.invoke("megle:delete-library-file", libraryId),
  pickLibraryFile: () => ipcRenderer.invoke("megle:pick-library-file"),
  pickNewLibraryPath: (defaultName) => ipcRenderer.invoke("megle:pick-new-library-path", defaultName),
  onLibraryPhase: (cb) => {
    const listener = (_event, phase) => cb(phase);
    ipcRenderer.on("megle:library-load-phase", listener);
    return () => ipcRenderer.removeListener("megle:library-load-phase", listener);
  },
  ...(visualHarnessEnabled
    ? {
        visual: {
          capturePage: () => ipcRenderer.invoke("megle:visual-capture-page")
        }
      }
    : {})
});

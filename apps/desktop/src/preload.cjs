// CommonJS preload script. Electron's sandboxed preload runtime requires
// CJS, while the rest of the desktop module is ESM (because the workspace
// root sets type: module). We ship this hand-maintained .cjs alongside the
// tsc output so the renderer reliably gets the bridge.

const { contextBridge, ipcRenderer } = require("electron");

function readArg(prefix) {
  const arg = process.argv.find((item) => item.startsWith(prefix));
  return arg ? arg.slice(prefix.length) : undefined;
}

function subscribe(channel, cb) {
  const listener = (_event, value) => cb(value);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

async function snapshotAndSubscribeLogs(query, cb) {
  const queued = [];
  let ready = false;
  let active = true;
  const listener = (_event, batch) => {
    if (!active) return;
    if (!ready) {
      queued.push(batch);
      return;
    }
    cb(batch);
  };
  ipcRenderer.on(NOTIFICATION_IPC_CHANNELS.logsChanged, listener);
  try {
    const initial = await ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsSubscribe, query);
    ready = true;
    for (const batch of queued.splice(0)) cb(batch);
    return {
      ...initial,
      unsubscribe: () => {
        if (!active) return;
        active = false;
        ipcRenderer.removeListener(NOTIFICATION_IPC_CHANNELS.logsChanged, listener);
        ipcRenderer.send(NOTIFICATION_IPC_CHANNELS.logsUnsubscribe);
      }
    };
  } catch (error) {
    active = false;
    ipcRenderer.removeListener(NOTIFICATION_IPC_CHANNELS.logsChanged, listener);
    ipcRenderer.send(NOTIFICATION_IPC_CHANNELS.logsUnsubscribe);
    throw error;
  }
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
  openNotificationsWindow: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.openWindow),
  closeNotificationsWindow: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.closeWindow),
  notifySecondaryWindowReady: (role) =>
    ipcRenderer.invoke("megle:secondary-window-ready", role),
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
  notifications: {
    getCompactSnapshot: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.compact),
    listHistory: (query) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.history, query),
    cancel: (libraryId, operationId) =>
      ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.cancel, libraryId, operationId),
    retry: (libraryId, operationId) =>
      ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.retry, libraryId, operationId),
    reportError: (input) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.reportError, input),
    publishThumbnailSummary: (input) =>
      ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.thumbnailSummary, input),
    onChanged: (cb) => subscribe(NOTIFICATION_IPC_CHANNELS.changed, cb),
    getLogs: (query) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsPage, query),
    onLogs: (cb) => subscribe(NOTIFICATION_IPC_CHANNELS.logsChanged, cb),
    snapshotAndSubscribeLogs,
    clearLogs: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsClear),
    emitRendererLogs: (entries) => ipcRenderer.send(NOTIFICATION_IPC_CHANNELS.rendererLogs, entries)
  },
  ...(visualHarnessEnabled
    ? {
        visual: {
          capturePage: () => ipcRenderer.invoke("megle:visual-capture-page")
        }
      }
    : {})
});

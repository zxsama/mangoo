// CommonJS preload script. Electron's sandboxed preload runtime requires
// CJS, while the rest of the desktop module is ESM (because the workspace
// root sets type: module). We ship this hand-maintained .cjs alongside the
// tsc output so the renderer reliably gets the bridge.

const { contextBridge, ipcRenderer, webUtils } = require("electron");

function readArg(prefix) {
  const arg = process.argv.find((item) => item.startsWith(prefix));
  return arg ? arg.slice(prefix.length) : undefined;
}

function subscribe(channel, cb) {
  const listener = (_event, value) => cb(value);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

let pendingNotificationWindowIntent = null;
const notificationWindowIntentSubscribers = new Set();
ipcRenderer.on(NOTIFICATION_IPC_CHANNELS.windowIntent, (_event, intent) => {
  if (notificationWindowIntentSubscribers.size === 0) {
    pendingNotificationWindowIntent = intent;
    return;
  }
  pendingNotificationWindowIntent = null;
  for (const subscriber of notificationWindowIntentSubscribers) subscriber(intent);
});

function subscribeNotificationWindowIntent(cb) {
  notificationWindowIntentSubscribers.add(cb);
  if (pendingNotificationWindowIntent !== null) {
    const intent = pendingNotificationWindowIntent;
    pendingNotificationWindowIntent = null;
    cb(intent);
  }
  return () => notificationWindowIntentSubscribers.delete(cb);
}

let pendingPluginViewIntent = null;
const pluginViewIntentSubscribers = new Set();
ipcRenderer.on("mangoo:plugin-view-intent", (_event, intent) => {
  if (pluginViewIntentSubscribers.size === 0) { pendingPluginViewIntent = intent; return; }
  pendingPluginViewIntent = null;
  for (const subscriber of pluginViewIntentSubscribers) subscriber(intent);
});
function subscribePluginViewIntent(cb) {
  pluginViewIntentSubscribers.add(cb);
  if (pendingPluginViewIntent !== null) {
    const intent = pendingPluginViewIntent;
    pendingPluginViewIntent = null;
    cb(intent);
  }
  return () => pluginViewIntentSubscribers.delete(cb);
}

const MAX_QUEUED_LOG_LINES = 5000;

function mergeQueuedLogBatch(previous, next) {
  const lines = [...(previous?.lines ?? []), ...(next?.lines ?? [])];
  let droppedBeforeSequence = Math.max(
    previous?.droppedBeforeSequence ?? 0,
    next?.droppedBeforeSequence ?? 0
  ) || null;
  if (lines.length > MAX_QUEUED_LOG_LINES) {
    lines.splice(0, lines.length - MAX_QUEUED_LOG_LINES);
    droppedBeforeSequence = lines[0]?.sequence ?? next.afterSequence;
  }
  return {
    lines,
    afterSequence: next.afterSequence,
    droppedBeforeSequence
  };
}

function snapshotAndSubscribeLogs(query, cb) {
  const queuedBySubscription = new Map();
  let active = true;
  let subscriptionId = null;
  let unsubscribeSent = false;
  const sendUnsubscribe = (id) => {
    if (unsubscribeSent) return;
    unsubscribeSent = true;
    ipcRenderer.send(NOTIFICATION_IPC_CHANNELS.logsUnsubscribe, id);
  };
  const listener = (_event, payload) => {
    if (!active) return;
    if (!payload || typeof payload.subscriptionId !== "string") return;
    if (subscriptionId === null) {
      queuedBySubscription.set(
        payload.subscriptionId,
        mergeQueuedLogBatch(queuedBySubscription.get(payload.subscriptionId), payload.batch)
      );
      if (queuedBySubscription.size > 8) {
        queuedBySubscription.delete(queuedBySubscription.keys().next().value);
      }
      return;
    }
    if (payload.subscriptionId === subscriptionId) cb(payload.batch);
  };
  ipcRenderer.on(NOTIFICATION_IPC_CHANNELS.logsChanged, listener);
  const ready = ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsSubscribe, query).then(
    (initial) => {
      subscriptionId = initial.subscriptionId;
      if (!active) {
        sendUnsubscribe(subscriptionId);
      } else if (queuedBySubscription.has(subscriptionId)) {
        cb(queuedBySubscription.get(subscriptionId));
      }
      queuedBySubscription.clear();
      return { snapshot: initial.snapshot, afterSequence: initial.afterSequence };
    },
    (error) => {
      active = false;
      queuedBySubscription.clear();
      ipcRenderer.removeListener(NOTIFICATION_IPC_CHANNELS.logsChanged, listener);
      throw error;
    }
  );
  return {
    ready,
    unsubscribe: () => {
      if (!active) return;
      active = false;
      queuedBySubscription.clear();
      ipcRenderer.removeListener(NOTIFICATION_IPC_CHANNELS.logsChanged, listener);
      if (subscriptionId !== null) sendUnsubscribe(subscriptionId);
    }
  };
}

const visualHarnessEnabled = readArg("--mangoo-visual-harness=") === "1";
const requestedWindowRole = readArg("--mangoo-window=");
const windowRole = ["main", "settings", "plugins", "notifications", "image-viewer"].includes(requestedWindowRole)
  ? requestedWindowRole
  : "unknown";
const windowControls = {
  minimize: () => ipcRenderer.invoke("mangoo:window-minimize"),
  maximize: () => ipcRenderer.invoke("mangoo:window-maximize"),
  close: () => ipcRenderer.invoke("mangoo:window-close"),
  isMaximized: () => ipcRenderer.invoke("mangoo:window-is-maximized"),
  beginDrag: (payload) => ipcRenderer.invoke("mangoo:window-begin-drag", payload),
  moveDrag: () => ipcRenderer.invoke("mangoo:window-drag-move"),
  endDrag: () => ipcRenderer.invoke("mangoo:window-end-drag"),
  getBounds: () => ipcRenderer.invoke("mangoo:window-get-bounds"),
  setPosition: (x, y) => ipcRenderer.invoke("mangoo:window-set-position", x, y)
};
/** @type {import("@mangoo/notification-contract").NotificationIpcBridge} */
const notificationsBridge = {
  getCompactSnapshot: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.compact),
  getHistoryFacets: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.historyFacets),
  listHistory: (query) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.history, query),
  cancel: (libraryId, operationId) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.cancel, libraryId, operationId),
  retry: (libraryId, operationId) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.retry, libraryId, operationId),
  dismissManagedRoot: (input) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.dismissManagedRoot, input),
  reviewManagedRoot: (input) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.reviewManagedRoot, input),
  reportError: (input) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.reportError, input),
  publishThumbnailSummary: (input) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.thumbnailSummary, input),
  onChanged: (cb) => subscribe(NOTIFICATION_IPC_CHANNELS.changed, cb),
  getLogs: (query) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsPage, query),
  exportLogs: (query) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsExport, query),
  snapshotAndSubscribeLogs,
  clearLogs: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsClear),
  emitRendererLogs: (entries) => ipcRenderer.send(NOTIFICATION_IPC_CHANNELS.rendererLogs, entries)
};
const commonWindowBridge = {
  windowRole,
  notifySecondaryWindowReady: (role) =>
    ipcRenderer.invoke("mangoo:secondary-window-ready", role),
  windowControls
};
// Built-in image viewer plugin (com.mangoo.image-viewer). Channel names mirror
// bundled-plugins/com.mangoo.image-viewer/src/shared/ipc.ts (guarded by a test).
const imageViewerHostBridge = {
  associationStatus: () => ipcRenderer.invoke("mangoo:image-viewer:association-status"),
  setAssociation: (enabled) => ipcRenderer.invoke("mangoo:image-viewer:set-association", enabled === true),
  openDefaultAppsSettings: () => ipcRenderer.invoke("mangoo:image-viewer:open-default-apps-settings"),
  openFiles: (paths) => ipcRenderer.invoke("mangoo:image-viewer:open-files", paths)
};
const imageViewerBridge = {
  ...imageViewerHostBridge,
  ready: () => ipcRenderer.invoke("mangoo:image-viewer:ready"),
  onOpenRequest: (cb) => subscribe("mangoo:image-viewer:open-request", cb),
  pickFiles: () => ipcRenderer.invoke("mangoo:image-viewer:pick-files"),
  pickFolder: () => ipcRenderer.invoke("mangoo:image-viewer:pick-folder"),
  admitPaths: (paths) => ipcRenderer.invoke("mangoo:image-viewer:admit-paths", paths),
  pathForFile: (file) => {
    try {
      return webUtils.getPathForFile(file) || null;
    } catch {
      return null;
    }
  },
  trash: (path) => ipcRenderer.invoke("mangoo:image-viewer:trash", path),
  rename: (path, newName) => ipcRenderer.invoke("mangoo:image-viewer:rename", path, newName),
  reveal: (path) => ipcRenderer.invoke("mangoo:image-viewer:reveal", path),
  openExternally: (path) => ipcRenderer.invoke("mangoo:image-viewer:open-externally", path),
  copyImage: (png) => ipcRenderer.invoke("mangoo:image-viewer:copy-image", png),
  copyText: (text) => ipcRenderer.invoke("mangoo:image-viewer:copy-text", text),
  toggleFullscreen: () => ipcRenderer.invoke("mangoo:image-viewer:toggle-fullscreen"),
  onFullscreenChanged: (cb) => subscribe("mangoo:image-viewer:fullscreen-changed", cb),
  setAlwaysOnTop: (value) => ipcRenderer.invoke("mangoo:image-viewer:set-always-on-top", value === true),
  openMainWindow: () => ipcRenderer.invoke("mangoo:image-viewer:open-main-window")
};
const imageViewerWindowBridge = {
  ...commonWindowBridge,
  coreUrl: readArg("--mangoo-core-url="),
  sessionToken: readArg("--mangoo-session-token="),
  imageViewer: imageViewerBridge
};
const fullBridge = {
  ...commonWindowBridge,
  notifyPluginsChanged: () => ipcRenderer.invoke("mangoo:plugins-changed"),
  imageViewerHost: imageViewerHostBridge,
  coreUrl: readArg("--mangoo-core-url="),
  sessionToken: readArg("--mangoo-session-token="),
  notifyShellReady: () => ipcRenderer.invoke("mangoo:shell-ready"),
  pickFolder: (request) => ipcRenderer.invoke("mangoo:pick-folder", request),
  diagnostics: () => ipcRenderer.invoke("mangoo:diagnostics"),
  openSettingsWindow: () => ipcRenderer.invoke("mangoo:open-settings-window"),
  closeSettingsWindow: () => ipcRenderer.invoke("mangoo:close-settings-window"),
  openPluginsWindow: () => ipcRenderer.invoke("mangoo:open-plugins-window"),
  ...(windowRole === "plugins" ? { onPluginViewIntent: subscribePluginViewIntent } : {}),
  closePluginsWindow: () => ipcRenderer.invoke("mangoo:close-plugins-window"),
  openNotificationsWindow: (intent) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.openWindow, intent),
  closeNotificationsWindow: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.closeWindow),
  shell: {
    revealPath: (path) => ipcRenderer.invoke("mangoo:shell-reveal-path", path),
    openPath: (path) => ipcRenderer.invoke("mangoo:shell-open-path", path),
    openPluginExternalLink: (request) =>
      ipcRenderer.invoke("mangoo:open-plugin-external-link", request),
    requestOAuthReturnFocus: () =>
      ipcRenderer.invoke("mangoo:request-oauth-return-focus"),
    copyText: (text) => ipcRenderer.invoke("mangoo:clipboard-write-text", text)
  },
  getLibraryPhase: () => ipcRenderer.invoke("mangoo:get-library-load-phase"),
  retryLibraryLoad: () => ipcRenderer.invoke("mangoo:retry-library-load"),
  pickNewLibraryPath: (defaultName) => ipcRenderer.invoke("mangoo:pick-new-library-path", defaultName),
  resolveLibraryCreationPaths: (input) => ipcRenderer.invoke("mangoo:resolve-library-creation-paths", input),
  onLibraryPhase: (cb) => {
    const listener = (_event, phase) => cb(phase);
    ipcRenderer.on("mangoo:library-load-phase", listener);
    return () => ipcRenderer.removeListener("mangoo:library-load-phase", listener);
  },
  onFocusMedia: (cb) => subscribe(NOTIFICATION_IPC_CHANNELS.focusMediaRequested, cb),
  onFocusMediaCancelled: (cb) => subscribe(NOTIFICATION_IPC_CHANNELS.focusMediaCancelled, cb),
  completeMediaFocus: (completion) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.focusMediaCompleted, completion),
  updateMediaFocusStage: (update) =>
    ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.focusMediaStage, update),
  notifications: notificationsBridge
};
const notificationWindowBridge = {
  ...commonWindowBridge,
  listNotificationPluginEntries: () => ipcRenderer.invoke("mangoo:notification-plugin-entries"),
  openNotificationPluginView: (request) => ipcRenderer.invoke("mangoo:open-notification-plugin-view", request),
  closeNotificationsWindow: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.closeWindow),
  onNotificationWindowIntent: subscribeNotificationWindowIntent,
  focusMedia: (input) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.focusMedia, input),
  notifications: notificationsBridge
};
const roleBridge = windowRole === "notifications"
  ? notificationWindowBridge
  : windowRole === "image-viewer"
    ? imageViewerWindowBridge
  : ["main", "settings", "plugins"].includes(windowRole)
    ? fullBridge
    : { windowRole };
contextBridge.exposeInMainWorld("mangooDesktop", {
  ...roleBridge,
  ...(visualHarnessEnabled
    ? { visual: { capturePage: () => ipcRenderer.invoke("mangoo:visual-capture-page") } }
    : {})
});

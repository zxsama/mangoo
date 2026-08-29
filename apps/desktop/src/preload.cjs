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

const visualHarnessEnabled = readArg("--megle-visual-harness=") === "1";
const requestedWindowRole = readArg("--megle-window=");
const windowRole = ["main", "settings", "plugins", "notifications"].includes(requestedWindowRole)
  ? requestedWindowRole
  : "unknown";
const windowControls = {
  minimize: () => ipcRenderer.invoke("megle:window-minimize"),
  maximize: () => ipcRenderer.invoke("megle:window-maximize"),
  close: () => ipcRenderer.invoke("megle:window-close"),
  isMaximized: () => ipcRenderer.invoke("megle:window-is-maximized"),
  beginDrag: (payload) => ipcRenderer.invoke("megle:window-begin-drag", payload),
  moveDrag: () => ipcRenderer.invoke("megle:window-drag-move"),
  endDrag: () => ipcRenderer.invoke("megle:window-end-drag"),
  getBounds: () => ipcRenderer.invoke("megle:window-get-bounds"),
  setPosition: (x, y) => ipcRenderer.invoke("megle:window-set-position", x, y)
};
/** @type {import("@megle/notification-contract").NotificationIpcBridge} */
const notificationsBridge = {
  getCompactSnapshot: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.compact),
  getHistoryFacets: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.historyFacets),
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
  exportLogs: (query) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsExport, query),
  snapshotAndSubscribeLogs,
  clearLogs: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.logsClear),
  emitRendererLogs: (entries) => ipcRenderer.send(NOTIFICATION_IPC_CHANNELS.rendererLogs, entries)
};
const commonWindowBridge = {
  windowRole,
  notifySecondaryWindowReady: (role) =>
    ipcRenderer.invoke("megle:secondary-window-ready", role),
  windowControls
};
const fullBridge = {
  ...commonWindowBridge,
  coreUrl: readArg("--megle-core-url="),
  sessionToken: readArg("--megle-session-token="),
  notifyShellReady: () => ipcRenderer.invoke("megle:shell-ready"),
  pickFolder: () => ipcRenderer.invoke("megle:pick-folder"),
  diagnostics: () => ipcRenderer.invoke("megle:diagnostics"),
  openSettingsWindow: () => ipcRenderer.invoke("megle:open-settings-window"),
  closeSettingsWindow: () => ipcRenderer.invoke("megle:close-settings-window"),
  openPluginsWindow: () => ipcRenderer.invoke("megle:open-plugins-window"),
  closePluginsWindow: () => ipcRenderer.invoke("megle:close-plugins-window"),
  openNotificationsWindow: (intent) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.openWindow, intent),
  closeNotificationsWindow: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.closeWindow),
  shell: {
    revealPath: (path) => ipcRenderer.invoke("megle:shell-reveal-path", path),
    openPath: (path) => ipcRenderer.invoke("megle:shell-open-path", path),
    openExternalAuthorizationUrl: (url) =>
      ipcRenderer.invoke("megle:open-external-authorization-url", url),
    openEntraAppRegistrationPage: () =>
      ipcRenderer.invoke("megle:open-entra-app-registration-page"),
    requestOAuthReturnFocus: () =>
      ipcRenderer.invoke("megle:request-oauth-return-focus"),
    copyText: (text) => ipcRenderer.invoke("megle:clipboard-write-text", text)
  },
  getLibraryPhase: () => ipcRenderer.invoke("megle:get-library-load-phase"),
  retryLibraryLoad: () => ipcRenderer.invoke("megle:retry-library-load"),
  pickNewLibraryPath: (defaultName) => ipcRenderer.invoke("megle:pick-new-library-path", defaultName),
  onLibraryPhase: (cb) => {
    const listener = (_event, phase) => cb(phase);
    ipcRenderer.on("megle:library-load-phase", listener);
    return () => ipcRenderer.removeListener("megle:library-load-phase", listener);
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
  closeNotificationsWindow: () => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.closeWindow),
  onNotificationWindowIntent: subscribeNotificationWindowIntent,
  focusMedia: (input) => ipcRenderer.invoke(NOTIFICATION_IPC_CHANNELS.focusMedia, input),
  notifications: notificationsBridge
};
const roleBridge = windowRole === "notifications"
  ? notificationWindowBridge
  : ["main", "settings", "plugins"].includes(windowRole)
    ? fullBridge
    : { windowRole };
contextBridge.exposeInMainWorld("megleDesktop", {
  ...roleBridge,
  ...(visualHarnessEnabled
    ? { visual: { capturePage: () => ipcRenderer.invoke("megle:visual-capture-page") } }
    : {})
});

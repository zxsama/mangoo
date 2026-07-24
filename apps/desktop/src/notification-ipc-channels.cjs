"use strict";

const NOTIFICATION_IPC_CHANNELS = Object.freeze({
  openWindow: "megle:open-notifications-window",
  windowIntent: "megle:notification-window-intent",
  closeWindow: "megle:close-notifications-window",
  focusMedia: "megle:notification-focus-media",
  focusMediaRequested: "megle:notification-focus-media-requested",
  compact: "megle:notifications-compact",
  historyFacets: "megle:notifications-history-facets",
  history: "megle:notifications-history",
  cancel: "megle:notifications-cancel",
  retry: "megle:notifications-retry",
  reportError: "megle:notifications-report-error",
  thumbnailSummary: "megle:notifications-thumbnail-summary",
  changed: "megle:notifications-changed",
  logsPage: "megle:process-logs",
  logsExport: "megle:process-logs-export",
  logsSubscribe: "megle:process-logs-subscribe",
  logsUnsubscribe: "megle:process-logs-unsubscribe",
  logsClear: "megle:process-logs-clear",
  logsChanged: "megle:process-logs-changed",
  rendererLogs: "megle:renderer-logs"
});

if (typeof module === "object" && module) {
  module.exports = NOTIFICATION_IPC_CHANNELS;
}

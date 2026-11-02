"use strict";

const NOTIFICATION_IPC_CHANNELS = Object.freeze({
  openWindow: "mangoo:open-notifications-window",
  windowIntent: "mangoo:notification-window-intent",
  closeWindow: "mangoo:close-notifications-window",
  focusMedia: "mangoo:notification-focus-media",
  focusMediaRequested: "mangoo:notification-focus-media-requested",
  focusMediaCancelled: "mangoo:notification-focus-media-cancelled",
  focusMediaCompleted: "mangoo:notification-focus-media-completed",
  focusMediaStage: "mangoo:notification-focus-media-stage",
  compact: "mangoo:notifications-compact",
  historyFacets: "mangoo:notifications-history-facets",
  history: "mangoo:notifications-history",
  cancel: "mangoo:notifications-cancel",
  retry: "mangoo:notifications-retry",
  dismissManagedRoot: "mangoo:notifications-dismiss-managed-root",
  reviewManagedRoot: "mangoo:notifications-review-managed-root",
  reportError: "mangoo:notifications-report-error",
  thumbnailSummary: "mangoo:notifications-thumbnail-summary",
  changed: "mangoo:notifications-changed",
  logsPage: "mangoo:process-logs",
  logsExport: "mangoo:process-logs-export",
  logsSubscribe: "mangoo:process-logs-subscribe",
  logsUnsubscribe: "mangoo:process-logs-unsubscribe",
  logsClear: "mangoo:process-logs-clear",
  logsChanged: "mangoo:process-logs-changed",
  rendererLogs: "mangoo:renderer-logs"
});

if (typeof module === "object" && module) {
  module.exports = NOTIFICATION_IPC_CHANNELS;
}

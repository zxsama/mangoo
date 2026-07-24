import type { NotificationOperationSnapshot } from "@megle/core-client/generated-contract";
import type {
  DesktopNotificationOperation,
  NotificationIpcBridge,
  MediaFocusRequest,
  NotificationWindowIntent,
  ProcessLogLine,
  RendererErrorReport,
  ProcessLogSubscriptionEvent,
  ProcessLogSubscriptionStart,
  ThumbnailFailureDetail
} from "../src/index.js";
import {
  legacyRendererErrorOperationId,
  rendererErrorIdentityKey,
  rendererErrorOperationId
} from "../src/index.js";

declare const operation: DesktopNotificationOperation;
const coreOperation: NotificationOperationSnapshot = operation;
declare const logLine: ProcessLogLine;
const traceLevel: "trace" | "debug" | "info" | "warn" | "error" = logLine.level;
declare const start: ProcessLogSubscriptionStart;
declare const event: ProcessLogSubscriptionEvent;
declare const bridge: NotificationIpcBridge;
declare const rendererReport: RendererErrorReport;
declare const intent: NotificationWindowIntent;
declare const focus: MediaFocusRequest;
declare const failure: ThumbnailFailureDetail;

void coreOperation;
void traceLevel;
void start.subscriptionId;
void event.subscriptionId;
void bridge.snapshotAndSubscribeLogs;
void bridge.getHistoryFacets;
void bridge.exportLogs;
void rendererReport.reportId;
void intent.logFilter.eventKind;
void focus.fileId;
void failure.relativePath;
void rendererErrorIdentityKey(rendererReport);
void rendererErrorOperationId(rendererReport.libraryId, rendererReport);
void legacyRendererErrorOperationId(rendererReport.libraryId, rendererReport);

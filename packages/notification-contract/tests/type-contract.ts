import type { NotificationOperationSnapshot } from "@megle/core-client/generated-contract";
import type {
  DesktopNotificationOperation,
  NotificationIpcBridge,
  ProcessLogLine,
  RendererErrorReport,
  ProcessLogSubscriptionEvent,
  ProcessLogSubscriptionStart
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

void coreOperation;
void traceLevel;
void start.subscriptionId;
void event.subscriptionId;
void bridge.snapshotAndSubscribeLogs;
void bridge.getHistoryFacets;
void bridge.exportLogs;
void rendererReport.reportId;
void rendererErrorIdentityKey(rendererReport);
void rendererErrorOperationId(rendererReport.libraryId, rendererReport);
void legacyRendererErrorOperationId(rendererReport.libraryId, rendererReport);

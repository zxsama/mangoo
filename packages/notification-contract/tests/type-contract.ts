import type { NotificationOperationSnapshot } from "@megle/core-client/generated-contract";
import type {
  DesktopNotificationOperation,
  NotificationIpcBridge,
  ProcessLogLine,
  ProcessLogSubscriptionEvent,
  ProcessLogSubscriptionStart
} from "../src/index.js";

declare const operation: DesktopNotificationOperation;
const coreOperation: NotificationOperationSnapshot = operation;
declare const logLine: ProcessLogLine;
const traceLevel: "trace" | "debug" | "info" | "warn" | "error" = logLine.level;
declare const start: ProcessLogSubscriptionStart;
declare const event: ProcessLogSubscriptionEvent;
declare const bridge: NotificationIpcBridge;

void coreOperation;
void traceLevel;
void start.subscriptionId;
void event.subscriptionId;
void bridge.snapshotAndSubscribeLogs;
void bridge.listLibraries;
void bridge.exportLogs;

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

test("legacy root review keeps target validation but opens the generic plugin manager", () => {
  const source = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const handler = source.slice(source.indexOf("ipcMain.handle(NOTIFICATION_IPC_CHANNELS.reviewManagedRoot,"), source.indexOf("ipcMain.handle(NOTIFICATION_IPC_CHANNELS.reportError,"));
  assert.ok(handler.includes("requireNotificationIpcAccess"));
  assert.ok(handler.includes("validateManagedRootTarget"));
  assert.ok(handler.includes("if (!validation.ok) return validation"));
  assert.ok(handler.includes("if (!rootResponse.ok)"));
  assert.ok(handler.includes('openSecondaryWindow("plugins")'));
  assert.ok(!handler.includes('openSecondaryWindow("settings")'));
  assert.ok(!handler.includes('page: "sync-folders"'));
});

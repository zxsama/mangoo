import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const source = readFileSync(path.join(process.cwd(), "src", "index.ts"), "utf8");

if (!/import type \{[\s\S]*NotificationOperationSnapshot[\s\S]*\} from "@mangoo\/core-client\/generated-contract"/.test(source)) {
  throw new Error("notification contract must reuse the generated Core notification operation type");
}
const generated = readFileSync(path.join(process.cwd(), "..", "core-client", "src", "generated-contract.ts"), "utf8");
if (!generated.includes("libraryUuid?: string | null;") || !generated.includes("rootUuid?: string | null;")) {
  throw new Error("notification operation snapshots must expose optional nullable stable Library/root UUID context");
}
if (/from "node:|require\(["']node:/.test(source)) {
  throw new Error("notification contract must not introduce runtime Node dependencies");
}
if (!source.includes('"trace" | "debug" | "info" | "warn" | "error"')) {
  throw new Error("notification contract process logs must retain the trace level");
}
if (source.includes("onLogs")) {
  throw new Error("notification contract must expose only the tokenized log subscription API");
}
if (source.includes("NotificationRetryableFailureReference") || source.includes("retryableFailures")) {
  throw new Error("renderer notification contract must not expose internal failure references");
}
if (!/interface NotificationCompactSnapshot[\s\S]*hasRetryableFailure: boolean;[\s\S]*syncHealth: NotificationSyncHealth;/.test(source)) {
  throw new Error("notification compact snapshot must expose one authoritative failure boolean");
}
if (!/interface RendererErrorReport[\s\S]*reportId\?: string;/.test(source)) {
  throw new Error("renderer error reports must support a stable logical occurrence id");
}
if (
  !source.includes("rendererErrorIdentityKey") ||
  !source.includes("rendererErrorOperationId") ||
  !source.includes("legacyRendererErrorOperationId")
) {
  throw new Error("renderer error identity and legacy migration helpers must stay shared");
}

console.log("PASS: shared notification contract boundary");

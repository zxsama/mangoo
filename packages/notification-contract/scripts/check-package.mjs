import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const source = readFileSync(path.join(process.cwd(), "src", "index.ts"), "utf8");

if (!/import type \{[\s\S]*NotificationOperationSnapshot[\s\S]*\} from "@megle\/core-client\/generated-contract"/.test(source)) {
  throw new Error("notification contract must reuse the generated Core notification operation type");
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

console.log("PASS: shared notification contract boundary");

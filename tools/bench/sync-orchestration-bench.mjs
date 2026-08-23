import { spawnSync } from "node:child_process";

// The orchestration benchmark deliberately executes the production
// SyncCoordinator path (provider transport, Host due selection, active-first
// admission, independent apply/media semaphores and real Catalog writers).
const result = spawnSync(
  "cargo",
  ["test", "-p", "megle-core", "--release", "sync::coordinator::tests::sync_protocol_acceptance_probe", "--", "--ignored", "--nocapture"],
  { cwd: process.cwd(), encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 60 * 60 * 1000 },
);
process.stdout.write(result.stdout ?? "");
process.stderr.write(result.stderr ?? "");
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

const match = result.stdout.match(/SYNC_BENCH_JSON=(\{[^\n]+\})/);
if (!match) throw new Error("production sync coordinator did not emit benchmark metrics");
const metrics = JSON.parse(match[1]);
for (const field of ["libraries", "applyP95Ms", "applyP99Ms", "idleCpuPercent", "dormantPrivateMiB", "browseP95Ms", "searchP95Ms", "thumbP95Ms"]) {
  if (!Number.isFinite(metrics[field])) throw new Error(`missing production metric: ${field}`);
}
if (metrics.libraries < 2) throw new Error("production scheduler benchmark needs multiple Libraries");
if (metrics.providerCalls !== 0) throw new Error(`browse/search/thumbnail issued ${metrics.providerCalls} Provider calls`);
console.log(`PASS: production sync orchestration benchmark ${JSON.stringify(metrics)}`);

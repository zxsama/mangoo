import { spawnSync } from "node:child_process";

const result = spawnSync(
  "cargo",
  [
    "test",
    "-p",
    "megle-core",
    "--release",
    "sync::coordinator::tests::sync_protocol_acceptance_probe",
    "--",
    "--ignored",
    "--nocapture",
  ],
  {
    cwd: process.cwd(),
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    // A cold Windows machine needs ~30-40 minutes to populate and rebuild all
    // production secondary indexes for the exact million-asset fixture.
    timeout: 60 * 60 * 1000,
  },
);
process.stdout.write(result.stdout ?? "");
process.stderr.write(result.stderr ?? "");
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

const match = result.stdout.match(/SYNC_BENCH_JSON=(\{[^\n]+\})/);
if (!match) throw new Error("sync benchmark did not emit measured results");
const metrics = JSON.parse(match[1]);
const required = [
  "applyP95Ms", "applyP99Ms", "maxAppliedEvents", "maxDecodedBytes",
  "assets", "localCaptureP95Ms", "localCaptureP99Ms", "coordinatorCpuMs",
  "idleCpuPercent", "dormantPrivateMiB", "browseP95Ms", "searchP95Ms",
  "thumbP95Ms", "providerCalls", "reopenEvents", "reopenFrontier",
];
for (const name of required) {
  if (!Number.isFinite(metrics[name])) throw new Error(`missing measured metric: ${name}`);
}

const thresholds = {
  libraries: 8,
  applyP95Ms: 10,
  applyP99Ms: 25,
  assets: Number.parseInt(process.env.MEGLE_SYNC_BENCH_ASSETS ?? "1000000", 10),
  localCaptureP95Ms: 5,
  localCaptureP99Ms: 15,
  maxAppliedEvents: 100,
  maxDecodedBytes: 1024 * 1024,
  idleCpuPercent: 1,
  dormantPrivateMiB: 100,
  browseP95Ms: 50,
  searchP95Ms: 150,
  thumbP95Ms: 50,
};
const violations = [];
if (metrics.libraries !== thresholds.libraries) violations.push(`libraries=${metrics.libraries}`);
if (metrics.assets !== thresholds.assets) violations.push(`assets=${metrics.assets}`);
for (const name of ["applyP95Ms", "applyP99Ms", "maxAppliedEvents", "maxDecodedBytes",
  "localCaptureP95Ms", "localCaptureP99Ms", "idleCpuPercent", "dormantPrivateMiB", "browseP95Ms", "searchP95Ms", "thumbP95Ms"]) {
  if (metrics[name] > thresholds[name]) violations.push(`${name}=${metrics[name]} > ${thresholds[name]}`);
}
if (metrics.providerCalls !== 0) violations.push(`providerCalls=${metrics.providerCalls}`);
if (metrics.reopenEvents !== 800 || metrics.reopenFrontier !== 600) {
  violations.push(`reopen=${metrics.reopenEvents}/${metrics.reopenFrontier}`);
}
if (violations.length) throw new Error(`sync benchmark threshold failure: ${violations.join(", ")}`);

console.log(`PASS: measured sync protocol benchmark ${JSON.stringify({ metrics, thresholds })}`);

const kill = spawnSync(
  "cargo",
  ["test", "-p", "megle-core", "--release", "sync::coordinator::tests::sync_protocol_abrupt_kill_replay_probe", "--", "--ignored", "--nocapture"],
  { cwd: process.cwd(), encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 30 * 60 * 1000 },
);
process.stdout.write(kill.stdout ?? "");
process.stderr.write(kill.stderr ?? "");
if (kill.error) throw kill.error;
if (kill.status !== 0) process.exit(kill.status ?? 1);
const killMatch = kill.stdout.match(/SYNC_KILL_JSON=(\{[^\n]+\})/);
if (!killMatch) throw new Error("sync kill probe did not emit measured results");
const killMetrics = JSON.parse(killMatch[1]);
if (killMetrics.abruptKill !== 1 || killMetrics.killEvents !== 100000 || killMetrics.killFrontier !== 100000) {
  throw new Error(`sync abrupt kill invariant failure: ${killMatch[1]}`);
}
console.log(`PASS: abrupt process-kill replay ${killMatch[1]}`);

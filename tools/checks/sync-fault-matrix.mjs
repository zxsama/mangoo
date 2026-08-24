import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { createRunDirectory, environmentFacts, requiredFailures, root, runCommand, writeJson } from "./sync-release-lib.mjs";

function parseArgs(argv) {
  const options = { output: null };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "--output" && argv[index + 1]) options.output = argv[++index];
    else throw new Error(`unknown or incomplete argument: ${value}`);
  }
  return options;
}

const options = parseArgs(process.argv.slice(2));
const runDirectory = await createRunDirectory("fault-matrix", options.output);
const manifestPath = path.join(root, "tools", "checks", "sync-fault-matrix.json");
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const groupRows = {};
for (const [group, command] of Object.entries(manifest.groups)) {
  groupRows[group] = await runCommand({
    name: `fault-${group}`,
    command: command.command,
    args: command.args,
    timeoutMs: command.timeoutMs,
    runDirectory,
  });
}
const cases = manifest.cases.map((entry) => ({
  ...entry,
  actualStatus: entry.blockedReason ? "blocked" : (groupRows[entry.group]?.status ?? "failed"),
  actualEvidence: entry.blockedReason ? null : (groupRows[entry.group]?.stdoutPath ?? null),
  reason: entry.blockedReason ?? groupRows[entry.group]?.reason ?? null,
}));
const rows = Object.values(groupRows);
const failedCases = cases.filter((entry) => entry.actualStatus !== "passed");
const result = {
  schemaVersion: 1,
  kind: "sync-fault-matrix",
  runDirectory,
  environment: await environmentFacts(),
  manifestPath,
  rows,
  cases,
  status: requiredFailures(rows).length === 0 && failedCases.length === 0 ? "passed" : "failed",
};
await writeJson(path.join(runDirectory, "fault-matrix-result.json"), result);
console.log(`SYNC_FAULT_MATRIX_JSON=${JSON.stringify(result)}`);
if (result.status !== "passed") process.exitCode = 1;

import { execFileSync } from "node:child_process";
import process from "node:process";

const started = process.hrtime.bigint();
execFileSync("cargo", ["test", "--release", "-p", "megle-onedrive-provider", "--features", "graph-emulator", "--test", "process_conformance"], {
  cwd: process.cwd(),
  stdio: "inherit",
  shell: process.platform === "win32"
});
const elapsedMs = Number(process.hrtime.bigint() - started) / 1_000_000;
console.log(JSON.stringify({ benchmark: "onedrive-provider-real-child-emulator", elapsedMs: Math.round(elapsedMs), namespaces: 8, resumedUploadBytes: 72 * 1024 * 1024, streamWindowBytes: 8 * 1024 * 1024 }));

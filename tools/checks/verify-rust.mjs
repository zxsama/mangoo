import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const cargoHome = process.env.CARGO_HOME ?? path.join(process.env.USERPROFILE ?? "", ".cargo");

const rustcCandidates = ["rustc"];
const cargoCandidates = ["cargo"];

if (process.platform === "win32") {
  rustcCandidates.push(path.join(cargoHome, "bin", "rustc.exe"));
  cargoCandidates.push(path.join(cargoHome, "bin", "cargo.exe"));
} else {
  rustcCandidates.push(path.join(cargoHome, "bin", "rustc"));
  cargoCandidates.push(path.join(cargoHome, "bin", "cargo"));
}

function commandVersion(command) {
  const result = spawnSync(command, ["--version"], {
    encoding: "utf8",
    shell: command === "rustc" || command === "cargo"
  });

  if (result.error || result.status !== 0) {
    return undefined;
  }
  return result.stdout.trim();
}

function findCommand(candidates) {
  for (const candidate of candidates) {
    if (candidate !== "rustc" && candidate !== "cargo" && !existsSync(candidate)) {
      continue;
    }
    const version = commandVersion(candidate);
    if (version) {
      return { command: candidate, version };
    }
  }
  return undefined;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: command === "cargo" || command === "rustc"
  });

  if (result.error) {
    console.error(`FAIL: ${command} ${args.join(" ")} failed: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

if (!existsSync("Cargo.toml") || !existsSync("crates/core/Cargo.toml")) {
  console.error("FAIL: Rust workspace files are missing");
  process.exit(1);
}

const rustc = findCommand(rustcCandidates);
const cargo = findCommand(cargoCandidates);

if (!rustc || !cargo) {
  console.log("SKIP: Rust toolchain unavailable; install rustc/cargo to enable cargo checks");
  process.exit(0);
}

console.log(`PASS: ${rustc.version}`);
console.log(`PASS: ${cargo.version}`);

run(process.execPath, ["tools/build/build-bundled-plugins.mjs"]);
run(cargo.command, ["fmt", "--all", "--check"]);
run(cargo.command, ["test", "--workspace"]);

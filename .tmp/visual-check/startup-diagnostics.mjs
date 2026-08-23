const ANSI_ESCAPE_PATTERN = /\u001B\[[0-?]*[ -/]*[@-~]/g;
const CARGO_FINISHED_PATTERN = /Finished [`'](?:dev|test|release)[`'] profile/i;
const CARGO_DIAGNOSTIC_PATTERN =
  /^\s*(?:warning|error)(?:\[[^\]]+\])?:|^\s*thread .* panicked at\b|\b(?:failed to|could not compile|build failed)\b/i;
const RUNTIME_DIAGNOSTIC_PATTERN = /\b(?:warn|warning|error|failed|panic|unhandled)\b/i;

function normalizeLine(line) {
  return String(line).replace(ANSI_ESCAPE_PATTERN, "");
}

function outputLines(startupOutput) {
  return startupOutput.flatMap((entry) =>
    String(entry.text)
      .split(/\r?\n/)
      .map((line) => ({ source: entry.source, line }))
  );
}

export function collectStartupDiagnostics(startupOutput, { ignoreRuntimeLine } = {}) {
  let cargoBuildFinished = false;
  let cargoBuildActive = true;
  const cargoDiagnostics = [];
  const runtimeProblems = [];

  for (const problem of outputLines(startupOutput)) {
    const normalizedLine = normalizeLine(problem.line);
    if (!normalizedLine.trim()) continue;
    if (/^\s*(?:Compiling|Checking)\s+\S+/i.test(normalizedLine)) {
      cargoBuildActive = true;
      cargoBuildFinished = false;
      continue;
    }
    if (CARGO_FINISHED_PATTERN.test(normalizedLine)) {
      cargoBuildFinished = true;
      cargoBuildActive = false;
      continue;
    }
    if (
      cargoBuildFinished &&
      /^\s*(?:Compiling|Checking)\s+\S+|^\s*warning(?:\[[^\]]+\])?:/i.test(normalizedLine)
    ) {
      cargoBuildActive = true;
      cargoBuildFinished = false;
    }
    if (!cargoBuildFinished || cargoBuildActive) {
      if (CARGO_DIAGNOSTIC_PATTERN.test(normalizedLine)) {
        cargoDiagnostics.push({ ...problem, line: normalizedLine, phase: "cargo-build" });
      }
      continue;
    }
    if (normalizedLine.includes("Running `target\\debug\\megle-core.exe`")) continue;
    if (normalizedLine.includes("Running `target/debug/megle-core`")) continue;
    if (ignoreRuntimeLine?.(normalizedLine)) continue;
    if (RUNTIME_DIAGNOSTIC_PATTERN.test(normalizedLine)) {
      runtimeProblems.push({ ...problem, line: normalizedLine, phase: "runtime" });
    }
  }

  return {
    cargoBuildFinished,
    cargoDiagnostics,
    runtimeProblems,
    startupProblems: [...cargoDiagnostics, ...runtimeProblems]
  };
}

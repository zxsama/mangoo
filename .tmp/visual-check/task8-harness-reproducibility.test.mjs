import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  prepareControlledMediaRoot,
  resolveRepositoryRoot,
  validateControlledMediaRoot
} from "./controlled-media.mjs";
import { stopThenCollectDiagnostics } from "./harness-run-finalization.mjs";
import {
  collectUnexpectedNetworkProblems,
  isExpectedTeardownNetworkProblem
} from "./network-diagnostics.mjs";
import { collectStartupDiagnostics } from "./startup-diagnostics.mjs";

const harnessDirectory = path.dirname(fileURLToPath(import.meta.url));
const repositoryRoot = path.resolve(harnessDirectory, "..", "..");
const regressionPath = path.join(harnessDirectory, "desktop-ui-regression.mjs");
const exhaustivePath = path.join(harnessDirectory, "desktop-ui-exhaustive-interaction-sweep.mjs");
const overlayCssPath = path.join(repositoryRoot, "apps", "web", "src", "design", "overlays", "overlay.css");
const appearanceFixturePath = path.join(
  repositoryRoot,
  "apps",
  "web",
  "tests",
  "fixtures",
  "overlayAppearanceVisualFixture.tsx"
);

test("repository root resolves from the harness URL instead of cwd", async () => {
  const harnessUrl = pathToFileURL(path.join(repositoryRoot, ".tmp", "visual-check", "fixture.mjs")).href;
  assert.equal(await resolveRepositoryRoot(harnessUrl), repositoryRoot);
});

test("controlled media is generated and validated without preexisting files", async () => {
  const visualRoot = path.join(
    repositoryRoot,
    ".tmp",
    "visual-check",
    "task8-selftest",
    `${Date.now()}-${process.pid}`
  );
  const first = await prepareControlledMediaRoot(visualRoot, "controlled-run");
  const second = await prepareControlledMediaRoot(visualRoot, "controlled-run");

  assert.equal(path.basename(first.root), "outputs");
  assert.deepEqual(first.files.map((file) => file.relativePath), [
    "landscape-sample.bmp",
    "portrait-sample.bmp",
    path.join("img2img-images", "nested-sample.bmp"),
    path.join("txt2img-images", "txt2img-sample.bmp"),
    ...Array.from({ length: 6 }, (_, index) =>
      path.join("img2img-grids", `grid-sample-${String(index + 1).padStart(2, "0")}.bmp`)
    )
  ]);
  assert.deepEqual(
    first.files.map((file) => file.sha256),
    second.files.map((file) => file.sha256)
  );
  assert.deepEqual(await validateControlledMediaRoot(first.root), first);
});

test("controlled media validation rejects a missing root clearly", async () => {
  const missingRoot = path.join(
    repositoryRoot,
    ".tmp",
    "visual-check",
    "task8-selftest",
    `missing-${Date.now()}-${process.pid}`,
    "outputs"
  );
  await assert.rejects(
    validateControlledMediaRoot(missingRoot),
    /controlled media fixture validation failed/i
  );
});

test("Cargo diagnostics preserve pre-finish warnings without treating progress as failures", () => {
  const diagnostics = collectStartupDiagnostics([
    { source: "stdout", text: "Compiling megle-core v0.1.0\nFinished `dev` profile [unoptimized + debuginfo] target(s)\n" },
    { source: "stdout", text: "Running `target\\debug\\megle-core.exe`\n" },
    { source: "stderr", text: "error: runtime failure\n" }
  ]);

  assert.equal(diagnostics.cargoBuildFinished, true);
  assert.deepEqual(diagnostics.cargoDiagnostics, []);
  assert.deepEqual(diagnostics.runtimeProblems.map(({ line, phase }) => ({ line, phase })), [
    { line: "error: runtime failure", phase: "runtime" }
  ]);

  const failedBuild = collectStartupDiagnostics([
    { source: "stderr", text: "Compiling megle-core v0.1.0\nwarning: real build warning\nerror[E0308]: mismatched types\n" }
  ]);
  assert.equal(failedBuild.cargoBuildFinished, false);
  assert.deepEqual(failedBuild.cargoDiagnostics.map(({ line, phase }) => ({ line, phase })), [
    { line: "warning: real build warning", phase: "cargo-build" },
    { line: "error[E0308]: mismatched types", phase: "cargo-build" }
  ]);
});

test("final diagnostics include errors emitted while the child is stopping", async () => {
  const events = [];
  const startupProblems = [];
  const diagnostics = await stopThenCollectDiagnostics({
    stop: async () => {
      events.push("stop");
      startupProblems.push({ source: "stderr", line: "late shutdown error" });
    },
    waitForOutput: async () => {
      events.push("flush");
    },
    collect: () => {
      events.push("collect");
      return {
        startupProblems: [...startupProblems],
        hardFailures: startupProblems.map((problem) => problem.line)
      };
    }
  });

  assert.deepEqual(events, ["stop", "flush", "collect"]);
  assert.deepEqual(diagnostics.startupProblems, [
    { source: "stderr", line: "late shutdown error" }
  ]);
  assert.deepEqual(diagnostics.hardFailures, ["late shutdown error"]);
});

test("normal OS regression finalizes the child before collecting and writing diagnostics", async () => {
  const source = await readFile(regressionPath, "utf8");
  const normalBranchStart = source.indexOf("const integratedTitlebarMain");
  const catchStart = source.lastIndexOf("} catch (error) {");
  const normalBranch = source.slice(normalBranchStart, catchStart);
  const finalizationIndex = normalBranch.indexOf("const runDiagnostics = await stopThenCollectDiagnostics");
  const stopIndex = normalBranch.indexOf("stop: stopDevApp", finalizationIndex);
  const waitIndex = normalBranch.indexOf("waitForOutput: () => delay(300)", stopIndex);
  const collectIndex = normalBranch.indexOf("collect: () => collectRunDiagnostics(hardFailures)", waitIndex);
  const finalSummaryIndex = normalBranch.indexOf("summary = {", collectIndex);
  const writeIndex = normalBranch.indexOf("await writeFile(summaryPath", finalSummaryIndex);

  assert.ok(normalBranchStart >= 0, "normal desktop regression branch must be present");
  assert.ok(finalizationIndex >= 0, "normal branch must use stopThenCollectDiagnostics");
  assert.ok(stopIndex > finalizationIndex);
  assert.ok(waitIndex > stopIndex);
  assert.ok(collectIndex > waitIndex);
  assert.ok(finalSummaryIndex > collectIndex);
  assert.ok(writeIndex > finalSummaryIndex);
  assert.equal(
    normalBranch.slice(0, finalizationIndex).includes("writeFile(summaryPath"),
    false,
    "normal branch must not write a pre-stop summary"
  );
  assert.match(normalBranch, /childStoppedBeforeDiagnostics: true/);
  assert.match(normalBranch, /outputWaitCompletedBeforeDiagnostics: true/);
});

test("network diagnostics accept only the strict canceled teardown abort", () => {
  const expected = {
    status: "loadingFailed",
    errorText: "net::ERR_ABORTED",
    canceled: true
  };
  const httpFailure = { status: 503, url: "http://127.0.0.1:5178/api/assets" };
  const activeAbort = {
    status: "loadingFailed",
    errorText: "net::ERR_ABORTED",
    canceled: false
  };

  assert.equal(isExpectedTeardownNetworkProblem(expected), true);
  assert.equal(isExpectedTeardownNetworkProblem(httpFailure), false);
  assert.equal(isExpectedTeardownNetworkProblem(activeAbort), false);
  assert.deepEqual(
    collectUnexpectedNetworkProblems([expected, httpFailure, activeAbort]),
    [httpFailure, activeAbort]
  );
});

test("desktop harnesses have no machine-specific roots and gate diagnostics", async () => {
  const [regressionSource, exhaustiveSource] = await Promise.all([
    readFile(regressionPath, "utf8"),
    readFile(exhaustivePath, "utf8")
  ]);

  for (const source of [regressionSource, exhaustiveSource]) {
    assert.doesNotMatch(source, /["'`]\s*[A-Z]:\\\\(?:Megle|AI_Painter)/i);
    assert.match(source, /resolveRepositoryRoot\(import\.meta\.url\)/);
    assert.match(source, /prepareControlledMediaRoot/);
  }

  assert.match(regressionSource, /collectRunDiagnostics/);
  assert.match(regressionSource, /cargoDiagnostics/);
  assert.match(regressionSource, /cargoBuildFinished/);
  for (const field of [
    "startupProblems",
    "startupFailures",
    "consoleErrors",
    "consoleFailures",
    "networkProblems",
    "networkFailures",
    "scenarioFailures",
    "hardFailures"
  ]) {
    assert.match(regressionSource, new RegExp(`${field}[:,]`));
  }
  assert.match(regressionSource, /MEGLE_VISUAL_DEBUG_PORT.*9500/);
  assert.match(regressionSource, /desktop-ui-regression-\$\{visualRunId\}\.stdout\.log/);
  assert.match(regressionSource, /desktop-ui-regression-\$\{visualRunId\}\.stderr\.log/);
  assert.match(regressionSource, /controlledMedia\.files\.length/);
  assert.match(regressionSource, /await delay\(3000\)/);
  assert.match(regressionSource, /const fallback = window\.setTimeout\(finish, 250\)/);
  assert.match(regressionSource, /window\.clearTimeout\(fallback\)/);
  assert.match(regressionSource, /const appearanceMatrix = await settingsAppearanceMatrix\(settingsClient, client\);\s*await settingsClient\.send\("Page\.bringToFront"\)/);

  assert.doesNotMatch(exhaustiveSource, /buildSummary\(true\)/);
  assert.match(exhaustiveSource, /invoked tree URL/);
  for (const field of ["startupProblems", "consoleErrors", "networkFailures", "finalHardFailures", "failures"]) {
    assert.match(exhaustiveSource, new RegExp(`${field}\\.length`));
  }
  for (const field of ["startupFailures", "consoleFailures", "networkProblems", "networkFailures"]) {
    assert.match(exhaustiveSource, new RegExp(`${field}[:,]`));
  }
  assert.match(exhaustiveSource, /Network\.enable/);
  assert.match(exhaustiveSource, /Network\.requestWillBeSent/);
  assert.match(exhaustiveSource, /Network\.responseReceived/);
  assert.match(exhaustiveSource, /Network\.loadingFailed/);
  assert.match(exhaustiveSource, /collectUnexpectedNetworkProblems\(networkProblems\)/);
  assert.match(exhaustiveSource, /finalHardFailures\.push\("network failures"\)/);
  assert.match(exhaustiveSource, /MEGLE_EXHAUSTIVE_DEBUG_PORT.*9501/);
  assert.match(exhaustiveSource, /desktop-ui-exhaustive-\$\{visualRunId\}\.stdout\.log/);
  assert.match(exhaustiveSource, /desktop-ui-exhaustive-\$\{visualRunId\}\.stderr\.log/);
  assert.match(exhaustiveSource, /cargoDiagnostics/);
  assert.match(exhaustiveSource, /cargoBuildFinished/);
});

test("DPI, preference, and appearance checks assert effective production state", async () => {
  const [source, appearanceFixture, overlayCss] = await Promise.all([
    readFile(regressionPath, "utf8"),
    readFile(appearanceFixturePath, "utf8"),
    readFile(overlayCssPath, "utf8")
  ]);

  assert.match(source, /Emulation\.setDeviceMetricsOverride/);
  assert.match(source, /devicePixelRatio/);
  assert.match(source, /protocolVisualViewport/);
  assert.match(source, /effectiveScale/);
  assert.match(source, /CDP timeout:/);
  assert.match(source, /params\.expression/);
  assert.match(source, /page scale factor did not take effect/i);
  assert.match(source, /reduced-motion emulation did not match/i);
  assert.match(source, /reduced-transparency emulation did not match/i);
  assert.match(source, /allBackdropFilters/);
  assert.match(source, /production/);
  assert.match(source, /targetEffectsChanged/);
  assert.match(source, /unaffectedGroups/);
  assert.match(source, /unexpectedChangedPaths/);
  assert.match(source, /requiredSurfaces/);
  for (const surface of [
    "notification-nav",
    "notification-content",
    "field-control",
    "select-control",
    "readable-inner",
    "selected-fill",
    "popover",
    "nested",
    "context",
    "media-grid",
    "preview-canvas"
  ]) {
    assert.match(appearanceFixture, new RegExp(`data-appearance-surface=["']${surface}["']`));
  }
  assert.match(appearanceFixture, /ModalSurface/);
  assert.match(appearanceFixture, /useSecondaryWindowInterfaceStyle/);
  for (const probe of ["popover", "nested", "context", "modal"]) {
    assert.match(appearanceFixture, new RegExp(`data-fallback-probe=["']${probe}["']`));
  }
  assert.match(source, /controlledSolidFallbackEvidence/);
  assert.match(source, /production-data-overlay-solid-fallback-variable-path/);
  assert.match(source, /solid fallback .* keyboard activation/);
  assert.match(source, /surface\.opaque/);
  assert.match(source, /surface\.readable/);
  assert.match(overlayCss, /@supports not[\s\S]*--overlay-solid-fallback-fill/);
  assert.match(overlayCss, /:root\[data-overlay-solid-fallback="forced"\][\s\S]*--overlay-solid-fallback-fill/);
  assert.match(overlayCss, /background: var\(--overlay-solid-fallback-fill/);
  assert.match(overlayCss, /--overlay-solid-fallback-filter/);
});

test("notification fixture navigation focuses before one explicit open handshake", async () => {
  const source = await readFile(regressionPath, "utf8");
  const start = source.indexOf("async function navigate(state)");
  const end = source.indexOf("async function", start + 1);
  const navigateSource = source.slice(start, end < 0 ? source.length : end);
  const focusIndex = navigateSource.indexOf('send("Page.bringToFront")');
  const openIndex = navigateSource.indexOf("notificationPopoverFixture.open()");

  assert.ok(focusIndex >= 0, "navigate must focus the fixture window");
  assert.ok(openIndex > focusIndex, "navigate must explicitly open only after focus");
  assert.equal(navigateSource.match(/notificationPopoverFixture\.open\(\)/g)?.length, 1);
  assert.match(navigateSource, /renderedOpenId/);
});

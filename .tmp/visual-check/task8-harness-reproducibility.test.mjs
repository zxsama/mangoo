import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { Writable } from "node:stream";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  prepareControlledMediaRoot,
  resolveRepositoryRoot,
  validateControlledMediaRoot
} from "./controlled-media.mjs";
import { finalizeNormalOsDiagnostics } from "./harness-run-finalization.mjs";
import {
  collectUnexpectedConsoleErrors,
  collectUnexpectedNetworkProblems,
  collectUnexpectedPhaseAwareNetworkProblems,
  isExpectedTeardownNetworkProblem
} from "./network-diagnostics.mjs";
import { collectStartupDiagnostics } from "./startup-diagnostics.mjs";

const harnessDirectory = path.dirname(fileURLToPath(import.meta.url));
const repositoryRoot = path.resolve(harnessDirectory, "..", "..");
const regressionPath = path.join(harnessDirectory, "desktop-ui-regression.mjs");
const exhaustivePath = path.join(harnessDirectory, "desktop-ui-exhaustive-interaction-sweep.mjs");
const libraryDataPath = path.join(repositoryRoot, "apps", "web", "src", "core", "useLibraryData.ts");
const mediaResourcesPath = path.join(repositoryRoot, "apps", "web", "src", "core", "mediaResources.ts");
const previewBitmapSourcePath = path.join(
  repositoryRoot,
  "apps",
  "web",
  "src",
  "features",
  "preview",
  "previewBitmapSource.ts"
);
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

test("normal OS finalization waits past 300ms for child close and both raw streams", async () => {
  const events = [];
  const startupProblems = [];
  let resolveChildClose;
  const childClose = new Promise((resolve) => {
    resolveChildClose = resolve;
  });
  const makeStream = (name) => {
    const stream = new Writable({
      write(_chunk, _encoding, callback) {
        callback();
      }
    });
    stream.on("finish", () => events.push(`${name}-finish`));
    stream.on("close", () => events.push(`${name}-close`));
    return stream;
  };
  const stdout = makeStream("stdout");
  const stderr = makeStream("stderr");
  const startedAt = Date.now();
  const diagnostics = await finalizeNormalOsDiagnostics({
    stop: async () => {
      events.push("stop");
      setTimeout(() => {
        events.push("late-output");
        startupProblems.push({ source: "stderr", line: "warning: late shutdown warning" });
        startupProblems.push({ source: "stderr", line: "error: late shutdown error" });
        stderr.write("warning: late shutdown warning\nerror: late shutdown error\n");
      }, 340);
      setTimeout(() => {
        events.push("child-close");
        resolveChildClose({ closeObserved: true, code: 0, signal: null });
      }, 380);
    },
    waitForChildClose: () => childClose,
    outputStreams: [
      { name: "stdout", stream: stdout },
      { name: "stderr", stream: stderr }
    ],
    collect: () => {
      events.push("collect");
      return {
        startupProblems: [...startupProblems],
        hardFailures: startupProblems.map((problem) => problem.line)
      };
    }
  });

  const summary = {
    run: { finalization: diagnostics.finalization },
    startupFailures: diagnostics.startupProblems,
    hardFailures: diagnostics.hardFailures
  };
  assert.ok(Date.now() - startedAt >= 330, "finalization must not use a fixed 300ms cutoff");
  assert.ok(events.indexOf("late-output") > events.indexOf("stop"));
  assert.ok(events.indexOf("child-close") > events.indexOf("late-output"));
  assert.ok(events.indexOf("collect") > events.indexOf("stdout-close"));
  assert.ok(events.indexOf("collect") > events.indexOf("stderr-close"));
  assert.deepEqual(summary.startupFailures.map((problem) => problem.line), [
    "warning: late shutdown warning",
    "error: late shutdown error"
  ]);
  assert.deepEqual(summary.hardFailures, [
    "warning: late shutdown warning",
    "error: late shutdown error"
  ]);
  assert.deepEqual(summary.run.finalization, {
    childCloseObserved: true,
    childExitCode: 0,
    childSignal: null,
    stdoutFinished: true,
    stdoutClosed: true,
    stderrFinished: true,
    stderrClosed: true,
    streamErrors: [],
    complete: true
  });
});

test("normal OS regression uses observed finalization instead of hard-coded booleans", async () => {
  const source = await readFile(regressionPath, "utf8");
  const normalBranchStart = source.indexOf("const integratedTitlebarMain");
  const catchStart = source.lastIndexOf("} catch (error) {");
  const normalBranch = source.slice(normalBranchStart, catchStart);

  assert.ok(normalBranchStart >= 0, "normal desktop regression branch must be present");
  assert.match(normalBranch, /finalizeNormalOsDiagnostics\(\{/);
  assert.match(normalBranch, /waitForChildClose: waitForDevAppClose/);
  assert.match(normalBranch, /outputStreams: outputStreams\(\)/);
  assert.match(normalBranch, /finalization: runDiagnostics\.finalization/);
  assert.doesNotMatch(normalBranch, /childStoppedBeforeDiagnostics: true/);
  assert.doesNotMatch(normalBranch, /outputWaitCompletedBeforeDiagnostics: true/);
});

test("network diagnostics accept only the strict canceled teardown abort", () => {
  const teardownAbort = {
    phase: "teardown",
    status: "loadingFailed",
    errorText: "net::ERR_ABORTED",
    canceled: true
  };
  const scenarioAbort = { ...teardownAbort, phase: "scenario" };
  const httpFailure = { status: 503, url: "http://127.0.0.1:5178/api/assets" };
  const activeAbort = {
    status: "loadingFailed",
    errorText: "net::ERR_ABORTED",
    canceled: false
  };

  assert.equal(isExpectedTeardownNetworkProblem(teardownAbort), true);
  assert.equal(isExpectedTeardownNetworkProblem(scenarioAbort), false);
  assert.equal(isExpectedTeardownNetworkProblem(httpFailure), false);
  assert.equal(isExpectedTeardownNetworkProblem(activeAbort), false);
  assert.deepEqual(
    collectUnexpectedPhaseAwareNetworkProblems([
      scenarioAbort,
      teardownAbort,
      httpFailure,
      activeAbort
    ]),
    [scenarioAbort, httpFailure, activeAbort]
  );
  assert.deepEqual(
    collectUnexpectedNetworkProblems([teardownAbort, httpFailure, activeAbort]),
    [httpFailure, activeAbort]
  );
});

test("console diagnostics preserve raw Electron artifacts but fail every other error", () => {
  const known = [
    "Electron sandboxed_renderer.bundle.js script failed to run",
    "TypeError: Cannot destructure property 'preloadScripts' of 'binding.startupData' as it is null."
  ];
  const unexpected = "TypeError: product overlay crashed";
  assert.deepEqual(collectUnexpectedConsoleErrors([...known, unexpected]), [unexpected]);
  assert.deepEqual(collectUnexpectedConsoleErrors(known), []);
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
  for (const field of ["startupProblems", "consoleFailures", "networkFailures", "finalHardFailures", "failures"]) {
    assert.match(exhaustiveSource, new RegExp(`${field}\\.length`));
  }
  for (const field of ["startupFailures", "consoleFailures", "networkProblems", "networkFailures"]) {
    assert.match(exhaustiveSource, new RegExp(`${field}[:,]`));
  }
  assert.match(exhaustiveSource, /Network\.enable/);
  assert.match(exhaustiveSource, /Network\.requestWillBeSent/);
  assert.match(exhaustiveSource, /Network\.responseReceived/);
  assert.match(exhaustiveSource, /Network\.loadingFailed/);
  assert.match(exhaustiveSource, /Network\.loadingFinished/);
  assert.match(exhaustiveSource, /pendingNetworkRequests\.delete/);
  assert.match(exhaustiveSource, /\^https\?:\\\/\\\/\[\^\/\]\+\\\/api\\\//);
  assert.match(exhaustiveSource, /tile-thumb-ready img/);
  assert.match(exhaustiveSource, /initial library interaction readiness before scenario telemetry/);
  assert.match(exhaustiveSource, /await delay\(3000\);\s*await client\.send\("Network\.enable"\)/);
  assert.match(exhaustiveSource, /await waitForNetworkIdle\("initial library load"\)/);
  assert.match(exhaustiveSource, /await waitForNetworkIdle\(`before invoke selector/);
  assert.match(exhaustiveSource, /collectUnexpectedPhaseAwareNetworkProblems\(networkProblems\)/);
  assert.match(exhaustiveSource, /phase: networkPhase/);
  assert.match(exhaustiveSource, /interaction: networkInteraction/);
  assert.match(
    exhaustiveSource,
    /networkPhase = "teardown";\s*networkInteraction = "shutdown";\s*await stopDevApp\(\)/
  );
  assert.match(exhaustiveSource, /collectUnexpectedConsoleErrors\(consoleErrors\)/);
  assert.match(exhaustiveSource, /finalHardFailures\.push\("console errors"\)/);
  assert.match(exhaustiveSource, /finalHardFailures\.push\("network failures"\)/);
  assert.match(exhaustiveSource, /MEGLE_EXHAUSTIVE_DEBUG_PORT.*9501/);
  assert.match(exhaustiveSource, /desktop-ui-exhaustive-\$\{visualRunId\}\.stdout\.log/);
  assert.match(exhaustiveSource, /desktop-ui-exhaustive-\$\{visualRunId\}\.stderr\.log/);
  assert.match(exhaustiveSource, /cargoDiagnostics/);
  assert.match(exhaustiveSource, /cargoBuildFinished/);
});

test("thumbnail priority scope changes coalesce without scenario request aborts", async () => {
  const source = await readFile(libraryDataPath, "utf8");
  const start = source.indexOf("const flushThumbnailPriorityScopeSync");
  const end = source.indexOf("const scheduleThumbnailPriorityScopeSync", start);
  const flush = source.slice(start, end);

  assert.ok(start >= 0 && end > start);
  assert.match(
    flush,
    /if \(thumbnailPriorityScopeSyncControllerRef\.current\) \{\s*thumbnailPriorityScopeSyncTrailingPendingRef\.current = true;\s*return;/
  );
  assert.doesNotMatch(flush, /thumbnailPriorityScopeSyncControllerRef\.current\?\.abort\(\)/);
  assert.match(source, /if \(thumbnailPriorityScopeSyncTimerRef\.current === null\) \{\s*openThumbnailScopeSyncWindow\(\);/);
});

test("departed preview consumers keep the shared cacheable request alive", async () => {
  const source = await readFile(mediaResourcesPath, "utf8");
  const start = source.indexOf("function withSharedAbortSignal");
  const end = source.indexOf("function createAbortError", start);
  const sharedAbort = source.slice(start, end);

  assert.ok(start >= 0 && end > start);
  assert.match(sharedAbort, /entry\.activeConsumers = Math\.max\(0, entry\.activeConsumers - 1\)/);
  assert.doesNotMatch(sharedAbort, /entry\.controller\.abort\(\)/);
  assert.match(source, /abortSharedRequests\(inFlightOriginalPreviewRequests\)/);
});

test("preview navigation cancels decode without aborting the bounded Core response", async () => {
  const source = await readFile(previewBitmapSourcePath, "utf8");
  const start = source.indexOf("export async function requestPreviewBitmap");
  const end = source.indexOf("export function prefetchPreviewBitmap", start);
  const request = source.slice(start, end);

  assert.ok(start >= 0 && end > start);
  assert.match(request, /getPreviewResource\(media\.id, \{[\s\S]*?signal: undefined/);
  assert.match(request, /decodeBlob\(resource\.blob, options\.signal, key\)/);
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
  assert.match(source, /controlledSolidFallbackEvidence[\s\S]*Page\.bringToFront[\s\S]*activateFocusedButtonWithEnter\(fixtureClient\)/);
  assert.match(source, /activateFocusedButtonWithEnter[\s\S]*text: "\\r"[\s\S]*unmodifiedText: "\\r"/);
  assert.match(source, /megle-overlay-fixture-modal[\s\S]*solid fallback non-modal keyboard phase/);
  assert.match(source, /solid fallback modal keyboard phase[\s\S]*solid fallback modal keyboard activation/);
  assert.match(appearanceFixture, /open=\{modalOpen\}/);
  assert.match(appearanceFixture, /megle-overlay-fixture-modal/);
  assert.match(source, /production-data-overlay-solid-fallback-variable-path/);
  assert.match(source, /solid fallback .* keyboard activation/);
  assert.match(source, /surface\.opaque/);
  assert.match(source, /surface\.readable/);
  assert.match(source, /contrastRatio >= 4\.5/);
  assert.match(source, /background\.alpha >= 0\.95 \|\| backdropBackground\.alpha >= 0\.95/);
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

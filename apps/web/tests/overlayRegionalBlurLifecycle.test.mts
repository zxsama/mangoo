import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

interface RegionalBlurLifecycleResult {
  ok: boolean;
  error?: string;
  root: {
    childBackdropFilter: string;
    edgeMode: string | null;
    gaussianInput: string | null;
    maskAlphaIntercept: string | null;
    maskAlphaSlope: string | null;
    maskBoundsExpanded: boolean;
    maskGaussianInput: string | null;
    maskSigma: string | null;
    roundedMaskPrimitiveCount: number;
    rootFiltered: boolean;
    stdDeviation: string | null;
    surfaceBackdropFilter: string;
  };
  nested: {
    filterCount: number;
    lowerSurfaceFiltered: boolean;
    topSurfaceFiltered: boolean;
  };
  modal: {
    modalSurfaceFiltered: boolean;
    rootMaskCoversTarget: boolean;
  };
  cleanup: {
    defsPresent: boolean;
    rootInlineFilter: string;
  };
  fallback: { rootFiltered: boolean };
  zeroBlur: { rootFiltered: boolean };
  stability: {
    afterUnchangedMask: string;
    beforeUnchangedMask: string;
    filterChangedAfterGeometry: boolean;
    filterStableAfterUnchangedEvents: boolean;
    resizeStormRebuildCount: number;
  };
  atomic: {
    decodeCalls: number;
    geometryChangedByNextFrame: boolean;
    modalPoppedByNextFrame: boolean;
    nestedPoppedByNextFrame: boolean;
    rapidStackMatchesLatestByNextFrame: boolean;
  };
}

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const electronRunner = fileURLToPath(
  new URL("./fixtures/runOverlayRegionalBlurLifecycle.mjs", import.meta.url)
);
const resultPrefix = "OVERLAY_REGIONAL_BLUR_RESULT:";

let server: ViteDevServer;
let result: RegionalBlurLifecycleResult;

async function runFixture(url: string): Promise<RegionalBlurLifecycleResult> {
  const electronModule = await import("electron");
  const electronPath = electronModule.default as unknown as string;
  const environment = { ...process.env };
  delete environment.ELECTRON_RUN_AS_NODE;

  return await new Promise((resolve, reject) => {
    const child = spawn(electronPath, [electronRunner, url], {
      cwd: webRoot,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"]
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill();
      reject(new Error(`regional blur Electron runner timed out\n${stderr}`));
    }, 45_000);

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => { stdout += chunk; });
    child.stderr.on("data", (chunk: string) => { stderr += chunk; });
    child.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on("exit", (code) => {
      clearTimeout(timeout);
      const markerIndex = stdout.lastIndexOf(resultPrefix);
      if (markerIndex < 0) {
        reject(new Error(
          `regional blur Electron runner exited ${code} without a result\n${stdout}\n${stderr}`
        ));
        return;
      }
      resolve(JSON.parse(stdout.slice(markerIndex + resultPrefix.length).trim()));
    });
  });
}

before(async () => {
  server = await createServer({
    root: webRoot,
    logLevel: "error",
    server: { host: "127.0.0.1", port: 0, strictPort: false }
  });
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address === "object");
  result = await runFixture(
    `http://127.0.0.1:${address.port}/tests/fixtures/overlay-regional-blur-lifecycle.html`
  );
  assert.equal(result.ok, true, result.error);
});

after(async () => {
  await server?.close();
});

describe("mounted overlay regional blur lifecycle", () => {
  it("flattens SourceGraphic through a synchronous rounded Gaussian mask", () => {
    assert.equal(result.root.rootFiltered, true);
    assert.equal(result.root.gaussianInput, "SourceGraphic");
    assert.equal(result.root.stdDeviation, "34");
    assert.equal(result.root.edgeMode, "duplicate");
    assert.equal(result.root.roundedMaskPrimitiveCount, 1);
    assert.equal(result.root.maskGaussianInput, "regional-mask-0-source");
    assert.equal(result.root.maskSigma, "4.32");
    assert.equal(result.root.maskAlphaSlope, "16");
    assert.equal(result.root.maskAlphaIntercept, "-7.5");
    assert.equal(result.root.maskBoundsExpanded, true);
    assert.equal(result.root.surfaceBackdropFilter, "none");
    assert.equal(result.root.childBackdropFilter, "none");
  });

  it("filters lower render planes without filtering the top nested surface", () => {
    assert.equal(result.nested.filterCount, 2);
    assert.equal(result.nested.lowerSurfaceFiltered, true);
    assert.equal(result.nested.topSurfaceFiltered, false);
  });

  it("expands a modal mask to every complete lower target", () => {
    assert.equal(result.modal.rootMaskCoversTarget, true);
    assert.equal(result.modal.modalSurfaceFiltered, false);
  });

  it("restores prior inline filters and removes generated definitions", () => {
    assert.equal(result.cleanup.defsPresent, false);
    assert.equal(result.cleanup.rootInlineFilter, "brightness(1)");
  });

  it("disables regional filtering for solid fallback and zero blur", () => {
    assert.equal(result.fallback.rootFiltered, false);
    assert.equal(result.zeroBlur.rootFiltered, false);
  });

  it("does not rebuild for unchanged event storms but follows real geometry", () => {
    assert.equal(
      result.stability.filterStableAfterUnchangedEvents,
      true,
      JSON.stringify(result.stability)
    );
    assert.ok(
      result.stability.resizeStormRebuildCount > 0 &&
        result.stability.resizeStormRebuildCount <= 2,
      `resize storm rebuilt ${result.stability.resizeStormRebuildCount} graphs`
    );
    assert.equal(result.stability.filterChangedAfterGeometry, true);
  });

  it("switches stack and geometry masks before the next painted frame", () => {
    assert.equal(result.atomic.decodeCalls, 0);
    assert.equal(result.atomic.geometryChangedByNextFrame, true);
    assert.equal(result.atomic.modalPoppedByNextFrame, true);
    assert.equal(result.atomic.nestedPoppedByNextFrame, true);
    assert.equal(result.atomic.rapidStackMatchesLatestByNextFrame, true);
  });
});

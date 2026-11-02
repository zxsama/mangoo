import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "./viteTestServer.mts";

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
    surfaceAfterBorderWidth: string | null;
    surfaceAfterBoxShadow: string | null;
    surfaceBackgroundImage: string | null;
    surfaceBackdropFilter: string;
    surfaceBorderColor: string | null;
    surfaceBorderStyle: string | null;
    surfaceBorderWidth: string | null;
  };
  rootMaskContract: MaskContract;
  firstFrame: {
    activeCount: number;
    everyActiveMaskUsesOuterBorderBox: boolean;
    everyActiveSurfaceReady: boolean;
    firstActive: {
      maskRect: { left: number; top: number; width: number; height: number } | null;
      surfaceRect: { left: number; top: number; width: number; height: number } | null;
      surfaceVisibility: string | null;
    } | null;
    trace: unknown[];
  };
  nested: {
    filterCount: number;
    lowerSurfaceFiltered: boolean;
    topSurfaceFiltered: boolean;
  };
  nestedMaskContract: MaskContract;
  modal: {
    modalSurfaceFiltered: boolean;
    rootMaskCoversTarget: boolean;
  };
  modalMaskContract: MaskContract;
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

interface MaskContract {
  outerBorderBox: { left: number; top: number; width: number; height: number } | null;
  masks: Array<{
    source: {
      result: string | null;
      rect: { left: number; top: number; width: number; height: number } | null;
    };
    rounding: {
      result: string | null;
      rect: { left: number; top: number; width: number; height: number } | null;
      stdDeviation: string | null;
    } | null;
    roundedAlphaResult: string | null;
    sourceClip: {
      in: string | null;
      in2: string | null;
      operator: string | null;
      result: string | null;
    } | null;
  }>;
  unionInputs: Array<string | null> | null;
  downstreamMaskInputs: Array<{ result: string | null; in2: string | null }>;
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
  it("clips each rounded alpha back to its complete source outer border box", () => {
    const [mask] = result.rootMaskContract.masks;
    assert.deepEqual(mask?.source.rect, result.rootMaskContract.outerBorderBox);
    assert.ok(mask?.rounding, JSON.stringify(result.rootMaskContract));
    assert.ok(
      mask.rounding.rect.left < mask.source.rect.left &&
        mask.rounding.rect.top < mask.source.rect.top &&
        mask.rounding.rect.width > mask.source.rect.width &&
        mask.rounding.rect.height > mask.source.rect.height,
      JSON.stringify(result.rootMaskContract)
    );
    assert.equal(mask.roundedAlphaResult, "regional-mask-0-rounded");
    assert.deepEqual(mask.sourceClip, {
      in: "regional-mask-0-rounded",
      in2: "regional-mask-0-source",
      operator: "in",
      result: "regional-mask-0-clipped"
    });
    assert.deepEqual(result.rootMaskContract.downstreamMaskInputs, [
      { result: "blurred-inside", in2: "regional-mask-0-clipped" },
      { result: "clear-outside", in2: "regional-mask-0-clipped" }
    ]);
  });

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

  it("never activates a regional mask for the hidden pre-layout popup", () => {
    assert.ok(result.firstFrame.activeCount > 0, JSON.stringify(result.firstFrame));
    assert.equal(
      result.firstFrame.everyActiveSurfaceReady,
      true,
      JSON.stringify(result.firstFrame)
    );
    assert.equal(
      result.firstFrame.everyActiveMaskUsesOuterBorderBox,
      true,
      JSON.stringify(result.firstFrame)
    );
  });

  it("paints the historical border and gradient on the same mounted surface", () => {
    assert.match(result.root.surfaceBackgroundImage ?? "", /linear-gradient\(108deg/);
    assert.equal(result.root.surfaceBorderWidth, "1px");
    assert.equal(result.root.surfaceBorderStyle, "solid");
    assert.equal(result.root.surfaceBorderColor, "rgba(255, 255, 255, 0.3)");
    assert.equal(result.root.surfaceAfterBorderWidth, "0px");
    assert.equal(result.root.surfaceAfterBoxShadow, "none");
  });

  it("filters lower render planes without filtering the top nested surface", () => {
    assert.equal(result.nested.filterCount, 2);
    assert.equal(result.nested.lowerSurfaceFiltered, true);
    assert.equal(result.nested.topSurfaceFiltered, false);
  });

  it("clips every rounded mask before union and feeds the union downstream", () => {
    const { masks, unionInputs, downstreamMaskInputs } = result.nestedMaskContract;
    assert.equal(masks.length, 2, JSON.stringify(result.nestedMaskContract));
    for (const [index, mask] of masks.entries()) {
      assert.equal(mask.source.result, `regional-mask-${index}-source`);
      assert.equal(mask.roundedAlphaResult, `regional-mask-${index}-rounded`);
      assert.deepEqual(mask.sourceClip, {
        in: `regional-mask-${index}-rounded`,
        in2: `regional-mask-${index}-source`,
        operator: "in",
        result: `regional-mask-${index}-clipped`
      });
    }
    assert.deepEqual(unionInputs, ["regional-mask-0-clipped", "regional-mask-1-clipped"]);
    assert.deepEqual(downstreamMaskInputs, [
      { result: "blurred-inside", in2: "regional-mask-union" },
      { result: "clear-outside", in2: "regional-mask-union" }
    ]);
  });

  it("expands a modal mask to every complete lower target", () => {
    assert.equal(result.modal.rootMaskCoversTarget, true);
    assert.equal(result.modal.modalSurfaceFiltered, false);
  });

  it("uses the exact source flood for a radius-zero full-target mask", () => {
    const [mask] = result.modalMaskContract.masks;
    assert.equal(result.modalMaskContract.masks.length, 1, JSON.stringify(result.modalMaskContract));
    assert.deepEqual(mask?.source.rect, result.modalMaskContract.outerBorderBox);
    assert.equal(mask?.source.result, "regional-mask-0-source");
    assert.equal(mask?.rounding, null);
    assert.equal(mask?.roundedAlphaResult, null);
    assert.equal(mask?.sourceClip, null);
    assert.equal(result.modalMaskContract.unionInputs, null);
    assert.deepEqual(result.modalMaskContract.downstreamMaskInputs, [
      { result: "blurred-inside", in2: "regional-mask-0-source" },
      { result: "clear-outside", in2: "regional-mask-0-source" }
    ]);
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

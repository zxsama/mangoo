import { app, BrowserWindow } from "electron";

const fixtureUrl = process.argv[2];
const resultPrefix = "OVERLAY_REGIONAL_BLUR_RESULT:";
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function evaluate(fixtureWindow, expression) {
  return await fixtureWindow.webContents.executeJavaScript(expression, true);
}

async function waitFor(fixtureWindow, expression, label, timeoutMs = 6000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(fixtureWindow, expression)) return;
    await delay(40);
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function settle() {
  await delay(180);
}

async function setPhase(fixtureWindow, phase) {
  await evaluate(
    fixtureWindow,
    `window.dispatchEvent(new CustomEvent("megle-regional-blur-phase", { detail: ${JSON.stringify(phase)} })); true`
  );
}

async function main() {
  await app.whenReady();
  const fixtureWindow = new BrowserWindow({
    show: false,
    skipTaskbar: true,
    width: 900,
    height: 650,
    webPreferences: {
      backgroundThrottling: false,
      contextIsolation: true,
      sandbox: true
    }
  });

  try {
    await fixtureWindow.loadURL(fixtureUrl);
    await waitFor(
      fixtureWindow,
      `Boolean(document.querySelector("main"))`,
      "fixture mount"
    );

    await evaluate(fixtureWindow, `(() => {
      const original = HTMLImageElement.prototype.decode;
      window.__regionalBlurOriginalDecode = original;
      window.__regionalBlurDecodeCalls = 0;
      HTMLImageElement.prototype.decode = function delayedRegionalMaskDecode() {
        window.__regionalBlurDecodeCalls += 1;
        return new Promise((resolve, reject) => {
          setTimeout(() => original.call(this).then(resolve, reject), 120);
        });
      };
    })()`);

    await setPhase(fixtureWindow, "root");
    await waitFor(
      fixtureWindow,
      `Boolean(document.querySelector("#regional-lifecycle-root"))`,
      "pending root surface"
    );
    await waitFor(
      fixtureWindow,
      `document.querySelector("#root")?.getAttribute("data-overlay-regional-blur") === "active" && getComputedStyle(document.querySelector("#regional-lifecycle-root")).visibility === "visible" && document.querySelector('filter[data-overlay-regional-blur-target="app-root"] [data-overlay-regional-mask-primitive="true"]')?.getAttribute("width") === "280"`,
      "positioned root regional filter"
    );
    const root = await evaluate(fixtureWindow, `(() => {
      const defs = document.querySelector('svg[data-overlay-regional-blur-defs="true"]');
      const filter = defs?.querySelector('filter[data-overlay-regional-blur-target="app-root"]');
      const gaussian = filter?.querySelector('feGaussianBlur');
      const maskSource = filter?.querySelector('[data-overlay-regional-mask-primitive="true"]');
      const maskGaussian = filter?.querySelector('[data-overlay-regional-mask-rounding="true"]');
      const maskAlpha = maskGaussian?.nextElementSibling?.querySelector('feFuncA') ?? null;
      const number = (element, name) => Number(element?.getAttribute(name) ?? NaN);
      const surface = document.querySelector('#regional-lifecycle-root');
      const child = surface?.querySelector(':scope > .liquid-glass-backdrop');
      return {
        rootFiltered: document.querySelector('#root')?.getAttribute('data-overlay-regional-blur') === 'active',
        gaussianInput: gaussian?.getAttribute('in') ?? null,
        stdDeviation: gaussian?.getAttribute('stdDeviation') ?? null,
        edgeMode: gaussian?.getAttribute('edgeMode') ?? null,
        maskGaussianInput: maskGaussian?.getAttribute('in') ?? null,
        maskSigma: maskGaussian?.getAttribute('stdDeviation') ?? null,
        maskAlphaSlope: maskAlpha?.getAttribute('slope') ?? null,
        maskAlphaIntercept: maskAlpha?.getAttribute('intercept') ?? null,
        maskBoundsExpanded:
          number(maskGaussian, 'x') <= number(maskSource, 'x') - number(maskGaussian, 'stdDeviation') * 3 &&
          number(maskGaussian, 'y') <= number(maskSource, 'y') - number(maskGaussian, 'stdDeviation') * 3 &&
          number(maskGaussian, 'width') >= number(maskSource, 'width') + number(maskGaussian, 'stdDeviation') * 6 &&
          number(maskGaussian, 'height') >= number(maskSource, 'height') + number(maskGaussian, 'stdDeviation') * 6,
        roundedMaskPrimitiveCount: filter?.querySelectorAll('[data-overlay-regional-mask-primitive="true"]').length ?? 0,
        surfaceBackdropFilter: surface ? getComputedStyle(surface).backdropFilter : '',
        childBackdropFilter: child ? getComputedStyle(child).backdropFilter : ''
      };
    })()`);
    await settle();
    const stableFilterId = await evaluate(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.id ?? ''`
    );
    const beforeUnchangedMask = await evaluate(
      fixtureWindow,
      `(() => { const node = document.querySelector('filter[data-overlay-regional-blur-target="app-root"] [data-overlay-regional-mask-primitive="true"]'); return node ? [node.getAttribute('x'), node.getAttribute('y'), node.getAttribute('width'), node.getAttribute('height')].join(',') : ''; })()`
    );
    await evaluate(fixtureWindow, `(() => {
      for (let index = 0; index < 30; index += 1) {
        document.querySelector('main')?.dispatchEvent(new Event('scroll', { bubbles: false }));
      }
      document.querySelector('#regional-lifecycle-root > .overlay-scroll-viewport > div')
        ?.dispatchEvent(new Event('animationend', { bubbles: true }));
    })()`);
    await settle();
    const filterAfterUnchangedEvents = await evaluate(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.id ?? ''`
    );
    const afterUnchangedMask = await evaluate(
      fixtureWindow,
      `(() => { const node = document.querySelector('filter[data-overlay-regional-blur-target="app-root"] [data-overlay-regional-mask-primitive="true"]'); return node ? [node.getAttribute('x'), node.getAttribute('y'), node.getAttribute('width'), node.getAttribute('height')].join(',') : ''; })()`
    );
    const resizeStormRebuildCount = await evaluate(fixtureWindow, `(async () => {
      const surface = document.querySelector('#regional-lifecycle-root');
      const currentFilterId = () => document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.id ?? '';
      const revision = (id) => Number(id.match(/blur-\\d+-(\\d+)-0$/)?.[1] ?? 0);
      if (!(surface instanceof HTMLElement)) return -1;
      const original = surface.getBoundingClientRect.bind(surface);
      let reads = 0;
      surface.getBoundingClientRect = () => {
        const rect = original();
        const shift = ++reads;
        return new DOMRect(rect.x + shift, rect.y, rect.width, rect.height);
      };
      const before = revision(currentFilterId());
      for (let index = 0; index < 30; index += 1) window.dispatchEvent(new Event('resize'));
      await new Promise((resolve) => requestAnimationFrame(resolve));
      const after = revision(currentFilterId());
      surface.getBoundingClientRect = original;
      surface.dispatchEvent(new Event('transitionend'));
      return after - before;
    })()`);
    const geometryChangedByNextFrame = await evaluate(fixtureWindow, `(async () => {
      const surface = document.querySelector('#regional-lifecycle-root');
      if (surface instanceof HTMLElement) {
        surface.style.left = '220px';
        surface.dispatchEvent(new Event('transitionend'));
      }
      await new Promise((resolve) => requestAnimationFrame(resolve));
      return document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.id !== ${JSON.stringify(stableFilterId)};
    })()`);
    await waitFor(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.id !== ${JSON.stringify(stableFilterId)}`,
      "geometry-driven regional rebuild"
    );
    const changedFilterId = await evaluate(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.id ?? ''`
    );
    const stability = {
      beforeUnchangedMask,
      afterUnchangedMask,
      filterStableAfterUnchangedEvents: filterAfterUnchangedEvents === stableFilterId,
      filterChangedAfterGeometry: changedFilterId !== stableFilterId,
      resizeStormRebuildCount
    };

    await setPhase(fixtureWindow, "nested");
    await waitFor(
      fixtureWindow,
      `document.querySelector("#regional-lifecycle-root")?.getAttribute("data-overlay-regional-blur") === "active"`,
      "lower nested target filter"
    );
    const nested = await evaluate(fixtureWindow, `(() => ({
      filterCount: document.querySelectorAll('svg[data-overlay-regional-blur-defs="true"] filter').length,
      lowerSurfaceFiltered: document.querySelector('#regional-lifecycle-root')?.getAttribute('data-overlay-regional-blur') === 'active',
      topSurfaceFiltered: document.querySelector('#regional-lifecycle-nested')?.getAttribute('data-overlay-regional-blur') === 'active'
    }))()`);

    await setPhase(fixtureWindow, "modal");
    await waitFor(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.getAttribute("data-overlay-regional-full-target") === "true"`,
      "modal full-target mask"
    );
    const modal = await evaluate(fixtureWindow, `(() => ({
      rootMaskCoversTarget: document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.getAttribute('data-overlay-regional-full-target') === 'true',
      modalSurfaceFiltered: document.querySelector('#regional-lifecycle-modal')?.getAttribute('data-overlay-regional-blur') === 'active'
    }))()`);

    const modalPoppedByNextFrame = await evaluate(fixtureWindow, `(async () => {
      window.dispatchEvent(new CustomEvent("megle-regional-blur-phase", { detail: "nested" }));
      await new Promise((resolve) => requestAnimationFrame(resolve));
      const rootFilter = document.querySelector('filter[data-overlay-regional-blur-target="app-root"]');
      return rootFilter?.getAttribute('data-overlay-regional-full-target') === 'false' &&
        document.querySelectorAll('svg[data-overlay-regional-blur-defs="true"] filter').length === 2;
    })()`);
    await waitFor(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.getAttribute("data-overlay-regional-full-target") === "false"`,
      "modal pop regional mask"
    );

    const nestedPoppedByNextFrame = await evaluate(fixtureWindow, `(async () => {
      window.dispatchEvent(new CustomEvent("megle-regional-blur-phase", { detail: "root" }));
      await new Promise((resolve) => requestAnimationFrame(resolve));
      const rootFilter = document.querySelector('filter[data-overlay-regional-blur-target="app-root"]');
      return rootFilter?.getAttribute('data-overlay-regional-mask-count') === '1' &&
        document.querySelectorAll('svg[data-overlay-regional-blur-defs="true"] filter').length === 1;
    })()`);
    await waitFor(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.getAttribute("data-overlay-regional-mask-count") === "1" && document.querySelectorAll('svg[data-overlay-regional-blur-defs="true"] filter').length === 1`,
      "nested pop regional mask"
    );

    const rapidStackMatchesLatestByNextFrame = await evaluate(fixtureWindow, `(async () => {
      window.dispatchEvent(new CustomEvent("megle-regional-blur-phase", { detail: "nested" }));
      await new Promise((resolve) => setTimeout(resolve, 20));
      window.dispatchEvent(new CustomEvent("megle-regional-blur-phase", { detail: "modal" }));
      await new Promise((resolve) => requestAnimationFrame(resolve));
      return document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.getAttribute('data-overlay-regional-full-target') === 'true';
    })()`);
    await setPhase(fixtureWindow, "root");
    await waitFor(
      fixtureWindow,
      `document.querySelector('filter[data-overlay-regional-blur-target="app-root"]')?.getAttribute("data-overlay-regional-mask-count") === "1"`,
      "rapid stack reset"
    );

    const decodeCalls = await evaluate(fixtureWindow, `window.__regionalBlurDecodeCalls ?? 0`);
    await evaluate(fixtureWindow, `(() => {
      HTMLImageElement.prototype.decode = window.__regionalBlurOriginalDecode;
      delete window.__regionalBlurOriginalDecode;
      delete window.__regionalBlurDecodeCalls;
    })()`);
    const atomic = {
      decodeCalls,
      geometryChangedByNextFrame,
      modalPoppedByNextFrame,
      nestedPoppedByNextFrame,
      rapidStackMatchesLatestByNextFrame
    };

    await setPhase(fixtureWindow, "closed");
    await waitFor(
      fixtureWindow,
      `!document.querySelector('svg[data-overlay-regional-blur-defs="true"]')`,
      "regional filter cleanup"
    );
    const cleanup = await evaluate(fixtureWindow, `(() => ({
      defsPresent: Boolean(document.querySelector('svg[data-overlay-regional-blur-defs="true"]')),
      rootInlineFilter: document.querySelector('#root')?.style.filter ?? ''
    }))()`);

    await evaluate(
      fixtureWindow,
      `document.documentElement.setAttribute("data-overlay-solid-fallback", "forced")`
    );
    await setPhase(fixtureWindow, "root");
    await waitFor(fixtureWindow, `Boolean(document.querySelector('#regional-lifecycle-root'))`, "fallback root");
    await settle();
    const fallback = await evaluate(fixtureWindow, `({
      rootFiltered: document.querySelector('#root')?.getAttribute('data-overlay-regional-blur') === 'active'
    })`);

    await setPhase(fixtureWindow, "closed");
    await settle();
    await evaluate(fixtureWindow, `(() => {
      document.documentElement.removeAttribute("data-overlay-solid-fallback");
      document.documentElement.style.setProperty("--glass-dialog-blur", "0px");
    })()`);
    await setPhase(fixtureWindow, "root");
    await waitFor(fixtureWindow, `Boolean(document.querySelector('#regional-lifecycle-root'))`, "zero blur root");
    await settle();
    const zeroBlur = await evaluate(fixtureWindow, `({
      rootFiltered: document.querySelector('#root')?.getAttribute('data-overlay-regional-blur') === 'active'
    })`);

    return { ok: true, root, nested, modal, cleanup, fallback, zeroBlur, stability, atomic };
  } finally {
    fixtureWindow.destroy();
  }
}

main().then(
  (result) => process.stdout.write(`${resultPrefix}${JSON.stringify(result)}\n`, () => app.exit(0)),
  (error) => process.stdout.write(`${resultPrefix}${JSON.stringify({
    ok: false,
    error: error instanceof Error ? error.stack ?? error.message : String(error)
  })}\n`, () => app.exit(1))
);

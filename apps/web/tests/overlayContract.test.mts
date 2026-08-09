import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

const stripCssComments = (source: string) => {
  return source.replace(/\/\*[\s\S]*?\*\//g, "");
};

const cssRuleSelectors = (source: string) => {
  const withoutComments = stripCssComments(source);
  const selectors: string[] = [];
  let quote: '"' | "'" | null = null;
  let escaped = false;

  let preludeStart = 0;
  for (let index = 0; index < withoutComments.length; index += 1) {
    const character = withoutComments[index];
    if (quote) {
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === '"' || character === "'") {
      quote = character;
      continue;
    }
    if (character === "{") {
      const prelude = withoutComments.slice(preludeStart, index).trim();
      if (prelude && !prelude.startsWith("@")) selectors.push(prelude);
      preludeStart = index + 1;
    } else if (character === "}") {
      preludeStart = index + 1;
    }
  }
  return selectors;
};

const hasCompleteSelector = (source: string, selector: string) =>
  cssRuleSelectors(source)
    .flatMap((rule) => rule.split(","))
    .some((candidate) => new RegExp(`^${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?:[\\s:.#[>+~]|$)`).test(candidate.trim()));

const cssDeclarationsForExactSelector = (source: string, selector: string) =>
  [...stripCssComments(source).matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter((match) => match[1].split(",").some((item) => item.trim() === selector))
    .map((match) => match[2]);

const overlaySectionMarkers = [
  "/* Core variables and surfaces */",
  "/* Anchored positioning and modal shell */",
  "/* Menu and field controls */",
  "/* Titlebar overlays */",
  "/* Library switcher */",
  "/* Context menus and file dialogs */",
  "/* Compact notification */",
  "/* Full notification center */",
  "/* Responsive, reduced motion, reduced transparency, and backdrop-filter fallback */"
];

const sectionPositions = (source: string, markers: string[]) =>
  markers.map((marker) => {
    const escapedMarker = marker.replaceAll("*", String.fromCharCode(92) + "*");
    const matches = [...source.matchAll(new RegExp(`^[ \\t]*${escapedMarker}[ \\t]*\\r?$`, "gm"))];
    assert.equal(matches.length, 1, `${marker} must appear exactly once`);
    return matches[0]?.index ?? -1;
  });

describe("shared overlay source contract", () => {
  const mainSource = read("src/main.tsx");
  const settingsWindowSource = read("src/features/settings/SettingsWindow.tsx");
  const pluginsWindowSource = read("src/features/plugins/PluginsWindow.tsx");
  const legacyStyles = read("src/styles.css");
  const overlayCss = read("src/design/overlays/overlay.css");
  const anchoredOverlaySource = read("src/design/overlays/AnchoredOverlay.tsx");
  const overlayPositionSource = read("src/design/overlays/overlayPosition.ts");
  const overlaySourceBundle = [
    "overlayTypes.ts",
    "overlayMeasurement.ts",
    "overlayPosition.ts",
    "overlayStack.ts",
    "overlayRuntime.ts",
    "overlayMenuNavigation.ts",
    "overlayPresence.ts",
    "useOverlayPresence.ts",
    "OverlayProvider.tsx",
    "OverlaySurface.tsx",
    "AnchoredOverlay.tsx",
    "OverlayMenu.tsx",
    "useOverlayFocusTrap.ts",
    "OverlayDialog.tsx",
    "OverlayField.tsx",
    "index.ts"
  ].map((path) => read(`src/design/overlays/${path}`)).join("\n");

  it("mounts overlays only around the main app", () => {
    assert.match(mainSource, /design\/overlays\/overlay\.css/);
    assert.match(mainSource, /<OverlayProvider>/);
    assert.doesNotMatch(settingsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
    assert.doesNotMatch(pluginsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
  });

  it("uses existing material variables and no forbidden dependencies", () => {
    assert.match(overlayCss, /var\(--glass-dialog-fill\)/);
    assert.match(overlayCss, /var\(--glass-center-stroke\)/);
    assert.doesNotMatch(overlaySourceBundle, /fetch\s*\(|node:fs|@radix|@floating-ui/);
  });

  it("keeps popup material on one direct frosted backdrop", () => {
    const directBackdrop = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-surface > .liquid-glass-backdrop"
    ).join("\n");

    assert.match(directBackdrop, /linear-gradient\(\s*108deg/s);
    assert.match(directBackdrop, /var\(--glass-dialog-pane-light\)\s+0%/);
    assert.match(directBackdrop, /var\(--glass-dialog-pane-mid\)\s+52%/);
    assert.match(directBackdrop, /var\(--glass-dialog-pane-end\)\s+100%/);
    assert.match(directBackdrop, /blur\(var\(--glass-dialog-blur\)\)/);
    assert.match(directBackdrop, /var\(\s*--overlay-solid-fallback-fill,/);

    const outerSurface = cssDeclarationsForExactSelector(overlayCss, ".overlay-surface").join("\n");
    assert.match(outerSurface, /background:\s*transparent/);

    for (const selector of [
      ".overlay-surface-popover",
      ".overlay-surface-nested",
      ".library-switcher",
      ".library-switcher-header",
      ".library-switcher-list",
      ".library-switcher-footer",
      "#library-remove-confirm",
      ".context-menu",
      ".file-operation-dialog"
    ]) {
      const declarations = cssDeclarationsForExactSelector(overlayCss, selector).join("\n");
      assert.notEqual(declarations, "", `${selector} must have a production rule`);
      assert.match(declarations, /background:\s*transparent/);
      assert.doesNotMatch(declarations, /--glass-side-fill|--glass-dialog-fill/);
    }
  });

  it("dims only outside a modal while its bounded surface owns the blur", () => {
    const modalViewport = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-modal-backdrop"
    ).join("\n");
    const modalSurface = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-modal-surface"
    ).join("\n");
    const directBackdrop = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-surface > .liquid-glass-backdrop"
    ).join("\n");

    assert.match(modalViewport, /background:\s*transparent/);
    assert.match(modalViewport, /-webkit-backdrop-filter:\s*none/);
    assert.match(modalViewport, /(?:^|\s)backdrop-filter:\s*none/);
    assert.match(modalSurface, /0\s+0\s+0\s+100vmax\s+var\(--dialog-backdrop-dim\)/);
    assert.match(modalSurface, /var\(--glass-shadow\)/);
    assert.match(directBackdrop, /blur\(var\(--glass-dialog-blur\)\)/);
  });

  it("keeps migrated overlay ownership in overlay.css", () => {
    for (const selector of [
      ".context-menu",
      ".dialog-backdrop",
      ".library-switcher-backdrop",
      ".sort-menu-list",
      ".filter-menu-popover",
      ".notification-popover",
      ".notification-window-body",
      ".filter-menu-trigger",
      ".filter-menu-dot"
    ]) {
      assert.equal(hasCompleteSelector(legacyStyles, selector), false, `${selector} remains in styles.css`);
      assert.equal(hasCompleteSelector(overlayCss, selector), true, `${selector} is missing from overlay.css`);
    }
    assert.equal(
      hasCompleteSelector(legacyStyles, ".dialog-error"),
      false,
      ".dialog-error remains in styles.css"
    );

    assert.doesNotMatch(legacyStyles, /rgba\(20,\s*20,\s*22,\s*0\.98\)/);
    assert.doesNotMatch(legacyStyles, /rgba\(23,\s*27,\s*32,\s*0\.99\)/);
    assert.doesNotMatch(legacyStyles, /#1c1c1e/i);
  });

  it("keeps overlay sections unique and ordered", () => {
    const positions = sectionPositions(overlayCss, overlaySectionMarkers);
    assert.ok(positions.every((position, index) => index === 0 || position > positions[index - 1]));
  });

  it("does not mistake CSS comments or strings for selector ownership", () => {
    const fixture = `
      /* .filter-menu-trigger { color: red; } */
      .search-bar[data-titlebar-search="true"] { content: ".filter-menu-trigger"; }
    `;
    assert.equal(hasCompleteSelector(fixture, ".filter-menu-trigger"), false);
    assert.equal(hasCompleteSelector(fixture, ".search-bar"), true);
  });

  it("preserves the locked anchor API and one rendered width path", () => {
    assert.doesNotMatch(
      anchoredOverlaySource,
      /interface AnchoredOverlayProps\s*{[\s\S]*?\boffset\?:[\s\S]*?}/
    );
    assert.match(anchoredOverlaySource, /measureOverlayIntrinsicWidth\(surface\)/);
    assert.match(anchoredOverlaySource, /width:\s*layout\?\.width/);
    assert.match(
      overlayPositionSource,
      /ROOT_OVERLAY_OFFSET_PX,[\s\S]*?from "\.\/overlayTypes\.ts"/
    );
    assert.doesNotMatch(overlayPositionSource, /const ROOT_OVERLAY_OFFSET_PX/);
    assert.doesNotMatch(overlayPositionSource, /const NESTED_OVERLAY_OFFSET_PX/);
  });

  it("uses translation and opacity presence motion without scale", () => {
    assert.doesNotMatch(overlayCss, /scale\s*\(/);
    assert.match(
      overlayCss,
      /overlay-surface-popover\[data-overlay-state="opening"\][\s\S]*?140ms/
    );
    assert.match(
      overlayCss,
      /overlay-surface-popover\[data-overlay-state="closing"\][\s\S]*?140ms/
    );
    assert.match(
      overlayCss,
      /overlay-surface-nested\[data-overlay-state="opening"\][\s\S]*?110ms/
    );
    assert.match(
      overlayCss,
      /overlay-surface-nested\[data-overlay-state="closing"\][\s\S]*?110ms/
    );
    assert.match(overlayCss, /overlay-modal-backdrop\[data-overlay-state="opening"\]/);
    assert.match(overlayCss, /overlay-modal-backdrop\[data-overlay-state="closing"\]/);
    assert.match(overlayCss, /prefers-reduced-motion:\s*reduce[\s\S]*?animation:\s*none/);
  });
});

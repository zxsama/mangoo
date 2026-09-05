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
  const overlayProviderSource = read("src/design/overlays/OverlayProvider.tsx");
  const regionalBlurSource = read("src/design/overlays/overlayRegionalBlur.ts");
  const overlayPositionSource = read("src/design/overlays/overlayPosition.ts");
  const overlaySourceBundle = [
    "overlayTypes.ts",
    "overlayMeasurement.ts",
    "overlayPosition.ts",
    "overlayStack.ts",
    "overlayRuntime.ts",
    "overlayRegionalBlurPlan.ts",
    "overlayRegionalBlur.ts",
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

  it("mounts the shared overlay stack around every renderer role", () => {
    assert.match(mainSource, /design\/overlays\/overlay\.css/);
    assert.match(mainSource, /<OverlayProvider>\{children\}<\/OverlayProvider>/);
    assert.match(mainSource, /<ApplicationProviders><SettingsWindow \/><\/ApplicationProviders>/);
    assert.match(mainSource, /<ApplicationProviders><PluginsWindow \/><\/ApplicationProviders>/);
    assert.match(mainSource, /<ApplicationProviders><NotificationWindow \/><\/ApplicationProviders>/);
    assert.doesNotMatch(mainSource, /<ApplicationProviders><OverlayProvider>/);
    assert.doesNotMatch(settingsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
    assert.doesNotMatch(pluginsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
  });

  it("installs the shared overlay scrollbar controller in secondary dialog windows", () => {
    assert.match(settingsWindowSource, /useOverlayScrollbars\(\)/);
    assert.match(pluginsWindowSource, /useOverlayScrollbars\(\)/);
  });

  it("uses existing material variables and no forbidden dependencies", () => {
    assert.match(overlayCss, /var\(--glass-dialog-fill\)/);
    assert.match(overlayCss, /var\(--glass-center-stroke\)/);
    assert.doesNotMatch(overlaySourceBundle, /fetch\s*\(|node:fs|@radix|@floating-ui/);
  });

  it("keeps popup material on the portal-mounted surface", () => {
    const surface = cssDeclarationsForExactSelector(overlayCss, ".overlay-surface").join("\n");
    const frame = cssDeclarationsForExactSelector(
      overlayCss,
      ".liquid-glass.overlay-surface"
    ).join("\n");
    const retiredFramePlane = cssDeclarationsForExactSelector(
      overlayCss,
      ".liquid-glass.overlay-surface::after"
    ).join("\n");
    const child = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-surface > .liquid-glass-backdrop"
    ).join("\n");

    assert.match(surface, /linear-gradient\(\s*108deg/s);
    assert.match(surface, /var\(--glass-dialog-pane-light\)\s+0%/);
    assert.match(surface, /var\(--glass-dialog-pane-mid\)\s+52%/);
    assert.match(surface, /var\(--glass-dialog-pane-end\)\s+100%/);
    assert.match(surface, /var\(\s*--overlay-solid-fallback-fill,/);
    assert.match(surface, /-webkit-backdrop-filter:\s*none/);
    assert.match(surface, /(?:^|\s)backdrop-filter:\s*none/);
    assert.doesNotMatch(surface, /blur\(var\(--glass-dialog-blur\)\)/);
    assert.match(surface, /--glass-border-current:\s*rgba\(255,\s*255,\s*255,\s*0\.3\)/);

    assert.match(frame, /border:\s*1px\s+solid\s+var\(--glass-border-current\)/);
    assert.match(frame, /background-clip:\s*border-box/);
    assert.doesNotMatch(frame, /background-clip:\s*padding-box/);
    assert.match(retiredFramePlane, /border:\s*0/);
    assert.match(retiredFramePlane, /box-shadow:\s*none/);

    assert.match(child, /background:\s*transparent/);
    assert.match(child, /-webkit-backdrop-filter:\s*none/);
    assert.match(child, /(?:^|\s)backdrop-filter:\s*none/);
    assert.doesNotMatch(child, /glass-dialog-pane|glass-dialog-blur|overlay-solid-fallback/);

    for (const selector of [
      ".overlay-surface-popover",
      ".overlay-surface-nested",
      ".library-switcher",
      "#library-remove-confirm",
      ".context-menu",
      ".file-operation-dialog"
    ]) {
      const declarations = cssDeclarationsForExactSelector(overlayCss, selector).join("\n");
      assert.notEqual(declarations, "", `${selector} must have a production rule`);
      assert.doesNotMatch(declarations, /(?:^|;)\s*background\s*:/);
      assert.doesNotMatch(declarations, /--glass-side-fill|--glass-dialog-fill/);
    }
  });

  it("dims only outside a modal while the regional scene owns the blur", () => {
    const modalViewport = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-modal-backdrop"
    ).join("\n");
    const modalSurface = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-modal-surface"
    ).join("\n");
    const surface = cssDeclarationsForExactSelector(overlayCss, ".overlay-surface").join("\n");
    const child = cssDeclarationsForExactSelector(
      overlayCss,
      ".overlay-surface > .liquid-glass-backdrop"
    ).join("\n");

    assert.match(modalViewport, /background:\s*transparent/);
    assert.match(modalViewport, /-webkit-backdrop-filter:\s*none/);
    assert.match(modalViewport, /(?:^|\s)backdrop-filter:\s*none/);
    assert.match(modalSurface, /0\s+0\s+0\s+100vmax\s+var\(--dialog-backdrop-dim\)/);
    assert.match(modalSurface, /var\(--glass-shadow\)/);
    assert.match(surface, /backdrop-filter:\s*none/);
    assert.match(child, /backdrop-filter:\s*none/);
    assert.doesNotMatch(
      overlayCss,
      /\.overlay-modal-backdrop\[data-overlay-state="(?:opening|closing)"\]\s*\{[^}]*animation:/s
    );
    assert.match(
      overlayCss,
      /\.overlay-modal-surface\[data-overlay-state="opening"\][\s\S]*?160ms/
    );
    assert.match(
      overlayCss,
      /\.overlay-modal-surface\[data-overlay-state="closing"\][\s\S]*?160ms/
    );
  });

  it("binds the ordered overlay stack to the regional SourceGraphic filter", () => {
    assert.match(overlayProviderSource, /createOverlayRegionalBlurController/);
    assert.match(overlayProviderSource, /regionalBlurControllerRef\.current\?\.setLayers\(state\.layers\)/);
    assert.match(regionalBlurSource, /buildOverlayRegionalBlurPlans/);
    assert.match(regionalBlurSource, /SourceGraphic/);
    assert.match(regionalBlurSource, /feGaussianBlur/);
    assert.match(regionalBlurSource, /feColorMatrix/);
    assert.match(regionalBlurSource, /feFlood/);
    assert.match(regionalBlurSource, /feComponentTransfer/);
    assert.match(regionalBlurSource, /feComposite/);
    assert.match(regionalBlurSource, /feMerge/);
    assert.doesNotMatch(regionalBlurSource, /feImage|image\.decode|data:image\/svg\+xml/);
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
    assert.doesNotMatch(
      overlayCss,
      /\.overlay-modal-backdrop\[data-overlay-state="(?:opening|closing)"\]\s*\{[^}]*animation:/s
    );
    assert.match(overlayCss, /prefers-reduced-motion:\s*reduce[\s\S]*?animation:\s*none/);
  });
});

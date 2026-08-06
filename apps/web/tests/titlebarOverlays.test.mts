import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const readRepo = (path: string) =>
  readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8");

const cssRuleSelectors = (source: string) => {
  const withoutComments = source.replace(/\/\*[\s\S]*?\*\//g, "");
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
    .some((candidate) =>
      new RegExp(`^${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?:[\\s:.#[>+~]|$)`).test(candidate.trim())
    );

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

describe("titlebar overlay migration", () => {
  const app = read("src/app/App.tsx");
  const filter = read("src/features/library/FilterMenu.tsx");
  const sort = read("src/features/library/SortMenu.tsx");
  const layout = read("src/features/library/LayoutMenu.tsx");
  const search = read("src/features/library/SearchBar.tsx");
  const overlayField = read("src/design/overlays/OverlayField.tsx");
  const overlayFixture = read("tests/fixtures/overlayLifecycleFixture.tsx");
  const shellOverlayHost = read("src/app-shell/ShellOverlayHost.tsx");
  const styles = read("src/styles.css");
  const overlayCss = read("src/design/overlays/overlay.css");
  const webValidator = readRepo("tools/checks/validate-web-client.mjs");
  const uiValidator = readRepo("tools/checks/validate-ui-design.mjs");

  it("uses one shared root state", () => {
    assert.match(app, /useOverlayStack/);
    assert.doesNotMatch(app, /type CompactPopover/);
    assert.doesNotMatch(layout, /useState\(false\)/);
  });

  it("ports all titlebar menus through AnchoredOverlay", () => {
    for (const source of [filter, sort, layout]) assert.match(source, /AnchoredOverlay/);
    for (const source of [filter, sort, layout]) assert.doesNotMatch(source, /createPortal/);
  });

  it("keeps only the search input pointer-glow-free", () => {
    assert.match(search, /OverlayField/);
    assert.doesNotMatch(search, /data-pointer-glow="off"/);
    assert.match(overlayField, /data-pointer-glow="off"/);
    assert.doesNotMatch(search, /AnchoredOverlay|createPortal/);
    assert.doesNotMatch(styles, /\.search-bar-clear:hover/);
    assert.doesNotMatch(styles, /\.search-bar\[data-titlebar-search="true"\]:hover/);
  });

  it("keeps migrated titlebar ownership in the shared stylesheet", () => {
    for (const selector of [
      '.search-bar[data-titlebar-search="true"]',
      ".filter-menu-root",
      ".filter-menu-popover",
      ".sort-menu-list",
      ".sort-menu-options",
      ".layout-menu-options"
    ]) {
      assert.equal(hasCompleteSelector(overlayCss, selector), true, `${selector} is not an owned CSS rule`);
    }
    for (const selector of [
      ".search-bar",
      ".filter-menu-popover",
      ".sort-menu-list",
      ".layout-menu-options",
      ".filter-menu-trigger",
      ".filter-menu-dot"
    ]) {
      assert.equal(hasCompleteSelector(styles, selector), false, `${selector} remains in styles.css`);
    }

    const sectionOrder = sectionPositions(overlayCss, overlaySectionMarkers);
    assert.ok(sectionOrder.every((index, position) => position === 0 || index > sectionOrder[position - 1]));
  });

  it("requires actual CSS rules and ignores comment/string lookalikes", () => {
    const fixture = `
      /* .filter-menu-trigger { color: red; } */
      .search-bar[data-titlebar-search="true"] { content: ".filter-menu-trigger"; }
    `;
    assert.equal(hasCompleteSelector(fixture, ".filter-menu-trigger"), false);
    assert.equal(hasCompleteSelector(fixture, ".search-bar"), true);
    assert.equal(hasCompleteSelector(fixture, '.search-bar[data-titlebar-search="true"]'), true);
  });

  it("keeps validators aligned with the shared titlebar overlay contract", () => {
    assert.match(webValidator, /for \(const rootId of \["notifications", "filter", "sort", "layout"\]\)/);
    assert.match(webValidator, /const shellNotificationOverlay = read\("apps\/web\/src\/app-shell\/useShellNotificationOverlay\.ts"\)/);
    assert.match(webValidator, /rootId === "notifications" \? shellNotificationOverlay : app/);
    assert.match(webValidator, /CompactPopover/);
    assert.match(uiValidator, /const layoutMenu = read\("apps\/web\/src\/features\/library\/LayoutMenu\.tsx"\)/);
    assert.match(uiValidator, /const overlayField = read\("apps\/web\/src\/design\/overlays\/OverlayField\.tsx"\)/);
    assert.match(uiValidator, /const overlayStyles = read\("apps\/web\/src\/design\/overlays\/overlay\.css"\)/);
    assert.match(uiValidator, /const shellNotificationOverlay = read\("apps\/web\/src\/app-shell\/useShellNotificationOverlay\.ts"\)/);
    assert.match(uiValidator, /rootId === "notifications" \? shellNotificationOverlay : app/);
    for (const value of ["AnchoredOverlay", "OverlayMenuList", "OverlayMenuItem", "OverlayField"]) {
      assert.match(uiValidator, new RegExp(value));
    }
    assert.match(uiValidator, /data-pointer-glow/);
    assert.doesNotMatch(webValidator, /shared compact-popover state/);
    assert.doesNotMatch(uiValidator, /compact popover coordination contract/);
    assert.doesNotMatch(uiValidator, /titlebar search must keep exactly one outer outline rule/);
    assert.doesNotMatch(uiValidator, /titlebar search must suppress the inner glass backdrop/);
  });

  it("uses comment/string-safe structural validator guards", () => {
    assert.match(webValidator, /function stripSourceCommentsAndStrings/);
    assert.match(uiValidator, /function stripSourceCommentsAndStrings/);
    assert.match(webValidator, /function sourceModuleSpecifiers/);
    assert.match(webValidator, /ts\.isImportDeclaration/);
    assert.match(webValidator, /isDynamicImport/);
    assert.match(webValidator, /isRequire/);
    assert.match(webValidator, /retiredOverlayImportPatterns/);
    assert.match(uiValidator, /stripSourceCommentsAndStrings\(source\)/);
    assert.doesNotMatch(webValidator, /const retiredOverlayImport = \/from\\s/);
    assert.doesNotMatch(
      uiValidator,
      /if \(\/createPortal\|LiquidGlassSurface\|useOverlayStack\|model\\\.results\/\.test\(notificationPopover\)\)/
    );
  });

  it("exercises the production notification titlebar wiring", () => {
    assert.match(app, /useShellNotificationOverlay/);
    assert.match(shellOverlayHost, /export function ShellNotificationOverlay/);
    assert.match(overlayFixture, /ShellRightActions/);
    assert.match(overlayFixture, /ShellNotificationOverlay/);
    assert.doesNotMatch(overlayFixture, /event\.detail\s*===\s*0|autoFocusRef/);
    assert.doesNotMatch(overlayFixture, /<NotificationPopover/);
  });

  it("checks local search hover rules in both shared stylesheets", () => {
    assert.match(uiValidator, /\["styles\.css", stylesForChecks\]/);
    assert.match(uiValidator, /\["overlay\.css", overlayStyles\]/);
  });
});

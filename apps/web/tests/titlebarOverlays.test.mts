import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const readRepo = (path: string) =>
  readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8");

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
      assert.match(overlayCss, new RegExp(selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
    }
    for (const selector of [
      ".search-bar",
      ".filter-menu-popover",
      ".sort-menu-list",
      ".layout-menu-options"
    ]) {
      assert.doesNotMatch(styles, new RegExp(`^${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?:[\\s,{:.#]|$)`, "m"));
    }

    const sectionOrder = [
      "/* Core variables and surfaces */",
      "/* Titlebar overlays */",
      "/* Library switcher */",
      "/* Context menus and file dialogs */",
      "/* Compact notification */",
      "/* Full notification center */",
      "/* Responsive, reduced motion, reduced transparency, and backdrop-filter fallback */"
    ].map((marker) => overlayCss.indexOf(marker));
    assert.ok(sectionOrder.every((index) => index >= 0));
    assert.ok(sectionOrder.every((index, position) => position === 0 || index > sectionOrder[position - 1]));
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

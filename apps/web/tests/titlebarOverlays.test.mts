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

  it("keeps search inline and pointer-glow-free", () => {
    assert.match(search, /OverlayField/);
    assert.match(search, /data-pointer-glow="off"/);
    assert.doesNotMatch(search, /AnchoredOverlay|createPortal/);
  });

  it("keeps validators aligned with the shared titlebar overlay contract", () => {
    assert.match(webValidator, /for \(const rootId of \["notifications", "filter", "sort", "layout"\]\)/);
    assert.match(webValidator, /CompactPopover/);
    assert.match(uiValidator, /const layoutMenu = read\("apps\/web\/src\/features\/library\/LayoutMenu\.tsx"\)/);
    assert.match(uiValidator, /const overlayStyles = read\("apps\/web\/src\/design\/overlays\/overlay\.css"\)/);
    for (const value of ["AnchoredOverlay", "OverlayMenuList", "OverlayMenuItem", "OverlayField"]) {
      assert.match(uiValidator, new RegExp(value));
    }
    assert.match(uiValidator, /data-pointer-glow/);
    assert.doesNotMatch(webValidator, /shared compact-popover state/);
    assert.doesNotMatch(uiValidator, /compact popover coordination contract/);
  });
});

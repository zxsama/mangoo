import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import ts from "typescript";

const sourcePath = new URL("../../apps/web/src/features/library/subfolderHierarchy.ts", import.meta.url);
const source = await readFile(sourcePath, "utf8");
const stripSourcePath = new URL("../../apps/web/src/features/library/SubfolderStrip.tsx", import.meta.url);
const stripSource = await readFile(stripSourcePath, "utf8");
const liquidGlassSurfacePath = new URL(
  "../../apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx",
  import.meta.url
);
const liquidGlassSurface = await readFile(liquidGlassSurfacePath, "utf8");
const settingsSourcePath = new URL("../../apps/web/src/features/settings/SettingsDialog.tsx", import.meta.url);
const settingsSource = await readFile(settingsSourcePath, "utf8");
const gridPreferencesSourcePath = new URL("../../apps/web/src/features/media-grid/gridPreferences.ts", import.meta.url);
const gridPreferencesSource = await readFile(gridPreferencesSourcePath, "utf8");
const mediaGridSourcePath = new URL("../../apps/web/src/features/media-grid/MediaGrid.tsx", import.meta.url);
const mediaGridSource = await readFile(mediaGridSourcePath, "utf8");
const stylesPath = new URL("../../apps/web/src/styles.css", import.meta.url);
const styles = await readFile(stylesPath, "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
    verbatimModuleSyntax: false
  }
});
const moduleUrl = `data:text/javascript;base64,${Buffer.from(compiled.outputText).toString("base64")}`;
const { buildVisibleSubfolderEntries } = await import(moduleUrl);

const folder = (id, parentId, name) => ({ id, parentId, name, rootId: 1, status: "active" });
const byParent = {
  1: [folder(10, 1, "A"), folder(20, 1, "B")],
  10: [folder(11, 10, "A-1"), folder(12, 10, "A-2")],
  11: [folder(111, 11, "A-1-a")],
  20: [folder(21, 20, "B-1")]
};

assert.deepEqual(
  buildVisibleSubfolderEntries({
    childFoldersByParentId: byParent,
    expandedFolderIds: new Set(),
    parentFolderId: 1,
    recursiveExpansionEnabled: true
  }).map((entry) => [entry.folder.id, entry.depth]),
  [
    [10, 0],
    [20, 0]
  ],
  "collapsed recursive mode should show only the first folder layer"
);

assert.deepEqual(
  buildVisibleSubfolderEntries({
    childFoldersByParentId: byParent,
    expandedFolderIds: new Set([10, 11]),
    parentFolderId: 1,
    recursiveExpansionEnabled: true
  }).map((entry) => [entry.folder.id, entry.depth]),
  [
    [10, 0],
    [11, 1],
    [111, 2],
    [12, 1],
    [20, 0]
  ],
  "expanded recursive mode should insert children directly after their parent"
);

assert.deepEqual(
  buildVisibleSubfolderEntries({
    childFoldersByParentId: byParent,
    expandedFolderIds: new Set([10]),
    parentFolderId: 1,
    recursiveExpansionEnabled: true
  }).map((entry) => [entry.folder.id, entry.depth]),
  [
    [10, 0],
    [11, 1],
    [12, 1],
    [20, 0]
  ],
  "recursive mode should not auto-open grandchildren until that folder is expanded"
);

assert.deepEqual(
  buildVisibleSubfolderEntries({
    childFoldersByParentId: byParent,
    expandedFolderIds: new Set([10]),
    parentFolderId: 1,
    recursiveExpansionEnabled: true
  })
    .filter((entry) => entry.depth === 1)
    .map((entry) => [entry.folder.id, entry.siblingPosition, entry.siblingIndex, entry.siblingCount]),
  [
    [11, "first", 0, 2],
    [12, "last", 1, 2]
  ],
  "expanded child layers should expose sibling group metadata for connected folder styling"
);

assert.deepEqual(
  buildVisibleSubfolderEntries({
    childFoldersByParentId: byParent,
    expandedFolderIds: new Set([10, 11]),
    parentFolderId: 1,
    recursiveExpansionEnabled: true
  })
    .filter((entry) => entry.depth > 0)
    .map((entry) => [entry.folder.id, entry.depth, entry.inheritedGroupPosition, entry.siblingPosition]),
  [
    [11, 1, "first", "first"],
    [111, 2, "middle", "single"],
    [12, 1, "last", "last"]
  ],
  "descendant folders should keep one inherited background group while preserving direct sibling groups"
);

assert.deepEqual(
  buildVisibleSubfolderEntries({
    childFoldersByParentId: byParent,
    expandedFolderIds: new Set([10]),
    parentFolderId: 1,
    recursiveExpansionEnabled: false
  }).map((entry) => [entry.folder.id, entry.depth]),
  [
    [10, 0],
    [20, 0]
  ],
  "non-recursive mode should ignore nested expansion state"
);

assert.match(
  styles,
  /\.subfolder-card\[data-nested="true"\]::before\s*\{[^}]*opacity:\s*1;/s,
  "only nested subfolder cards should paint the connected child-layer background"
);
assert.doesNotMatch(
  styles,
  /\.subfolder-card\[data-expanded-children="true"\]::before\s*,\s*\.subfolder-card\[data-nested="true"\]::before\s*\{[^}]*opacity:\s*1;/s,
  "expanded parent folder cards should not paint the child-layer background"
);
assert.match(
  styles,
  /\.subfolder-card\[data-inherited-position="first"\]::before,[^}]*\.subfolder-card\[data-inherited-position="single"\]::before\s*\{[^}]*border-top-left-radius/s,
  "only the first or single descendant in an inherited expanded group should round the outer left background edge"
);
assert.match(
  styles,
  /\.subfolder-card\[data-inherited-position="last"\]::before,[^}]*\.subfolder-card\[data-inherited-position="single"\]::before\s*\{[^}]*border-top-right-radius/s,
  "only the last or single descendant in an inherited expanded group should round the outer right background edge"
);
assert.doesNotMatch(
  styles,
  /\.folder-gridcell--row-start\s+\.subfolder-card(?:\[data-nested="true"\])?::before[^}]*border-top-left-radius/s,
  "wrapped child-layer row starts should stay square unless they are the first child in the group"
);
assert.doesNotMatch(
  styles,
  /\.folder-gridcell--row-end\s+\.subfolder-card(?:\[data-nested="true"\])?::before[^}]*border-top-right-radius/s,
  "wrapped child-layer row ends should stay square unless they are the last child in the group"
);
assert.match(
  styles,
  /\.subfolder-card::before,\s*\.subfolder-card::after\s*\{[^}]*--subfolder-edge-shadow-size:\s*72px;[^}]*--subfolder-edge-shadow-color:\s*rgba\(0, 0, 0, var\(--subfolder-edge-shadow-alpha, 0\.25\)\);[^}]*background-color:\s*transparent;[^}]*background-image:/s,
  "connected child-layer background should be transparent and paint edge shadows independently"
);
assert.doesNotMatch(
  styles,
  /\.subfolder-card::before,\s*\.subfolder-card::after\s*\{[^}]*box-shadow:/s,
  "connected child-layer background must not use box-shadow or filled panel shadow"
);
assert.match(
  styles,
  /\.subfolder-card\[data-inherited-position="first"\]::before,[^}]*\.subfolder-card\[data-inherited-position="single"\]::before\s*\{[^}]*--subfolder-edge-left-shadow:\s*linear-gradient\([^}]*to right,[^}]*var\(--subfolder-edge-shadow-color\),[^}]*var\(--subfolder-edge-shadow-clear\)/s,
  "only the first inherited child should paint the outer left edge shadow"
);
assert.match(
  styles,
  /\.subfolder-card\[data-inherited-position="last"\]::before,[^}]*\.subfolder-card\[data-inherited-position="single"\]::before\s*\{[^}]*--subfolder-edge-right-shadow:\s*linear-gradient\([^}]*to left,[^}]*var\(--subfolder-edge-shadow-color\),[^}]*var\(--subfolder-edge-shadow-clear\)/s,
  "only the last inherited child should paint the outer right edge shadow"
);
assert.doesNotMatch(
  styles,
  /\.folder-gridcell--row-(?:start|end)\s+\.subfolder-card(?:\[data-nested="true"\])?::before[^}]*--subfolder-edge-(?:left|right)-shadow/s,
  "wrapped child-layer row starts and ends must not create side edge shadows"
);
assert.match(
  styles,
  /--subfolder-nested-cover-height:\s*calc\(\s*var\(--subfolder-cover-height,\s*0px\)\s*\*\s*var\(--subfolder-nested-cover-scale,\s*1\)\s*\)/,
  "nested subfolder covers should scale by 90% for each depth level"
);
assert.match(
  styles,
  /--subfolder-parent-cover-height:\s*calc\(\s*var\(--subfolder-cover-height,\s*0px\)\s*\*\s*var\(--subfolder-parent-cover-scale,\s*1\)\s*\)/,
  "child-layer backgrounds should derive their height from the expanded parent folder visual scale"
);
assert.match(
  styles,
  /--subfolder-nested-background-inset-y:\s*calc\(\s*\(100% - var\(--subfolder-parent-body-height\)\) \/ 2\s*\)/,
  "child-layer backgrounds should align vertically to their immediate parent folder body"
);
assert.match(
  styles,
  /--subfolder-card-body-height:\s*calc\(\s*var\(--subfolder-cover-height,\s*0px\)\s*\+\s*var\(--subfolder-label-height,\s*0px\)\s*\)/,
  "folder cards should define one visual body height from cover plus name slot"
);
assert.match(
  styles,
  /--subfolder-background-inset-y:\s*calc\(\(100% - var\(--subfolder-card-body-height\)\) \/ 2\)/,
  "connected child-layer background should align to the folder visual body instead of the whole row cell"
);
assert.match(
  stripSource,
  /"--subfolder-parent-cover-scale":\s*parentCoverScale\.toFixed\(4\)/,
  "SubfolderCard should expose each nested folder's parent visual scale for child-layer alignment"
);
assert.match(
  styles,
  /\.subfolder-card::before\s*\{[^}]*top:\s*var\(--subfolder-background-inset-y\);[^}]*bottom:\s*var\(--subfolder-background-inset-y\);/s,
  "outer inherited background should align vertically with the parent folder visual body"
);
assert.match(
  styles,
  /\.subfolder-card::after\s*\{[^}]*top:\s*var\(--subfolder-nested-background-inset-y\);[^}]*bottom:\s*var\(--subfolder-nested-background-inset-y\);/s,
  "inner nested background should align to the expanded parent folder visual body"
);
assert.match(
  styles,
  /\.subfolder-card\[data-nested-layer="true"\]::after\s*\{[^}]*opacity:\s*1;/s,
  "only depth 2+ folders should paint the inner nested background layer"
);
assert.match(
  styles,
  /\.subfolder-card-main\s*\{[^}]*grid-template-rows:\s*var\(--subfolder-cover-height, auto\) var\(--subfolder-label-height, auto\);[^}]*height:\s*var\(--subfolder-card-body-height\);[^}]*padding:\s*0;/s,
  "root folder button body should start exactly at the cover edge and reserve name spacing through a row"
);
assert.match(
  styles,
  /\.subfolder-card\[data-nested="true"\] \.subfolder-card-main\s*\{[^}]*grid-template-rows:\s*var\(--subfolder-nested-cover-height\) var\(--subfolder-label-height, auto\);[^}]*height:\s*var\(--subfolder-nested-body-height\)/s,
  "nested cards should center their smaller cover and name slot inside the inherited background"
);
assert.match(
  stripSource,
  /className="subfolder-card-main"[\s\S]*data-interactive-pointer-target-selector="\.subfolder-card-pointer-surface"/,
  "subfolder cover buttons should route the global pointer border to the clipped thumbnail surface instead of the full button box"
);
assert.match(
  stripSource,
  /className="subfolder-card-cover"[\s\S]*className="subfolder-card-pointer-surface"/,
  "subfolder covers should include a dedicated pointer highlight surface so hover outline does not overwrite selected-cover styling"
);
assert.doesNotMatch(
  stripSource,
  /subfolder-card-main[\s\S]{0,240}data-interactive-pointer-disabled/,
  "subfolder cover buttons must keep hover pointer feedback instead of disabling the global pointer border"
);
assert.match(
  liquidGlassSurface,
  /INTERACTIVE_POINTER_TARGET_SELECTOR_ATTRIBUTE[\s\S]*data-interactive-pointer-target-selector[\s\S]*interactiveAffordanceVisualTarget[\s\S]*querySelector<HTMLElement>/,
  "interactive pointer routing should support explicit visual targets for controls whose hit box differs from their painted surface"
);
assert.match(
  styles,
  /\.subfolder-card-pointer-surface\s*\{[^}]*position:\s*absolute;[^}]*inset:\s*0;[^}]*z-index:\s*1;[^}]*border-radius:\s*inherit;[^}]*pointer-events:\s*none;/s,
  "subfolder pointer surface should sit exactly over the clipped cover and inherit stitched corner radii"
);
assert.match(
  mediaGridSource,
  /"--subfolder-label-height":\s*`\$\{gridPreferences\.folderTileLabelHeight\}px`/,
  "MediaGrid should drive folder name spacing independently from content name spacing"
);
assert.match(
  mediaGridSource,
  /"--subfolder-tile-gap":\s*`\$\{gridPreferences\.folderTileGap\}px`/,
  "MediaGrid should drive folder thumbnail gaps independently from content thumbnail gaps"
);
assert.match(
  mediaGridSource,
  /"--subfolder-edge-shadow-alpha":\s*`\$\{gridPreferences\.folderEdgeShadowAlpha \/ 100\}`/,
  "MediaGrid should drive folder edge shadow alpha from settings"
);
assert.match(
  gridPreferencesSource,
  /folderEdgeShadowAlpha:\s*25[^]*?folderTileGap:\s*clampPreference\([^]*?tileGap[^]*?folderTileLabelHeight:\s*clampPreference\([^]*?tileLabelHeight/s,
  "stored preferences without folder-specific values should inherit the existing content spacing"
);
assert.match(
  settingsSource,
  /文件夹网格[\s\S]*library-folder-grid-gap[\s\S]*folderTileGap[\s\S]*library-folder-grid-label-height[\s\S]*folderTileLabelHeight[\s\S]*library-folder-edge-shadow-alpha[\s\S]*folderEdgeShadowAlpha/,
  "Settings should expose separate folder-area thumbnail, name spacing, and edge shadow sliders"
);
assert.match(
  styles,
  /\.subfolder-card\s*\{[^}]*--subfolder-cover-radius-top-left:\s*10px;[^}]*--subfolder-cover-radius-bottom-left:\s*10px;/s,
  "subfolder covers should use shared radius variables so image clipping and selection highlight stay aligned"
);
assert.match(
  styles,
  /\.subfolder-card-cover\s*\{[^}]*border-radius:\s*var\(--subfolder-cover-radius-top-left\)\s*var\(--subfolder-cover-radius-top-right\)\s*var\(--subfolder-cover-radius-bottom-right\)\s*var\(--subfolder-cover-radius-bottom-left\);/s,
  "subfolder cover clipping should use the shared radius variables"
);
assert.match(
  styles,
  /\.library-thumbnail-interaction-ring\s*\{[^}]*box-sizing:\s*border-box;[^}]*border:\s*var\(--library-thumbnail-idle-ring\);[^}]*border-radius:\s*inherit;[^}]*box-shadow:\s*none;/s,
  "selected subfolder highlight should use the same shared interaction ring as content thumbnails"
);
assert.match(
  styles,
  /\.subfolder-card\[data-nested="true"\]\s*\{[^}]*--subfolder-cover-radius-top-left:\s*0px;[^}]*--subfolder-cover-radius-bottom-left:\s*0px;/s,
  "nested folder covers should reset standalone corner radii before row and group edge rules are applied"
);
assert.match(
  styles,
  /\.subfolder-card\[data-nested-position="first"\],\s*\.subfolder-card\[data-nested-position="single"\]\s*\{[^}]*--subfolder-cover-radius-top-left:\s*10px;[^}]*--subfolder-cover-radius-bottom-left:\s*10px;/s,
  "only the first or single nested folder cover should round the stitched group left edge"
);
assert.match(
  styles,
  /\.subfolder-card\[data-nested-position="last"\],\s*\.subfolder-card\[data-nested-position="single"\]\s*\{[^}]*--subfolder-cover-radius-top-right:\s*10px;[^}]*--subfolder-cover-radius-bottom-right:\s*10px;/s,
  "only the last or single nested folder cover should round the stitched group right edge"
);
assert.doesNotMatch(
  styles,
  /\.folder-gridcell--row-start\s+\.subfolder-card\[data-nested="true"\]\s*\{[^}]*--subfolder-cover-radius-(?:top|bottom)-left/s,
  "wrapped row starts must not add left cover radius inside a stitched child-folder group"
);
assert.doesNotMatch(
  styles,
  /\.folder-gridcell--row-end\s+\.subfolder-card\[data-nested="true"\]\s*\{[^}]*--subfolder-cover-radius-(?:top|bottom)-right/s,
  "wrapped row ends must not add right cover radius inside a stitched child-folder group"
);
assert.match(
  styles,
  /\.subfolder-card\.selected\s+\.library-thumbnail-interaction-ring\s*\{[^}]*border:\s*var\(--library-thumbnail-selected-ring\);/s,
  "selected subfolder state should reveal the shared thumbnail interaction ring instead of styling a separate image frame"
);
assert.match(
  styles,
  /\.subfolder-card\.selected\s+\.subfolder-card-cover\s*\{[^}]*z-index:\s*3;[^}]*opacity:\s*1;/s,
  "selected subfolder cover should paint above adjacent stitched folder covers"
);
assert.match(
  styles,
  /\.subfolder-card-cover-image-frame\s*\{[^}]*border-radius:\s*inherit;/s,
  "ready folder thumbnails should inherit the same radius as the selection highlight"
);
assert.match(
  styles,
  /\.subfolder-card-cover-fallback\s*\{[^}]*border-radius:\s*inherit;/s,
  "fallback folder thumbnails should inherit the same radius as the selection highlight"
);

console.log("PASS: subfolder hierarchy");

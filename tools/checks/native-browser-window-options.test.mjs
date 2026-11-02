import assert from "node:assert/strict";
import { inspectNativeBrowserWindowOptions } from "./native-browser-window-options.mjs";

const commentAndStringOnly = `
  const deadString = 'new BrowserWindow({ backgroundMaterial: "acrylic", transparent: false, backgroundColor: "#00000000", roundedCorners: true, frame: false })';
  // new BrowserWindow({ backgroundMaterial: "acrylic", transparent: false, backgroundColor: "#00000000", roundedCorners: true, frame: false })
  /*
    new BrowserWindow({ backgroundMaterial: "acrylic", transparent: false, backgroundColor: "#00000000", roundedCorners: true, frame: false })
  */
  const win = new BrowserWindow({
    width: 1200,
    backgroundMaterial: "none",
    transparent: true,
    backgroundColor: "#11111111",
    roundedCorners: false,
    frame: true,
    webPreferences: {
      preload: "backgroundMaterial: \\"acrylic\\""
    }
  });
`;

const rejected = inspectNativeBrowserWindowOptions(commentAndStringOnly);
assert.equal(rejected.backgroundMaterial, "none");
assert.equal(rejected.backgroundMaterialSource, 'backgroundMaterial: "none"');
assert.equal(rejected.frameFalse, false);
assert.equal(rejected.transparent, true);
assert.equal(rejected.nonLayeredHost, false);
assert.equal(rejected.transparentBackgroundColor, false);
assert.equal(rejected.roundedCorners, false);
assert.equal(rejected.disablesNativeMaterial, true);
assert.deepEqual(rejected.missingRequiredProperties, [
  'backgroundMaterial: "acrylic"',
  "transparent: false",
  'backgroundColor: "#00000000"',
  "roundedCorners: true",
  "frame: false"
]);

const valid = inspectNativeBrowserWindowOptions(`
  const safeName = input.replace(/[<>:"/\\\\|?*\\u0000-\\u001f]/g, " ");
  const win = new BrowserWindow({
    title: "literal with frame: true",
    frame: false,
    transparent: false,
    backgroundColor: "#00000000",
    roundedCorners: true,
    backgroundMaterial: "acrylic",
    webPreferences: {
      preload: "backgroundMaterial: \\"acrylic\\""
    }
  });
`);

assert.equal(valid.backgroundMaterial, "acrylic");
assert.equal(valid.backgroundMaterialSource, 'backgroundMaterial: "acrylic"');
assert.equal(valid.frameFalse, true);
assert.equal(valid.transparent, false);
assert.equal(valid.nonLayeredHost, true);
assert.equal(valid.transparentBackgroundColor, true);
assert.equal(valid.roundedCorners, true);
assert.equal(valid.disablesNativeMaterial, false);
assert.deepEqual(valid.missingRequiredProperties, []);

const notFound = inspectNativeBrowserWindowOptions("const value = 1;");
assert.equal(notFound.browserWindowOptionsFound, false);
assert.deepEqual(notFound.missingRequiredProperties, [
  'backgroundMaterial: "acrylic"',
  "transparent: false",
  'backgroundColor: "#00000000"',
  "roundedCorners: true",
  "frame: false"
]);

const topLevelSpread = inspectNativeBrowserWindowOptions(`
  const win = new BrowserWindow({
    ...windowOptions,
    frame: false,
    transparent: false,
    backgroundColor: "#00000000",
    roundedCorners: true,
    backgroundMaterial: "acrylic",
    webPreferences: {
      preload: preloadPath
    }
  });
`);

assert.equal(topLevelSpread.browserWindowOptionsFound, true);
assert.equal(topLevelSpread.frameFalse, true);
assert.equal(topLevelSpread.transparent, false);
assert.equal(topLevelSpread.nonLayeredHost, true);
assert.equal(topLevelSpread.transparentBackgroundColor, true);
assert.equal(topLevelSpread.roundedCorners, true);
assert.equal(topLevelSpread.backgroundMaterial, "acrylic");
assert.deepEqual(topLevelSpread.unsafeTopLevelSpreads.map((spread) => spread.source), [
  "...windowOptions"
]);
assert.deepEqual(topLevelSpread.missingRequiredProperties, [
  "no top-level spread in BrowserWindow options"
]);

const multipleWindows = inspectNativeBrowserWindowOptions(`
  const main = new BrowserWindow({
    frame: false,
    transparent: false,
    backgroundColor: "#00000000",
    roundedCorners: true,
    backgroundMaterial: "acrylic"
  });
  const utility = new BrowserWindow({
    frame: true,
    transparent: true,
    backgroundColor: "#111111",
    roundedCorners: false,
    backgroundMaterial: "acrylic"
  });
`);

assert.equal(multipleWindows.browserWindowCount, 2);
assert.equal(multipleWindows.backgroundMaterial, "acrylic");
assert.equal(multipleWindows.frameFalse, false);
assert.equal(multipleWindows.transparent, false);
assert.equal(multipleWindows.nonLayeredHost, false);
assert.equal(multipleWindows.transparentBackgroundColor, false);
assert.equal(multipleWindows.roundedCorners, false);
assert.equal(multipleWindows.disablesNativeMaterial, false);
assert.deepEqual(multipleWindows.missingRequiredProperties, [
  "transparent: false",
  'backgroundColor: "#00000000"',
  "roundedCorners: true",
  "frame: false"
]);

const nestedSpread = inspectNativeBrowserWindowOptions(`
  const win = new BrowserWindow({
    frame: false,
    transparent: false,
    backgroundColor: "#00000000",
    roundedCorners: true,
    backgroundMaterial: "acrylic",
    webPreferences: {
      ...webPreferences,
      preload: preloadPath
    }
  });
`);

assert.equal(nestedSpread.browserWindowOptionsFound, true);
assert.deepEqual(nestedSpread.unsafeTopLevelSpreads, []);
assert.deepEqual(nestedSpread.missingRequiredProperties, []);

function windowWithOptionalIcon(spread) {
  return inspectNativeBrowserWindowOptions(`new BrowserWindow({
    frame: false, transparent: false, backgroundColor: "#00000000",
    roundedCorners: true, backgroundMaterial: "acrylic", ${spread}
  })`);
}

const optionalIcon = windowWithOptionalIcon("...(windowIcon ? { icon: windowIcon } : {})");
assert.deepEqual(optionalIcon.missingRequiredProperties, []);
assert.deepEqual(optionalIcon.unsafeTopLevelSpreads, []);
for (const spread of [
  "...(windowIcon ? { icon: windowIcon, transparent: true } : {})",
  '...(windowIcon ? { icon: windowIcon } : { backgroundMaterial: "none" })',
  "...(windowIcon ? { icon: windowIcon, ...otherOptions } : {})",
  "...(windowIcon ? otherOptions : {})",
  "...getWindowOptions()"
]) {
  assert.equal(windowWithOptionalIcon(spread).unsafeTopLevelSpreads.length, 1, spread);
}

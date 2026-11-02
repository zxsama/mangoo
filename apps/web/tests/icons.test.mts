import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { test } from "node:test";
import { build } from "esbuild";
import { createElement, createRef, type ReactElement, type Ref } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { AppIcon, AppIconProps } from "../src/design/icons.ts";

// Exercise the real TSX and package glyphs without a desktop process.
const scratch = fileURLToPath(new URL("../../../.tmp/icon-tests/", import.meta.url));
await mkdir(scratch, { recursive: true });
const outdir = await mkdtemp(`${scratch}run-`);
await build({
  stdin: {
    contents: 'export * from "./features/library/libraryGlyph"; export { OverlayProvider } from "./design/overlays"; export * as icons from "./design/icons";',
    resolveDir: fileURLToPath(new URL("../src/", import.meta.url)),
    loader: "ts",
  },
  outfile: `${outdir}/components.js`,
  bundle: true,
  platform: "node",
  format: "esm",
  packages: "external",
  jsx: "automatic",
});
const library: typeof import("../src/features/library/libraryGlyph.tsx") & {
  icons: typeof import("../src/design/icons.ts");
  OverlayProvider: typeof import("../src/design/overlays/index.ts").OverlayProvider;
} = await import(pathToFileURL(`${outdir}/components.js`).href);
const { icons } = library;
const renderIcon = (Icon: AppIcon, props: AppIconProps = {}) => renderToStaticMarkup(createElement(Icon, props));

test("project icons retain their box, currentColor, SVG props and forwarded ref", () => {
  const markup = renderIcon(icons.Check, { "aria-hidden": true, className: "control-icon", style: { opacity: 0.8 } });
  assert.match(markup, /width="24" height="24"/);
  assert.match(markup, /fill="currentColor"/);
  assert.match(markup, /viewBox="0 0 256 256"/);
  assert.match(markup, /aria-hidden="true"/);
  assert.match(markup, /class="control-icon"/);
  assert.match(markup, /opacity:0.8/);
  assert.match(renderIcon(icons.Check, { size: 16, color: "#123456", "aria-label": "Complete" }), /width="16" height="16" fill="#123456"/);
  const ref = createRef<SVGSVGElement>();
  const adapter = icons.Check as AppIcon & { render: (props: AppIconProps, ref: Ref<SVGSVGElement>) => ReactElement };
  const element = adapter.render({ size: 16 }, ref) as ReactElement & { ref: Ref<SVGSVGElement> };
  assert.equal(element.ref, ref);
});

test("every curated icon renders and layout modes have distinct artwork", (context) => {
  for (const [name, Icon] of Object.entries(icons)) {
    const markup = renderIcon(Icon);
    assert.match(markup, /<(?:path|circle|rect|polygon|polyline|line)\b/, name);
    assert.doesNotMatch(markup, /(?:NaN|undefined|lucide)/, name);
  }
  const layouts = [icons.LayoutPanelTop, icons.GalleryVerticalEnd, icons.LayoutGrid, icons.LayoutList];
  assert.equal(new Set(layouts.map((Icon) => renderIcon(Icon))).size, 4);
  for (const Icon of [icons.Heart, icons.Star]) {
    assert.notEqual(renderIcon(Icon), renderIcon(Icon, { weight: "fill" }));
    assert.match(renderIcon(Icon, { weight: "fill" }), /fill="currentColor"/);
  }
  assert.notEqual(renderIcon(icons.File), renderIcon(icons.File, { weight: "light" }));
  assert.match(renderIcon(icons.MoreHorizontal), /viewBox="0 0 256 256"/);
  context.diagnostic(`Validated ${Object.keys(icons).length} curated glyphs`);
});

test("Library appearance keeps stored identities, fallback and disabled selection semantics", () => {
  assert.deepEqual(library.LIBRARY_ICONS.slice(0, 16).map(({ key }) => key), [
    "library", "image", "camera", "film", "video", "music", "palette", "aperture",
    "layers", "archive", "box", "bookmark", "star", "heart", "folder-heart", "sparkles",
  ]);
  assert.deepEqual(library.LIBRARY_COLORS, [
    "#ffffff", "#7cc4ff", "#5ce1a6", "#ffd166", "#ff8c69", "#f06595", "#b197fc", "#4dd0e1", "#a3e635", "#ff922b", "#e5e7eb",
  ]);
  assert.equal(library.DEFAULT_LIBRARY_ICON, "library");
  assert.equal(library.DEFAULT_LIBRARY_COLOR, "#ffffff");
  assert.match(renderToStaticMarkup(createElement(library.LibraryGlyph)), /style="color:#ffffff"/);
  assert.match(renderToStaticMarkup(createElement(library.LibraryGlyph, { color: "#7cc4ff" })), /style="color:#7cc4ff"/);
  assert.equal(library.resolveLibraryIcon(), library.resolveLibraryIcon("unrecognized"));
  assert.equal(library.resolveLibraryIcon("library"), library.resolveLibraryIcon(""));
  {
    const markup = renderToStaticMarkup(createElement(library.OverlayProvider, null, createElement(library.LibraryIconPicker, {
      disabled: true, icon: "heart", color: "#ffd166", onPick: () => assert.fail("render must not select"),
    })));
    const count = library.LIBRARY_ICONS.length + library.LIBRARY_COLORS.length;
    assert.ok(library.LIBRARY_ICONS.length >= 48);
    assert.equal((markup.match(/<button\b/g) ?? []).length, count);
    assert.equal((markup.match(/<button[^>]* disabled=""/g) ?? []).length, count);
    assert.match(markup, /data-identity-variant="symbol"/);
    assert.match(markup, /style="color:currentColor"/);
    assert.equal((markup.match(/role="radiogroup"/g) ?? []).length, 2);
    assert.equal((markup.match(/role="radio"/g) ?? []).length, count);
    assert.equal((markup.match(/aria-checked="true"/g) ?? []).length, 2);
    assert.equal((markup.match(/tabindex="0"/g) ?? []).length, 2);
    assert.equal((markup.match(/tabindex="-1"/g) ?? []).length, count - 2);
    assert.equal((markup.match(/<svg[^>]*data-identity-variant="symbol"/g) ?? []).length, library.LIBRARY_ICONS.length);
    assert.equal((markup.match(/<svg[^>]*class="library-icon-picker-check"/g) ?? []).length, 1);
    // Appearance is identity artwork, rather than a selection-dependent command glyph.
    assert.equal(renderToStaticMarkup(createElement(library.LibraryGlyph, { icon: "heart", selected: true })),
      renderToStaticMarkup(createElement(library.LibraryGlyph, { icon: "heart", selected: false })));

  }
});

test("Library swatch checks contrast with dark and light saved colors", () => {
  assert.equal(library.libraryColorCheckColor("#123456"), "#ffffff");
  assert.equal(library.libraryColorCheckColor("#000"), "#ffffff");
  assert.equal(library.libraryColorCheckColor("#ffffff"), "#18202b");
  assert.equal(library.libraryColorCheckColor("#ffd166"), "#18202b");
});

test("icon dependencies and central-preview guard remain bounded", async () => {
  const source = await readFile(new URL("../src/design/icons.ts", import.meta.url), "utf8");
  const manifest = await readFile(new URL("../package.json", import.meta.url), "utf8");
  const validator = await readFile(new URL("../../../tools/checks/validate-ui-design.mjs", import.meta.url), "utf8");
  assert.doesNotMatch(source, /import\s+\*|from\s+["']@phosphor-icons\/react["']/);
  assert.equal(JSON.parse(manifest).dependencies["@phosphor-icons/react"], "2.1.10");
  assert.doesNotMatch(manifest, /lucide-react/);
  for (const guard of ["lucide-react", "@phosphor-icons/react", "design/icons"]) assert.ok(validator.includes(`"${guard}"`));
});

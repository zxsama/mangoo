import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { test } from "node:test";
import { build } from "esbuild";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const scratch = fileURLToPath(new URL("../../../.tmp/identity-artwork-tests/", import.meta.url));
await mkdir(scratch, { recursive: true });
const outdir = await mkdtemp(`${scratch}run-`);
await build({
  stdin: {
    contents: 'export * from "./identityArtwork"; export { IDENTITY_FLUENT_PATHS } from "./identityFluentPaths"; export { IDENTITY_TYPOGRAPHIC_PATHS } from "./identityTypographicPaths"; export { FolderGlyph } from "./FolderGlyph";',
    resolveDir: fileURLToPath(new URL("../src/features/library/", import.meta.url)), loader: "ts",
  },
  outfile: `${outdir}/artwork.js`, bundle: true, platform: "node", format: "esm", packages: "external", jsx: "automatic",
});
const artwork: typeof import("../src/features/library/identityArtwork.tsx") & typeof import("../src/features/library/identityFluentPaths.ts") & typeof import("../src/features/library/identityTypographicPaths.ts") & typeof import("../src/features/library/FolderGlyph.tsx") = await import(pathToFileURL(`${outdir}/artwork.js`).href);
const render = (props: Parameters<typeof artwork.IdentityArtwork>[0] = {}) => renderToStaticMarkup(createElement(artwork.IdentityArtwork, props));

test("identity catalog has unique stable keys, useful categories and bilingual search", () => {
  const { IDENTITY_ICONS: icons, IDENTITY_CATEGORIES: categories, matchesIdentityIcon: matches } = artwork;
  assert.equal(icons.length, 502);
  assert.deepEqual(categories.map(({ key }) => key), [
    "collections", "media", "creative", "work", "learning", "people", "home",
    "food", "sports", "animals", "nature", "places", "travel", "technology", "numbers", "letters-uppercase", "letters-lowercase", "roman-uppercase", "roman-lowercase", "chinese-numerals",
  ]);
  assert.equal(new Set(icons.map(({ key }) => key)).size, icons.length);
  assert.equal(new Set(icons.map(({ label }) => label)).size, icons.length);
  assert.deepEqual(icons.slice(0, 16).map(({ key }) => key), [
    "library", "image", "camera", "film", "video", "music", "palette", "aperture",
    "layers", "archive", "box", "bookmark", "star", "heart", "folder-heart", "sparkles",
  ]);
  for (const icon of icons) {
    assert.ok(categories.some(({ key }) => key === icon.category), icon.key);
    assert.match(icon.label, /[\u4e00-\u9fff]/u);
    assert.ok(matches(icon, icon.label));
    assert.ok(matches(icon, icon.keywords.split(" ")[0].toUpperCase()));
    assert.ok(matches(icon, "  "));
    assert.equal(artwork.resolveIdentityIcon(icon.key), icon);
  }
  assert.equal(artwork.resolveIdentityIcon("unrecognized-but-persisted"), icons[0]);
  assert.equal(artwork.resolveIdentityIcon(), icons[0]);
  assert.ok(matches(artwork.resolveIdentityIcon("camera"), "photo camera"));
  assert.ok(!matches(artwork.resolveIdentityIcon("camera"), "camera database"));
  for (const category of categories) {
    assert.ok(icons.filter((icon) => icon.category === category.key).length >= 12, category.key);
  }
  for (const [query, key] of [["露营", "tent"], ["PODCAST", "microphone"], ["robot automation", "robot"], ["打印", "printer"], ["化学", "molecule"], ["VIRTUAL REALITY", "vr-headset"], ["棋类", "chess"], ["furniture workspace", "desk"]]) {
    assert.ok(icons.filter((icon) => matches(icon, query)).some((icon) => icon.key === key), query);
  }
  for (const [key, category] of [
    ["food", "food"], ["home", "home"], ["person", "people"], ["graduation", "learning"],
    ["games", "sports"], ["cat", "animals"], ["sun", "nature"], ["building", "places"],
    ["airplane", "travel"], ["molecule", "learning"], ["couch", "home"], ["chess", "sports"],
  ]) {
    assert.equal(artwork.resolveIdentityIcon(key).category, category, key);
  }
});

test("catalog expansion preserves all 66 prior identities and adds distinct complete path pairs", () => {
  const legacyKeys = [
    "library", "image", "camera", "film", "video", "music", "palette", "aperture", "layers", "archive", "box", "bookmark", "star", "heart", "folder-heart", "sparkles",
    "folder", "book", "book-open", "notebook", "paint-brush", "document", "document-text", "document-pdf", "clipboard", "briefcase", "calendar", "clock", "home", "building",
    "people", "person", "cat", "dog", "leaf", "tree", "plant", "food", "coffee", "airplane", "car", "bicycle", "beach", "globe", "map", "location", "sun", "moon", "snowflake", "cloud",
    "desktop", "laptop", "code", "database", "hard-drive", "games", "football", "trophy", "graduation", "beaker", "lightbulb", "puzzle", "shopping-bag", "gift", "ribbon", "shield",
  ];
  assert.deepEqual(artwork.IDENTITY_ICONS.slice(0, legacyKeys.length).map(({ key }) => key), legacyKeys);
  const paths = artwork.IDENTITY_FLUENT_PATHS;
  assert.deepEqual(Object.keys(paths), artwork.IDENTITY_ICONS.filter((icon) => artwork.resolveIdentityIcon(icon.key).sourceKind !== "typographic").map(({ key }) => key));
  // Snapshot of both upstream path variants before the picker expansion.
  const legacyGeometry = Object.fromEntries(legacyKeys.map((key) => [key, paths[key]]));
  assert.equal(createHash("sha256").update(JSON.stringify(legacyGeometry)).digest("hex"), "3525ac14fb1f921e9c6f57ee8729dda539287d4ba5ef5784a0746338fb69472c");
  const seen = new Set(legacyKeys.map((key) => JSON.stringify(paths[key].filled)));
  for (const { key } of artwork.IDENTITY_ICONS.slice(legacyKeys.length).filter((icon) => artwork.resolveIdentityIcon(icon.key).sourceKind !== "typographic")) {
    const pair = paths[key];
    assert.ok(pair.filled.length > 0 && pair.regular.length > 0, key);
    assert.notDeepEqual(pair.filled, pair.regular, key);
    const geometry = JSON.stringify(pair.filled);
    assert.ok(!seen.has(geometry), `${key} must not be an alias for existing geometry`);
    seen.add(geometry);
  }
});

test("type grouping preserves all 144 earlier identities and adds distinct bilingual choices", () => {
  const prior = artwork.IDENTITY_ICONS.slice(0, 144);
  const metadata = prior.map(({ category: _category, ...identity }) => identity);
  const geometry = Object.fromEntries(prior.map(({ key }) => [key, artwork.IDENTITY_FLUENT_PATHS[key]]));
  assert.equal(createHash("sha256").update(JSON.stringify(metadata)).digest("hex"), "a4854d6a4743f857c6ebc0b76d05fe16d30ddb2047f7152a858023744258d094");
  assert.equal(createHash("sha256").update(JSON.stringify(geometry)).digest("hex"), "aaaa6eb940d7a88dff0c65c4d136c264ddc405a2e5ee56228d03691bb69a7eff");
  const sources = new Set(prior.map(({ source }) => source));
  for (const icon of artwork.IDENTITY_ICONS.slice(144).filter((icon) => artwork.resolveIdentityIcon(icon.key).sourceKind !== "typographic")) {
    assert.ok(!sources.has(icon.source), icon.key);
    sources.add(icon.source);
    assert.match(icon.keywords, /[a-z]/i, icon.key);
    assert.match(icon.keywords, /[\u4e00-\u9fff]/u, icon.key);
  }
  for (const variant of ["filled", "regular"] as const) {
    const seen = new Set(prior.map(({ key }) => JSON.stringify(artwork.IDENTITY_FLUENT_PATHS[key][variant])));
    for (const { key } of artwork.IDENTITY_ICONS.slice(144).filter((icon) => artwork.resolveIdentityIcon(icon.key).sourceKind !== "typographic")) {
      const paths = artwork.IDENTITY_FLUENT_PATHS[key][variant];
      assert.ok(paths.length > 0, key);
      const serialized = JSON.stringify(paths);
      assert.ok(!seen.has(serialized), `${key} must have distinct ${variant} geometry`);
      seen.add(serialized);
    }
  }
});

test("further expansion retains all 256 saved identities and grows every existing type", () => {
  const prior = artwork.IDENTITY_ICONS.slice(0, 256);
  const geometry = Object.fromEntries(prior.map(({ key }) => [key, artwork.IDENTITY_FLUENT_PATHS[key]]));
  assert.equal(createHash("sha256").update(JSON.stringify(prior)).digest("hex"), "2bde9afd19714690db7e097b0734ce2a136223a7c929cfec26211e3c48249d1f");
  assert.equal(createHash("sha256").update(JSON.stringify(geometry)).digest("hex"), "dd8ca113d75ea0a5455737a68a09a48b4bbbf3f63ae453565509502f6d09bed3");
  for (const { key } of artwork.IDENTITY_CATEGORIES) {
    assert.ok(artwork.IDENTITY_ICONS.slice(256).some((icon) => icon.category === key), key);
  }
  for (const [query, key] of [["BARCODE", "tech-barcode"], ["生态", "plant-earth"], ["甘特图", "work-gantt"], ["takeaway drink", "food-takeaway"]]) {
    assert.ok(artwork.matchesIdentityIcon(artwork.resolveIdentityIcon(key), query), query);
  }
});

test("numbers offer a complete official decimal circle series and searchable numbering choices", () => {
  const numbers = artwork.IDENTITY_ICONS.filter((icon) => icon.category === "numbers");
  assert.equal(numbers.length, 44);
  const digits = numbers.slice(0, 10);
  const chinese = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
  const english = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
  for (let value = 0; value < 10; value += 1) {
    const icon = digits[value];
    assert.equal(icon.key, `number-circle-${value}`);
    assert.equal(icon.source, `Number Circle ${value}`);
    for (const query of [`圈号 ${value}`, `digit ${value}`, `number ${english[value]}`, chinese[value], "数字 编号 圆圈"]) {
      assert.ok(artwork.matchesIdentityIcon(icon, query), `${icon.key}: ${query}`);
    }
    assert.notDeepEqual(artwork.IDENTITY_FLUENT_PATHS[icon.key].filled, artwork.IDENTITY_FLUENT_PATHS[icon.key].regular);
  }
  // All twenty official 24px SVG variants were independently downloaded from the pinned commit.
  const geometry = Object.fromEntries(digits.map(({ key }) => [key, artwork.IDENTITY_FLUENT_PATHS[key]]));
  assert.equal(createHash("sha256").update(JSON.stringify(geometry)).digest("hex"), "45a213149bdc591d340ca854aace6cab2a7bac1f4f90c166b49bbe08377b0cfb");
  for (const [query, key] of [["罗马 大写", "roman-uppercase"], ["sequence order", "sequence-steps"], ["页码", "page-number"], ["123", "clipboard-123"], ["字母 小写", "alphabet-lowercase"], ["计数", "number-counter"]]) {
    assert.ok(numbers.some((icon) => icon.key === key && artwork.matchesIdentityIcon(icon, query)), query);
  }
});

test("all identities render locally at compact sizes without IDs, network assets or control frames", () => {
  for (const { key } of artwork.IDENTITY_ICONS) {
    for (const size of [16, 18, 20, 24, 36]) {
      for (const variant of ["library", "symbol"] as const) {
        const svg = render({ icon: key, size, variant });
        assert.match(svg, new RegExp(`width="${size}" height="${size}"`));
        assert.match(svg, /viewBox="0 0 24 24"/);
        assert.match(svg, /aria-hidden="true" focusable="false"/);
        assert.match(svg, /<path d="M/i);
        assert.doesNotMatch(svg, /(?:\bid=|\bhref=|<image|<foreignObject|url\(|NaN|undefined|<rect)/);
        assert.equal((svg.match(/<svg\b/g) ?? []).length, 1);
      }
    }
  }
});

test("Library objects preserve tint and distinct layered semantics independently of selection", () => {
  assert.match(render(), /style="color:#ffffff"/);
  assert.match(render({ color: "#102030" }), /style="color:#102030"/);
  assert.match(render({ color: "oklch(62% 0.15 180)" }), /style="color:oklch\(62% 0.15 180\)"/);
  assert.equal(render({ icon: "future-key" }), render({ icon: "library" }));
  for (const icon of ["library", "archive", "box", "book", "camera", "folder-heart"]) {
    const object = render({ icon });
    const symbol = render({ icon, variant: "symbol", color: "currentColor" });
    assert.match(object, /data-identity-layer="detail"/);
    assert.match(object, /opacity="\.\d+"/);
    assert.doesNotMatch(symbol, /data-identity-layer="detail"/);
    assert.match(symbol, /style="color:currentColor"/);
    assert.notEqual(object, symbol);
  }
  assert.notEqual(render({ icon: "camera" }), render({ icon: "aperture" }));
  assert.notEqual(render({ icon: "folder" }), render({ icon: "folder-heart" }));
});

test("vendored identity geometry retains upstream source and full MIT attribution", async () => {
  const source = await readFile(new URL("../src/features/library/identityFluentPaths.ts", import.meta.url), "utf8");
  const license = await readFile(new URL("../src/features/library/identityFluent.LICENSE.md", import.meta.url), "utf8");
  assert.match(source, /9eb184e96268d3293462f9d9d19225b4c7b07774/);
  assert.match(license, /Copyright \(c\) 2020 Microsoft Corporation/);
  assert.match(license, /Permission is hereby granted, free of charge/);
  assert.match(license, /THE SOFTWARE IS PROVIDED "AS IS"/);
});

test("typographic additions preserve all 384 metadata entries and complete semantic series", () => {
  const prior = artwork.IDENTITY_ICONS.slice(0, 384);
  assert.equal(createHash("sha256").update(JSON.stringify(prior)).digest("hex"), "ecf655ef5a8d96239d589349b601bb170970212c5c2e990dabe47c426fea46e8");
  assert.equal(createHash("sha256").update(JSON.stringify(artwork.IDENTITY_FLUENT_PATHS)).digest("hex"), "723f6a90e8379b3263227d5308fbdcb809a578baea0aa9cd3399a88665b448a7");
  const added = artwork.IDENTITY_ICONS.slice(384);
  assert.equal(added.length, 118);
  assert.deepEqual(Object.keys(artwork.IDENTITY_TYPOGRAPHIC_PATHS), added.map(({ key }) => key));
  for (const icon of added) {
    assert.equal(icon.sourceKind, "typographic");
    assert.match(icon.key, /^[a-z0-9-]+$/);
    assert.ok(artwork.matchesIdentityIcon(icon, icon.keywords.split(" ")[0]));
    const svg = renderToStaticMarkup(createElement(artwork.FolderGlyph, { icon: icon.key, color: "#ffffff", size: 18 }));
    assert.match(svg, new RegExp(`data-identity-icon="${icon.key}"`));
    assert.match(svg, /data-identity-variant="symbol"/);
    assert.doesNotMatch(svg, /<text|<use|href=|font-family/);
  }
  for (const caseName of ["uppercase", "lowercase"]) {
    assert.equal(added.filter(({ category }) => category === `letters-${caseName}`).length, 26);
    for (let i = 0; i < 26; i++) {
      const letter = String.fromCharCode(97 + i);
      const entry = artwork.resolveIdentityIcon(`letter-${caseName}-${letter}`);
      assert.ok(artwork.matchesIdentityIcon(entry, `latin ${caseName} ${letter}`));
    }
    for (let i = 1; i <= 12; i++) {
      const entry = artwork.resolveIdentityIcon(`roman-${caseName}-${i}`);
      assert.ok(artwork.matchesIdentityIcon(entry, `roman ${caseName} ${i}`));
      assert.ok(artwork.matchesIdentityIcon(entry, String.fromCharCode((caseName === "uppercase" ? 0x2160 : 0x2170) + i - 1)));
    }
  }
  for (let i = 0; i < 10; i++) {
    for (const kind of ["plain", "square"]) assert.ok(artwork.matchesIdentityIcon(artwork.resolveIdentityIcon(`number-${kind}-${i}`), `arabic ${kind} ${i}`));
  }
  const chinese = added.filter(({ category }) => category === "chinese-numerals");
  assert.equal(chinese.length, 22);
  for (const char of "〇零一二三四五六七八九十壹贰叁肆伍陆柒捌玖拾") {
    assert.equal(chinese.filter(({ keywords }) => keywords.split(" ")[0] === char).length, 1);
  }
});

test("typographic outlines retain pinned source and OFL attribution separately from Fluent", async () => {
  const license = await readFile(new URL("../src/features/library/identityTypography.LICENSE.md", import.meta.url), "utf8");
  const shippedLicense = await readFile(new URL("../public/LICENSE-NotoSansCJK.txt", import.meta.url), "utf8");
  assert.equal(shippedLicense, license, "Vite public assets must include the complete canonical copyright and OFL notice");
  assert.match(license, /f8d157532fbfaeda587e826d4cd5b21a49186f7c/);
  assert.match(license, /ca094f6b0001fb048ca39ddd797a0cdb0179e1e55c6561e111c49c3e6a61d7b7/);
  assert.match(license, /2014-2021 Adobe/);
  assert.match(license, /SIL OPEN FONT LICENSE Version 1.1/);
});

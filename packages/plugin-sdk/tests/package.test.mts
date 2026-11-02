import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { CORE_METHODS } from "../src/core-methods.ts";
import { pluginUiCoreMethods, type PluginUiCoreClient } from "../../../apps/web/src/features/plugins/pluginUiCoreMethods.ts";

test("SDK method names match the Host's explicit callable transport surface", () => {
  const client = Object.fromEntries(CORE_METHODS.map((method) => [method, async () => undefined])) as unknown as PluginUiCoreClient;
  assert.deepEqual([...CORE_METHODS].sort(), Object.keys(pluginUiCoreMethods(client)).sort());
});

test("public SDK contains generic design/runtime without Host transport or provider UI", async () => {
  const runtime = await readFile(new URL("../dist/index.js", import.meta.url), "utf8");
  const design = await readFile(new URL("../dist/design.js", import.meta.url), "utf8");
  const css = await readFile(new URL("../dist/design.css", import.meta.url), "utf8");
  assert.doesNotMatch(runtime, /fetch\(|sessionToken|baseUrl|mangooDesktop/);
  assert.doesNotMatch(design, /OneDrive|SyncManagementPanel|SyncFoldersPanel|RestoreLibraryWizard/);
  assert.doesNotMatch(css, /settings-sync|sync-folder|managed-library-(?:restore|mode)|plugin-account/);
  assert.match(css, /\[data-interactive-pointer-target\]/);
  assert.match(design, /function LiquidGlassLayer/);
  const build = await readFile(new URL("../build.mjs", import.meta.url), "utf8");
  assert.doesNotMatch(build, /OneDrive|Mangoo_OneDriveSync|plugin-ui-extraction/);
});

test("shared design bundles glyphs without leaking an icon dependency to SDK consumers", async () => {
  const design = await readFile(new URL("../dist/design.js", import.meta.url), "utf8");
  assert.doesNotMatch(design, /(?:from\s*|import\s*\()\s*["'](?:@phosphor-icons\/react|lucide-react)/);
  assert.doesNotMatch(design, /lucide-react/);
  const typesRoot = fileURLToPath(new URL("../dist/types/", import.meta.url));
  const declarations = (await readdir(typesRoot, { recursive: true })).filter((file) => file.endsWith(".d.ts"));
  assert.ok(declarations.length > 0);
  for (const file of declarations) {
    assert.doesNotMatch(await readFile(path.join(typesRoot, file), "utf8"), /@phosphor-icons\/react|lucide-react/, file);
  }
});

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { it } from "node:test";

it("preserves Main's active-library rejection for a stale Settings snapshot", async () => {
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      removeLibraryFromList: async () => ({
        ok: false,
        error: "cannot remove the active library"
      })
    }
  };
  const { removeLibraryFromList } = await import("../src/core/desktop.ts?removal-guard");

  assert.deepEqual(await removeLibraryFromList("active-library"), {
    ok: false,
    error: "cannot remove the active library"
  });
});

it("surfaces a stale Settings removal rejection instead of silently reloading", async () => {
  const source = await readFile(
    new URL("../src/features/settings/settingsPanels.tsx", import.meta.url),
    "utf8"
  );
  const removeHandler = source.slice(
    source.indexOf("async function onRemove"),
    source.indexOf("async function onDeleteFile")
  );

  assert.match(removeHandler, /const res = await removeLibraryFromList\(l\.id\)/);
  assert.match(removeHandler, /if \(!res\.ok\) setError\(res\.error \?\? "移除失败"\)/);
});

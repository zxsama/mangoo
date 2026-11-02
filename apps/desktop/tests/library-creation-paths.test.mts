import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { resolveLibraryCreationPaths } from "../src/library-creation-paths.ts";
import { canAccessDesktopIpc } from "../src/desktop-ipc-permissions.ts";

test("creation paths use real Documents and pure path arithmetic even when ancestors are absent", () => {
  assert.deepEqual(resolveLibraryCreationPaths("R:\\Redirected Documents", { name: "设计" }, "win32"), {
    databasePath: "R:\\Redirected Documents\\Mangoo\\Libraries\\设计\\Database",
    defaultContentPath: "R:\\Redirected Documents\\Mangoo\\Libraries\\设计\\Files"
  });
  assert.deepEqual(resolveLibraryCreationPaths("/mnt/documents", { name: "Photos" }, "linux"), {
    databasePath: "/mnt/documents/Mangoo/Libraries/Photos/Database", defaultContentPath: "/mnt/documents/Mangoo/Libraries/Photos/Files"
  });
  const source = readFileSync(new URL("../src/library-creation-paths.ts", import.meta.url), "utf8");
  assert.doesNotMatch(source, /node:fs|mkdir|accessSync|showOpenDialog/);
});

test("custom Database is exact and derives sibling Files without replacing Core name validation", () => {
  assert.deepEqual(resolveLibraryCreationPaths("", { name: "CON", databasePath: "D:\\Archive\\Catalog" }, "win32"), {
    databasePath: "D:\\Archive\\Catalog", defaultContentPath: "D:\\Archive\\Files"
  });
  assert.throws(() => resolveLibraryCreationPaths("D:\\Documents", { name: "Photos", databasePath: "relative" }, "win32"), /完整路径/);
  assert.throws(() => resolveLibraryCreationPaths("D:\\Documents", { name: "Photos", databasePath: "\\drive-relative" }, "win32"), /完整路径/);
});

test("automatic invalid Windows names fail without substitutions or suffixes", () => {
  for (const name of ["", ".", "..", "CON", "nul.txt", "CONIN$", "COM1", "LPT9.png", "COM¹", "bad/name", "bad\\name", "bad.", "bad ", "bad:"]) {
    assert.throws(() => resolveLibraryCreationPaths("D:\\Documents", { name }, "win32"), (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.equal(error.message, "名称不能用于自动目录，请修改名称。");
      return true;
    });
  }
});

test("native creation preview is restricted to main and Settings, with explicit sender check", () => {
  const channel = "mangoo:resolve-library-creation-paths";
  assert.equal(canAccessDesktopIpc("main", channel), true);
  assert.equal(canAccessDesktopIpc("settings", channel), true);
  for (const role of ["plugins", "notifications", null] as const) assert.equal(canAccessDesktopIpc(role, channel), false);
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  assert.match(main, /requireDesktopIpcAccess\(event, "mangoo:resolve-library-creation-paths"\)/);
  assert.match(main, /resolveLibraryCreationPaths\(app.getPath\("documents"\), input\)/);
});

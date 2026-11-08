import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { ViewerAccessPolicy } from "../src/desktop/accessPolicy.ts";
import { parseDesktopState, visibleBounds } from "../src/desktop/desktopState.ts";
import {
  linuxDesktopEntry,
  linuxMimeTypes,
  registerAssociations,
  unregisterAssociations,
  updateMimeApps,
  windowsRegistrationScript,
  windowsUnregistrationScript,
  type AssociationEnvironment
} from "../src/desktop/fileAssociations.ts";
import { extractOpenPaths } from "../src/desktop/launchArgs.ts";
import { IMAGE_VIEWER_CHANNELS, parsePathList } from "../src/shared/ipc.ts";

const extensions = new Set([".jpg", ".png", ".heic"]);

test("launch args keep files and drop switches, the app script and URLs", () => {
  const argv = ["C:\\Mangoo\\Mangoo.exe", "--allow-file-access-from-files", "C:\\Photos\\a.jpg", "relative\\b.png", "mangoo://x", "-x"];
  assert.deepEqual(extractOpenPaths(argv, { defaultApp: false, cwd: "D:\\work", platform: "win32" }), [
    "C:\\Photos\\a.jpg",
    "D:\\work\\relative\\b.png"
  ]);
  assert.deepEqual(
    extractOpenPaths(["electron", "dist/main.js", "/home/u/p.png"], { defaultApp: true, cwd: "/", platform: "linux" }),
    ["/home/u/p.png"]
  );
  assert.deepEqual(extractOpenPaths(["electron", "dist/main.js"], { defaultApp: true, cwd: "/", platform: "linux" }), []);
});

test("access policy admits only images directly inside folders the user opened", () => {
  const policy = new ViewerAccessPolicy("win32");
  policy.admitFile("C:\\Photos\\Trip\\a.jpg");
  assert.equal(policy.allows("C:\\Photos\\Trip\\b.png", extensions), true);
  assert.equal(policy.allows("c:\\photos\\trip\\B.PNG".replace("B.PNG", "b.png"), extensions), true, "case-insensitive on Windows");
  assert.equal(policy.allows("C:\\Photos\\Trip\\notes.txt", extensions), false, "non-image");
  assert.equal(policy.allows("C:\\Photos\\Trip\\sub\\c.jpg", extensions), false, "subfolder not admitted");
  assert.equal(policy.allows("C:\\Photos\\Trip\\..\\secret.jpg", extensions), false, "traversal");
  assert.equal(policy.allows("C:\\Windows\\a.jpg", extensions), false, "other folder");
  assert.equal(policy.allows("relative\\a.jpg", extensions), false, "relative");
  policy.clear();
  assert.equal(policy.allows("C:\\Photos\\Trip\\b.png", extensions), false, "cleared with the window");

  const posix = new ViewerAccessPolicy("linux");
  posix.admitFolder("/home/u/Pictures");
  assert.equal(posix.allows("/home/u/Pictures/x.heic", extensions), true);
  assert.equal(posix.allows("/home/u/pictures/x.heic", extensions), false, "case-sensitive on Linux");
});

test("desktop state parsing is defensive and restored windows stay visible", () => {
  assert.equal(parseDesktopState("not json").associationWanted, false);
  const state = parseDesktopState(JSON.stringify({
    associationWanted: true,
    registration: { executable: "C:\\Mangoo.exe", extensions: [".jpg", "bad ext"] },
    window: { bounds: { x: 10, y: 10, width: 900, height: 600 }, maximized: true }
  }));
  assert.equal(state.associationWanted, true);
  assert.equal(state.registration, null, "invalid extension list is rejected");
  assert.deepEqual(state.window?.bounds, { x: 10, y: 10, width: 900, height: 600 });
  const work = [{ x: 0, y: 0, width: 1920, height: 1040 }];
  assert.deepEqual(visibleBounds({ x: 100, y: 100, width: 800, height: 600 }, work), { x: 100, y: 100, width: 800, height: 600 });
  const offscreen = visibleBounds({ x: 5000, y: 5000, width: 800, height: 600 }, work);
  assert.deepEqual(offscreen, { x: 560, y: 220, width: 800, height: 600 });
  assert.equal(visibleBounds(null, work).width, 1120);
});

function environment(platform: NodeJS.Platform, root: string, calls: string[][] = []): AssociationEnvironment {
  return {
    platform,
    executable: platform === "win32" ? "C:\\Program Files\\Mangoo\\Mangoo.exe" : "/opt/Mangoo/mangoo",
    launchArgs: [],
    appKey: "Mangoo",
    displayName: "Mangoo 看图",
    progId: "Mangoo.Image",
    iconPath: "/opt/Mangoo/icon.png",
    homeDir: root,
    tempDir: root,
    run: async (command, args) => {
      calls.push([command, ...args]);
      return { code: 0, stdout: "", stderr: "" };
    }
  };
}

test("Windows registration is per-user, escapes paths and is exactly reversible", () => {
  const env = environment("win32", os.tmpdir());
  const script = windowsRegistrationScript(env, [".heic", ".jpg"]);
  assert.match(script, /^Windows Registry Editor Version 5\.00/);
  assert.ok(!/HKEY_LOCAL_MACHINE/.test(script), "never machine-wide");
  assert.match(script, /\[HKEY_CURRENT_USER\\Software\\Classes\\Mangoo\.Image\\shell\\open\\command\]\r\n@="\\"C:\\\\Program Files\\\\Mangoo\\\\Mangoo\.exe\\" \\"%1\\""/);
  assert.match(script, /\[HKEY_CURRENT_USER\\Software\\Classes\\\.heic\\OpenWithProgids\]\r\n"Mangoo\.Image"=""/);
  assert.match(script, /"Mangoo"="Software\\\\Mangoo\\\\ImageViewer\\\\Capabilities"/);
  const removal = windowsUnregistrationScript(env, { executable: env.executable, extensions: [".heic", ".jpg"] });
  assert.match(removal, /\[-HKEY_CURRENT_USER\\Software\\Classes\\Mangoo\.Image\]/);
  assert.match(removal, /\[HKEY_CURRENT_USER\\Software\\Classes\\\.jpg\\OpenWithProgids\]\r\n"Mangoo\.Image"=-/);
  assert.match(removal, /"Mangoo"=-/);
  // Never deletes a whole extension key or another app's Applications entry.
  assert.ok(!/\[-HKEY_CURRENT_USER\\Software\\Classes\\\.jpg/.test(removal));
  assert.ok(!/\[-HKEY_CURRENT_USER\\Software\\Classes\\Applications\\Mangoo\.exe\]/.test(removal));
});

test("Windows registration imports one UTF-16 .reg file and cleans it up", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mangoo-assoc-"));
  try {
    const calls: string[][] = [];
    const env = environment("win32", root, calls);
    let imported = "";
    env.run = async (command, args) => {
      calls.push([command, ...args]);
      const bytes = fs.readFileSync(args[1]);
      assert.deepEqual([...bytes.subarray(0, 2)], [0xff, 0xfe], "UTF-16LE BOM");
      imported = bytes.subarray(2).toString("utf16le");
      return { code: 0, stdout: "", stderr: "" };
    };
    const registration = await registerAssociations(env, [".JPG", ".png", "bad", ".png"]);
    assert.deepEqual(registration.extensions, [".jpg", ".png"]);
    assert.equal(calls.length, 1);
    assert.equal(calls[0][0], "reg.exe");
    assert.match(imported, /Mangoo 看图/);
    assert.deepEqual(fs.readdirSync(root), [], "temporary .reg removed");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("Linux registration writes an XDG entry and defaults, and removes only ours", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mangoo-xdg-"));
  const previousData = process.env.XDG_DATA_HOME;
  const previousConfig = process.env.XDG_CONFIG_HOME;
  process.env.XDG_DATA_HOME = path.join(root, "data");
  process.env.XDG_CONFIG_HOME = path.join(root, "config");
  try {
    const env = environment("linux", root);
    fs.mkdirSync(process.env.XDG_CONFIG_HOME, { recursive: true });
    fs.writeFileSync(path.join(process.env.XDG_CONFIG_HOME, "mimeapps.list"), "[Default Applications]\nimage/png=other.desktop\ntext/plain=editor.desktop\n");
    await registerAssociations(env, [".jpg", ".png"]);
    const entry = fs.readFileSync(path.join(process.env.XDG_DATA_HOME, "applications", "mangoo-image-viewer.desktop"), "utf8");
    assert.match(entry, /^Exec=\/opt\/Mangoo\/mangoo %F$/m);
    assert.match(entry, /^MimeType=image\/jpeg;image\/png;$/m);
    let mimeApps = fs.readFileSync(path.join(process.env.XDG_CONFIG_HOME, "mimeapps.list"), "utf8");
    assert.match(mimeApps, /image\/png=mangoo-image-viewer\.desktop/);
    assert.match(mimeApps, /text\/plain=editor\.desktop/);
    await unregisterAssociations(env, { executable: env.executable, extensions: [".jpg", ".png"] });
    mimeApps = fs.readFileSync(path.join(process.env.XDG_CONFIG_HOME, "mimeapps.list"), "utf8");
    assert.doesNotMatch(mimeApps, /mangoo-image-viewer/);
    assert.match(mimeApps, /text\/plain=editor\.desktop/);
    assert.equal(fs.existsSync(path.join(process.env.XDG_DATA_HOME, "applications", "mangoo-image-viewer.desktop")), false);
  } finally {
    if (previousData === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousData;
    if (previousConfig === undefined) delete process.env.XDG_CONFIG_HOME;
    else process.env.XDG_CONFIG_HOME = previousConfig;
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("mimeapps editing keeps foreign defaults and quoting is shell safe", () => {
  const added = updateMimeApps("", "mangoo-image-viewer.desktop", ["image/png"], false);
  assert.equal(added, "[Default Applications]\nimage/png=mangoo-image-viewer.desktop\n");
  const foreign = "[Default Applications]\nimage/png=other.desktop\n";
  assert.equal(updateMimeApps(foreign, "mangoo-image-viewer.desktop", ["image/png"], true), foreign);
  assert.deepEqual(linuxMimeTypes([".jpg", ".jpeg", ".heic", ".unknown"]), ["image/heic", "image/jpeg"]);
  const env = { ...environment("linux", "/tmp"), executable: "/opt/My Apps/mangoo" };
  assert.match(linuxDesktopEntry(env, [".png"]), /^Exec="\/opt\/My Apps\/mangoo" %F$/m);
});

test("renderer path lists are bounded and typed", () => {
  assert.deepEqual(parsePathList(["C:\\a.jpg"]), ["C:\\a.jpg"]);
  assert.equal(parsePathList([]), null);
  assert.equal(parsePathList("C:\\a.jpg"), null);
  assert.equal(parsePathList(["ok", 5]), null);
  assert.equal(parsePathList(["bad\0"]), null);
  assert.equal(parsePathList(new Array(5000).fill("a")), null);
});

test("preload exposes exactly the plugin's viewer channels", () => {
  const preload = fs.readFileSync(new URL("../../../apps/desktop/src/preload.cjs", import.meta.url), "utf8");
  for (const channel of Object.values(IMAGE_VIEWER_CHANNELS)) {
    assert.ok(preload.includes(`"${channel}"`), `preload is missing ${channel}`);
  }
  const used = [...preload.matchAll(/"(mangoo:image-viewer:[a-z-]+)"/g)].map((match) => match[1]);
  const known = new Set(Object.values(IMAGE_VIEWER_CHANNELS) as string[]);
  for (const channel of used) assert.ok(known.has(channel), `preload uses unknown channel ${channel}`);
});

test("the settings window bridge exposes only association channels and section pushes", () => {
  const preload = fs.readFileSync(new URL("../../../apps/desktop/src/preload.cjs", import.meta.url), "utf8");
  const start = preload.indexOf("const imageViewerSettingsWindowBridge = {");
  assert.ok(start >= 0, "settings window bridge exists");
  const body = preload.slice(start, preload.indexOf("\n};", start));
  const used = new Set([...body.matchAll(/"(mangoo:image-viewer:[a-z-]+)"/g)].map((match) => match[1]));
  assert.deepEqual([...used].sort(), [
    IMAGE_VIEWER_CHANNELS.associationStatus,
    IMAGE_VIEWER_CHANNELS.openDefaultAppsSettings,
    IMAGE_VIEWER_CHANNELS.setAssociation,
    IMAGE_VIEWER_CHANNELS.settingsSection
  ].sort());
});

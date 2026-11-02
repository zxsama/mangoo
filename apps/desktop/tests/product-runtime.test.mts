import assert from "node:assert/strict";
import path from "node:path";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { describe, it } from "node:test";
import {
  configurePackagedRuntime,
  mainWindowSkipsTaskbarDuringBootstrap,
  MANGOO_APP_USER_MODEL_ID,
  applicationIdentity,
  windowIconPath,
  rendererUrlForSession,
  withDefaultHostStorage
} from "../src/product-runtime.ts";

describe("packaged desktop runtime", () => {
  it("points Core, web, and Providers at packaged resources", () => {
    const env: NodeJS.ProcessEnv = {
      MANGOO_ALLOWED_ORIGIN: "http://127.0.0.1:4173",
      MANGOO_CORE_ADDR: "127.0.0.1:4173",
      MANGOO_CORE_PROFILE: "release",
      MANGOO_CORE_URL: "http://127.0.0.1:4173/api",
      MANGOO_WEB_URL: "http://127.0.0.1:4173"
    };
    configurePackagedRuntime(
      { isPackaged: true, resourcesPath: "C:\\Mangoo\\resources", parentPid: 4242 },
      env
    );

    assert.equal(env.MANGOO_CORE_BINARY, path.join("C:\\Mangoo\\resources", "core", "mangoo-core.exe"));
    assert.equal(
      env.MANGOO_PLUGINS_DIR,
      path.join("C:\\Mangoo\\resources", "core", "bundled-plugins")
    );
    assert.equal(env.MANGOO_WEB_DIR, path.join("C:\\Mangoo\\resources", "web"));
    assert.equal(env.MANGOO_SERVE_WEB, "1");
    assert.equal(env.MANGOO_PARENT_PID, "4242");
    assert.equal(env.MANGOO_ALLOWED_ORIGIN, undefined);
    assert.equal(env.MANGOO_CORE_ADDR, undefined);
    assert.equal(env.MANGOO_CORE_PROFILE, undefined);
    assert.equal(env.MANGOO_CORE_URL, undefined);
    assert.equal(env.MANGOO_WEB_URL, undefined);
  });

  it("leaves development and external-Core environments unchanged", () => {
    const developmentEnv: NodeJS.ProcessEnv = { MANGOO_WEB_URL: "http://127.0.0.1:5173" };
    configurePackagedRuntime(
      { isPackaged: false, resourcesPath: "C:\\Mangoo\\resources", parentPid: 10 },
      developmentEnv
    );
    assert.deepEqual(developmentEnv, { MANGOO_WEB_URL: "http://127.0.0.1:5173" });

    const externalEnv: NodeJS.ProcessEnv = { MANGOO_CORE_EXTERNAL: "1", MANGOO_WEB_URL: "https://example.test" };
    configurePackagedRuntime(
      { isPackaged: true, resourcesPath: "C:\\Mangoo\\resources", parentPid: 11 },
      externalEnv
    );
    assert.deepEqual(externalEnv, { MANGOO_CORE_EXTERNAL: "1", MANGOO_WEB_URL: "https://example.test" });
  });

  it("loads the renderer from the Core origin and keeps hash routes", () => {
    assert.equal(
      rendererUrlForSession({ baseUrl: "http://127.0.0.1:49152/api" }),
      "http://127.0.0.1:49152/"
    );
    assert.equal(
      rendererUrlForSession({ baseUrl: "http://127.0.0.1:49152/api" }, "#/settings"),
      "http://127.0.0.1:49152/#/settings"
    );
  });

  it("uses the Mangoo identity in both development and packaged startup", () => {
    assert.equal(MANGOO_APP_USER_MODEL_ID, "com.mangoo.desktop");
    assert.deepEqual(applicationIdentity("win32"), {
      name: "Mangoo",
      appUserModelId: MANGOO_APP_USER_MODEL_ID
    });
    assert.deepEqual(applicationIdentity("linux"), { name: "Mangoo" });
  });

  it("keeps the packaged icon in the resource directory", () => {
    assert.equal(
      windowIconPath(true, "C:\\Mangoo\\resources", "unused-development-path"),
      path.join("C:\\Mangoo\\resources", "icon.png")
    );
  });

  it("uses the generated development icon only when it exists", (context) => {
    const desktopPath = mkdtempSync(path.join(tmpdir(), "mangoo desktop icon "));
    context.after(() => rmSync(desktopPath, { recursive: true, force: true }));
    assert.equal(windowIconPath(false, "unused-resources-path", desktopPath), undefined);
    mkdirSync(path.join(desktopPath, "build"));
    const iconPath = path.join(desktopPath, "build", "icon.png");
    writeFileSync(iconPath, Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==", "base64"));
    assert.equal(windowIconPath(false, "unused-resources-path", desktopPath), iconPath);
    assert.equal(path.isAbsolute(windowIconPath(false, "unused-resources-path", desktopPath)!), true);
  });

  it("keeps the packaged main window registered with the taskbar during bootstrap", () => {
    assert.equal(mainWindowSkipsTaskbarDuringBootstrap(true), false);
    assert.equal(mainWindowSkipsTaskbarDuringBootstrap(false), true);
  });

  it("preserves launcher-selected Host storage and fills only missing defaults", () => {
    const baseSession = {
      baseUrl: "http://127.0.0.1:49152/api",
      bindAddress: "127.0.0.1:49152",
      sessionToken: "session",
      allowedOrigin: "http://127.0.0.1:49152"
    };

    assert.deepEqual(withDefaultHostStorage(baseSession, "C:\\Users\\Mangoo"), {
      ...baseSession,
      hostDbPath: path.join("C:\\Users\\Mangoo", "host", "host.sqlite"),
      librariesDir: path.join("C:\\Users\\Mangoo", "host", "libraries")
    });

    assert.deepEqual(
      withDefaultHostStorage(
        {
          ...baseSession,
          hostDbPath: "D:\\Custom Profile\\host.sqlite",
          librariesDir: "E:\\Custom Libraries"
        },
        "C:\\Users\\Mangoo"
      ),
      {
        ...baseSession,
        hostDbPath: "D:\\Custom Profile\\host.sqlite",
        librariesDir: "E:\\Custom Libraries"
      }
    );
  });
});

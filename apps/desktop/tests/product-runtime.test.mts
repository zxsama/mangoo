import assert from "node:assert/strict";
import path from "node:path";
import { describe, it } from "node:test";
import {
  configurePackagedRuntime,
  MEGLE_APP_USER_MODEL_ID,
  packagedApplicationIdentity,
  packagedWindowIconPath,
  rendererUrlForSession,
  withDefaultHostStorage
} from "../src/product-runtime.ts";

describe("packaged desktop runtime", () => {
  it("points Core, web, and Providers at packaged resources", () => {
    const env: NodeJS.ProcessEnv = {
      MEGLE_ALLOWED_ORIGIN: "http://127.0.0.1:4173",
      MEGLE_CORE_ADDR: "127.0.0.1:4173",
      MEGLE_CORE_PROFILE: "release",
      MEGLE_CORE_URL: "http://127.0.0.1:4173/api",
      MEGLE_WEB_URL: "http://127.0.0.1:4173"
    };
    configurePackagedRuntime(
      { isPackaged: true, resourcesPath: "C:\\Megle\\resources", parentPid: 4242 },
      env
    );

    assert.equal(env.MEGLE_CORE_BINARY, path.join("C:\\Megle\\resources", "core", "megle-core.exe"));
    assert.equal(
      env.MEGLE_PLUGINS_DIR,
      path.join("C:\\Megle\\resources", "core", "bundled-plugins")
    );
    assert.equal(env.MEGLE_WEB_DIR, path.join("C:\\Megle\\resources", "web"));
    assert.equal(env.MEGLE_SERVE_WEB, "1");
    assert.equal(env.MEGLE_PARENT_PID, "4242");
    assert.equal(env.MEGLE_ALLOWED_ORIGIN, undefined);
    assert.equal(env.MEGLE_CORE_ADDR, undefined);
    assert.equal(env.MEGLE_CORE_PROFILE, undefined);
    assert.equal(env.MEGLE_CORE_URL, undefined);
    assert.equal(env.MEGLE_WEB_URL, undefined);
  });

  it("leaves development and external-Core environments unchanged", () => {
    const developmentEnv: NodeJS.ProcessEnv = { MEGLE_WEB_URL: "http://127.0.0.1:5173" };
    configurePackagedRuntime(
      { isPackaged: false, resourcesPath: "C:\\Megle\\resources", parentPid: 10 },
      developmentEnv
    );
    assert.deepEqual(developmentEnv, { MEGLE_WEB_URL: "http://127.0.0.1:5173" });

    const externalEnv: NodeJS.ProcessEnv = { MEGLE_CORE_EXTERNAL: "1", MEGLE_WEB_URL: "https://example.test" };
    configurePackagedRuntime(
      { isPackaged: true, resourcesPath: "C:\\Megle\\resources", parentPid: 11 },
      externalEnv
    );
    assert.deepEqual(externalEnv, { MEGLE_CORE_EXTERNAL: "1", MEGLE_WEB_URL: "https://example.test" });
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
    assert.equal(MEGLE_APP_USER_MODEL_ID, "com.megle.desktop");
    assert.deepEqual(packagedApplicationIdentity(true, "win32"), {
      name: "Megle",
      appUserModelId: MEGLE_APP_USER_MODEL_ID
    });
    assert.deepEqual(packagedApplicationIdentity(true, "linux"), { name: "Megle" });
    assert.equal(packagedApplicationIdentity(false, "win32"), undefined);
    assert.equal(
      packagedWindowIconPath(true, "C:\\Megle\\resources"),
      path.join("C:\\Megle\\resources", "icon.png")
    );
    assert.equal(packagedWindowIconPath(false, "C:\\Megle\\resources"), undefined);
  });

  it("preserves launcher-selected Host storage and fills only missing defaults", () => {
    const baseSession = {
      baseUrl: "http://127.0.0.1:49152/api",
      bindAddress: "127.0.0.1:49152",
      sessionToken: "session",
      allowedOrigin: "http://127.0.0.1:49152"
    };

    assert.deepEqual(withDefaultHostStorage(baseSession, "C:\\Users\\Megle"), {
      ...baseSession,
      hostDbPath: path.join("C:\\Users\\Megle", "host", "host.sqlite"),
      librariesDir: path.join("C:\\Users\\Megle", "host", "libraries")
    });

    assert.deepEqual(
      withDefaultHostStorage(
        {
          ...baseSession,
          hostDbPath: "D:\\Megle\\.data\\host\\host.sqlite",
          librariesDir: "D:\\Megle\\.data\\host\\libraries"
        },
        "C:\\Users\\Megle"
      ),
      {
        ...baseSession,
        hostDbPath: "D:\\Megle\\.data\\host\\host.sqlite",
        librariesDir: "D:\\Megle\\.data\\host\\libraries"
      }
    );
  });
});

import path from "node:path";
import type { CoreSession } from "./core-session.js";

export const MEGLE_APP_USER_MODEL_ID = "com.megle.desktop";

interface PackagedRuntimeOptions {
  readonly isPackaged: boolean;
  readonly resourcesPath: string;
  readonly parentPid: number;
}

export interface PackagedApplicationIdentity {
  readonly name: "Megle";
  readonly appUserModelId?: typeof MEGLE_APP_USER_MODEL_ID;
}

export function configurePackagedRuntime(
  options: PackagedRuntimeOptions,
  env: NodeJS.ProcessEnv = process.env
): void {
  if (!options.isPackaged || env.MEGLE_CORE_EXTERNAL === "1") return;

  env.MEGLE_CORE_BINARY = path.join(options.resourcesPath, "core", "megle-core.exe");
  env.MEGLE_PLUGINS_DIR = path.join(options.resourcesPath, "core", "bundled-plugins");
  env.MEGLE_SERVE_WEB = "1";
  env.MEGLE_WEB_DIR = path.join(options.resourcesPath, "web");
  env.MEGLE_PARENT_PID = String(options.parentPid);
  delete env.MEGLE_CORE_ADDR;
  delete env.MEGLE_CORE_PROFILE;
  delete env.MEGLE_CORE_URL;
  delete env.MEGLE_ALLOWED_ORIGIN;
  delete env.MEGLE_WEB_URL;
}

export function packagedApplicationIdentity(
  isPackaged: boolean,
  platform: NodeJS.Platform = process.platform
): PackagedApplicationIdentity | undefined {
  if (!isPackaged) return undefined;
  return {
    name: "Megle",
    ...(platform === "win32" ? { appUserModelId: MEGLE_APP_USER_MODEL_ID } : {})
  };
}

export function packagedWindowIconPath(isPackaged: boolean, resourcesPath: string): string | undefined {
  return isPackaged ? path.join(resourcesPath, "icon.png") : undefined;
}

export function withDefaultHostStorage(
  session: CoreSession,
  userDataDir: string
): CoreSession {
  const hostRoot = path.join(userDataDir, "host");
  return {
    ...session,
    hostDbPath: session.hostDbPath ?? path.join(hostRoot, "host.sqlite"),
    librariesDir: session.librariesDir ?? path.join(hostRoot, "libraries")
  };
}

export function rendererUrlForSession(session: Pick<CoreSession, "baseUrl">, route = ""): string {
  const rendererUrl = new URL(session.baseUrl);
  rendererUrl.pathname = "/";
  rendererUrl.search = "";
  rendererUrl.hash = route;
  return rendererUrl.toString();
}

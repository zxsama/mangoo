import path from "node:path";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { CoreSession } from "./core-session.js";

export const MANGOO_APP_USER_MODEL_ID = "com.mangoo.desktop";

interface PackagedRuntimeOptions {
  readonly isPackaged: boolean;
  readonly resourcesPath: string;
  readonly parentPid: number;
}

export interface ApplicationIdentity {
  readonly name: "Mangoo";
  readonly appUserModelId?: typeof MANGOO_APP_USER_MODEL_ID;
}

export function configurePackagedRuntime(
  options: PackagedRuntimeOptions,
  env: NodeJS.ProcessEnv = process.env
): void {
  if (!options.isPackaged || env.MANGOO_CORE_EXTERNAL === "1") return;

  env.MANGOO_CORE_BINARY = path.join(options.resourcesPath, "core", "mangoo-core.exe");
  env.MANGOO_PLUGINS_DIR = path.join(options.resourcesPath, "core", "bundled-plugins");
  env.MANGOO_SERVE_WEB = "1";
  env.MANGOO_WEB_DIR = path.join(options.resourcesPath, "web");
  env.MANGOO_PARENT_PID = String(options.parentPid);
  delete env.MANGOO_CORE_ADDR;
  delete env.MANGOO_CORE_PROFILE;
  delete env.MANGOO_CORE_URL;
  delete env.MANGOO_ALLOWED_ORIGIN;
  delete env.MANGOO_WEB_URL;
}

export function applicationIdentity(
  platform: NodeJS.Platform = process.platform
): ApplicationIdentity {
  return {
    name: "Mangoo",
    ...(platform === "win32" ? { appUserModelId: MANGOO_APP_USER_MODEL_ID } : {})
  };
}

export function windowIconPath(
  isPackaged: boolean,
  resourcesPath: string,
  desktopPath = fileURLToPath(new URL("../", import.meta.url))
): string | undefined {
  if (isPackaged) return path.join(resourcesPath, "icon.png");
  const developmentIcon = path.resolve(desktopPath, "build", "icon.png");
  return existsSync(developmentIcon) ? developmentIcon : undefined;
}

export function mainWindowSkipsTaskbarDuringBootstrap(isPackaged: boolean): boolean {
  return !isPackaged;
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

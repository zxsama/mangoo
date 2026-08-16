import { createCoreClient as createCorePackageClient } from "@megle/core-client";
import { getDesktopBridge } from "./desktop.ts";

export { CoreApiError } from "@megle/core-client";
export type { CoreClientConfig } from "@megle/core-client";

let cachedClient: ReturnType<typeof createCorePackageClient> | null = null;
let cachedClientKey = "";

export function getCoreClientConfig() {
  const desktop = getDesktopBridge();

  return {
    baseUrl:
      desktop?.coreUrl ??
      import.meta.env.VITE_MEGLE_CORE_URL ??
      "http://127.0.0.1:47321/api",
    sessionToken: desktop?.sessionToken
  };
}

export function createCoreClient(config = getCoreClientConfig()) {
  const key = `${config.baseUrl}\n${config.sessionToken ?? ""}`;
  if (!cachedClient || cachedClientKey !== key) {
    cachedClient = createCorePackageClient(config);
    cachedClientKey = key;
  }
  return cachedClient;
}

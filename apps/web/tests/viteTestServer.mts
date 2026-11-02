import { createServer as createViteServer, type InlineConfig } from "vite";
import { availableLoopbackPort } from "../../../tools/dev/web-dev-url.mjs";

export type { ViteDevServer } from "vite";

// Vite interprets port 0 as its fixed default, not an OS-assigned port.
// Mounted fixtures need a real free loopback port even when that default is reserved.
export async function createServer(config: InlineConfig = {}) {
  if (config.server?.port !== 0 || config.server.middlewareMode) {
    return createViteServer(config);
  }
  const port = await availableLoopbackPort(0);
  return createViteServer({
    ...config,
    server: { ...config.server, port, strictPort: true }
  });
}

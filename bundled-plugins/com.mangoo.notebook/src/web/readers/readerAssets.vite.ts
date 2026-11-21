import { readFile, readdir } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import type { Plugin } from "vite";

// A classic script and PDF.js's name-addressed resource folders must be served
// verbatim. Vite's normal JS transform and asset hashing would break those URLs.
export function notebookReaderAssets(): Plugin {
  const require = createRequire(import.meta.url);
  const pdf = path.dirname(require.resolve("pdfjs-dist/package.json"));
  const office = path.dirname(require.resolve("@jose.espana/docstream/package.json"));
  const prefix = `assets/notebook-readers/${require("pdfjs-dist/package.json").version}-${require("@jose.espana/docstream/package.json").version}/`;
  async function files() {
    const entries = new Map<string, string>([
      ["officeparser.browser.js", path.join(office, "dist/officeparser.browser.js")],
      ["LICENSE_DOCSTREAM", path.join(office, "LICENSE")],
      ["LICENSE_PDFJS", path.join(pdf, "LICENSE")]
    ]);
    for (const folder of ["cmaps", "standard_fonts", "iccs", "wasm"]) {
      for (const file of await readdir(path.join(pdf, folder), { withFileTypes: true })) {
        if (file.isFile()) entries.set(folder + "/" + file.name, path.join(pdf, folder, file.name));
      }
    }
    return entries;
  }
  return {
    name: "mangoo-notebook-reader-assets",
    config: () => ({ define: { __MANGOO_READER_ASSETS__: JSON.stringify(prefix) } }),
    async configureServer(server) {
      const inventory = await files();
      server.middlewares.use(server.config.base + prefix, (req, res, next) => {
        const name = (req.url ?? "").split("?")[0].replace(/^\//, "");
        const file = inventory.get(name);
        if (!file) { res.statusCode = 404; res.end(); return; }
        void readFile(file).then(bytes => {
          res.setHeader("Content-Type", name.endsWith(".js") ? "application/javascript" : name.endsWith(".wasm") ? "application/wasm" : "application/octet-stream");
          res.end(bytes);
        }, next);
      });
    },
    async generateBundle() {
      const inventory = await files();
      await Promise.all(Array.from(inventory, async ([name, file]) => {
        this.emitFile({ type: "asset", fileName: prefix + name, source: await readFile(file) });
      }));
    }
  };
}

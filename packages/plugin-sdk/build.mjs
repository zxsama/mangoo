import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root = path.dirname(fileURLToPath(import.meta.url));
await mkdir(path.join(root, "dist"), { recursive: true });
await build({ absWorkingDir: root, entryPoints: ["src/index.ts", "src/design.ts"], outdir: "dist", bundle: true, format: "esm", platform: "browser", external: ["react", "react-dom", "react/jsx-runtime"], target: "es2022" });
execFileSync(process.execPath, [path.join(root, "../../node_modules/typescript/bin/tsc"), "-p", path.join(root, "tsconfig.json")], { stdio: "inherit", windowsHide: true });
// CSS primitives are selected from Host source, never copied into a second authored design system.
const css = await readFile(path.join(root, "../../apps/web/src/styles.css"), "utf8");
const blocks = [];
let start = 0, depth = 0;
for (let i = 0; i < css.length; i++) {
  if (css[i] === "{") depth++;
  if (css[i] === "}" && --depth === 0) {
    const block = css.slice(start, i + 1); start = i + 1;
    const selector = block.slice(0, block.indexOf("{"));
    const clean = selector.replace(/\/\*[\s\S]*?\*\//g, "").trim();
    if (/^(?::root|:where\(|@media\s*\(forced-colors:|html\b|body\b|button\b|input\b|select\b|textarea\b|\*|\.liquid-glass\b|\.glass-|\[data-interactive-pointer-target\b|\.mangoo-overlay-scrollbar)/.test(clean)) blocks.push(block);
  }
}
blocks.push(await readFile(path.join(root, "../../apps/web/src/design/overlays/overlay.css"), "utf8"));
await writeFile(path.join(root, "dist/design.css"), blocks.join("\n"));

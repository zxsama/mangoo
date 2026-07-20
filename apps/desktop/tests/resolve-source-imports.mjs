import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export async function resolve(specifier, context, nextResolve) {
  if (specifier.endsWith(".js") && context.parentURL?.startsWith("file:")) {
    const parentPath = fileURLToPath(context.parentURL);
    if (parentPath.includes(`${path.sep}apps${path.sep}desktop${path.sep}src${path.sep}`)) {
      const candidate = path.resolve(path.dirname(parentPath), `${specifier.slice(0, -3)}.ts`);
      if (fs.existsSync(candidate)) {
        return nextResolve(pathToFileURL(candidate).href, context);
      }
    }
  }
  return nextResolve(specifier, context);
}

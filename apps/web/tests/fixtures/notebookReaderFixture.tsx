import { createRoot } from "react-dom/client";
import { DocumentReader } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/readers/DocumentReader";
import { readerAssetUrl } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/readers/readerAssetUrl";

const root = createRoot(document.getElementById("root")!);
Reflect.set(window, "checkReaderResources", async () => {
  const results = [];
  for (const name of ["cmaps/GB-EUC-H.bcmap","standard_fonts/LiberationSans-Regular.ttf","wasm/openjpeg_nowasm_fallback.js","iccs/CGATS001Compat-v2-micro.icc","officeparser.browser.js"]) {
    const response = await fetch(readerAssetUrl(name));
    const bytes = new Uint8Array(await response.arrayBuffer());
    const type = response.headers.get("Content-Type") || "";
    if (!response.ok || bytes.length < 100 || type.includes("text/html")) throw new Error("Missing reader resource " + name);
    results.push({ name, bytes:bytes.length, cache:response.headers.get("Cache-Control") });
  }
  return results;
});
Reflect.set(window, "showReader", (name: string) => {
  const load = async () => {
    const response = await fetch("/reader-fixtures/" + encodeURIComponent(name));
    if (!response.ok) throw new Error("Fixture response " + response.status);
    return response.blob();
  };
  root.render(<DocumentReader key={name} name={name} load={load} />);
});

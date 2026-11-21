import type { OfficeContentNode, OfficeParserAST } from "@jose.espana/docstream/dist/types";
import type { DocumentPage } from "./documentPages";

// A classic worker can load the upstream classic browser distribution in both
// Vite dev and production. Keep runtime imports out of this worker entry.
function escapeMarkup(text: string): string {
  return text.replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
}

type OfficeWorker = typeof globalThis & {
  importScripts: (...urls: string[]) => void;
  officeParser: { parseOffice: (file: ArrayBuffer, config: object) => Promise<OfficeParserAST> };
};
const scope = globalThis as OfficeWorker;

// The browser distribution expects window for its setImmediate shim. Keep it
// in a disposable worker so opening a large workbook cannot freeze the editor.
Object.defineProperty(scope, "window", { value: scope });
scope.onmessage = async ({ data: { data, parserUrl } }: MessageEvent<{ data: ArrayBuffer; parserUrl: string }>) => {
  try {
    scope.importScripts(parserUrl);
    const ast = await scope.officeParser.parseOffice(data, { extractAttachments: true, ocr: false });
    const images = new Map(ast.attachments.filter(item => item.type === "image" && /^image\/(png|jpeg|gif|webp|bmp)$/i.test(item.mimeType)).map(item => [item.name, `data:${item.mimeType};base64,${item.data}`]));
    const render = (node: OfficeContentNode): string => {
      const text = escapeMarkup(node.text ?? "");
      const inner = node.children?.length ? node.children.map(render).join("") : text;
      switch (node.type) {
        case "image": {
          const name = node.metadata && "attachmentName" in node.metadata ? String(node.metadata.attachmentName) : "";
          const image = images.get(name);
          return image ? `<img alt="${escapeMarkup(name)}" src="${image}">` : text;
        }
        case "table": return `<table>${inner}</table>`;
        case "row": return `<tr>${inner}</tr>`;
        case "cell": return `<td>${inner}</td>`;
        case "heading": return `<h2>${inner}</h2>`;
        case "paragraph": return `<p>${inner}</p>`;
        case "list": return `<div>• ${inner}</div>`;
        case "text": return node.formatting?.bold ? `<strong>${inner}</strong>` : node.formatting?.italic ? `<em>${inner}</em>` : inner;
        default: return inner;
      }
    };
    const sections = ast.content.filter(item => item.type === "sheet" || item.type === "slide");
    const pages: DocumentPage[] = sections.length ? sections.map((section, index) => ({
      title: section.metadata && "sheetName" in section.metadata ? String(section.metadata.sheetName) : `第 ${index + 1} 页`,
      html: render(section),
    })) : [{ title: "正文", html: ast.content.map(render).join("") }];
    scope.postMessage({ pages });
  } catch (cause) {
    scope.postMessage({ error: cause instanceof Error ? cause.message : String(cause) });
  }
};

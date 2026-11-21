import Turndown from "turndown";
import { attachmentScheme, isImageFile } from "./noteFormat.ts";

export const imageHelp = (zh: boolean) => zh
  ? "仅有图片链接。请复制图片本身，或通过「插入图片或文件」选择原图。"
  : "Only an image link is available. Copy the image itself or choose Insert image or file.";

interface ClipboardContent {
  files: File[];
  linkedOnly: boolean;
  render(ids: Array<string | undefined>): string;
}

function imageData(src: string, label: string): File | undefined {
  const match = /^data:(image\/(?:png|jpe?g|gif|webp|avif|bmp|svg\+xml))(?:;[^,]*)?,(.*)$/is.exec(src);
  if (!match) return;
  try {
    const base64 = /;base64,/i.test(src.slice(0, src.indexOf(",") + 1));
    const data = base64 ? Uint8Array.from(atob(match[2]), c => c.charCodeAt(0)) : decodeURIComponent(match[2]);
    const ext = match[1].split("/")[1].split("+")[0];
    return new File([data], /\.[a-z\d]+$/i.test(label) ? label : (label || "image") + "." + ext, { type: match[1] });
  } catch { return undefined; }
}

export function imageClipboardContent(transfer: DataTransfer, format: "markdown" | "html"): ClipboardContent | undefined {
  const originals = Array.from(transfer.files).filter(isImageFile);
  const rich = transfer.getData("text/html");
  const plain = transfer.getData("text/plain");
  const files: File[] = [];
  const sources = new Map<string, { token: string; index: number }>();
  const tokenPrefix = `mangoo-paste-${crypto.randomUUID()}-`;
  let linkedOnly = false;
  const collect = (src: string, label: string) => {
    const existing = sources.get(src);
    if (existing) return existing.token;
    if (src.startsWith(attachmentScheme)) return src;
    const byName = originals.findIndex(file => src.split(/[/?#]/).pop() === file.name);
    const file = originals.splice(Math.max(0, byName), 1)[0] ?? imageData(src, label);
    if (!file) { linkedOnly = true; return src; }
    const index = files.push(file) - 1;
    const token = tokenPrefix + index;
    sources.set(src, { token, index });
    return token;
  };
  let text: string;
  // A template keeps clipboard resources inert while inspecting and converting the fragment.
  const template = document.createElement("template");
  if (rich && /<img\b/i.test(rich)) template.innerHTML = rich;
  const images = template.content.querySelectorAll("img[src]");
  if (images.length) {
    template.content.querySelectorAll("script,style,meta,link,base,iframe,object,embed").forEach(node => node.remove());
    for (const node of template.content.querySelectorAll("*")) {
      for (const attr of Array.from(node.attributes)) if (/^on|^srcset$|^style$/i.test(attr.name)) node.removeAttribute(attr.name);
    }
    for (const img of images) {
      const alt = img.getAttribute("alt") || originals[0]?.name || "image";
      img.setAttribute("alt", alt);
      img.setAttribute("src", collect(img.getAttribute("src")!, alt));
    }
    // Turndown walks an already parsed, detached DOM. Never parse remote img URLs in the active document.
    const container = template.content.ownerDocument.createElement("div");
    container.append(template.content);
    text = format === "markdown" ? new Turndown({ headingStyle: "atx", codeBlockStyle: "fenced" }).turndown(container) : container.innerHTML;
  } else {
    // Code editors copy source as plain text. Preserve all syntax around embedded image bytes.
    if (!/data:image\//i.test(plain)) return;
    text = format === "markdown"
      ? plain.replace(/(!\[((?:\\.|[^\]])*)\]\(<?)(data:image\/[^\s)>]+)/gi,
        (_match, start: string, label: string, src: string) => start + collect(src, label))
      : plain.replace(/(<img\b[^>]*?\bsrc\s*=\s*["'])(data:image\/[^"']+)/gi,
        (_match, start: string, src: string) => start + collect(src, ""));
    if (!sources.size) return;
  }
  return { files, linkedOnly, render(ids) {
    let result = text;
    // Delimit the index to avoid replacing the token for image 10 with image 1's ID.
    for (const [src, { token, index }] of [...sources].reverse()) result = result.replaceAll(token, ids[index] ? attachmentScheme + ids[index] : src);
    return result;
  } };
}

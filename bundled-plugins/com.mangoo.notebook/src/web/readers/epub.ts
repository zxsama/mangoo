import { BlobReader, BlobWriter, ZipReader, type FileEntry } from "@zip.js/zip.js/lib/zip-native.js";
import { escapeMarkup, type DocumentPage } from "./documentPages";

function relativePath(base: string, href: string) {
  const url = new URL(href, "https://book.invalid/" + base);
  return url.origin === "https://book.invalid" ? decodeURIComponent(url.pathname.slice(1)) : "";
}

export async function readEpub(blob: Blob, signal: AbortSignal): Promise<DocumentPage[]> {
  const reader = new ZipReader(new BlobReader(blob), { useWebWorkers: false, checkSignature: true });
  try {
    const entries = new Map<string, FileEntry>();
    for await (const entry of reader.getEntriesGenerator()) {
      if (!entry.directory) entries.set(entry.filename, entry);
    }
    // This reader builds sanitized HTML pages in memory. Charge only the XML
    // and embedded images it actually renders, never unused archive members.
    let previewBytes = 0;
    const read = async (name: string, type?: string): Promise<Blob> => {
      signal.throwIfAborted();
      const entry = entries.get(name);
      if (!entry) throw new Error("电子书缺少 " + name);
      previewBytes += entry.uncompressedSize;
      if (entry.uncompressedSize > 32 * 1024 ** 2 || previewBytes > 160 * 1024 ** 2)
        throw new Error("章节或插图超出阅读器的预览容量，可下载原文件阅读。");
      return entry.getData(new BlobWriter(type), { signal });
    };
    const xml = async (name: string) => {
      const document = new DOMParser().parseFromString(await (await read(name)).text(), "application/xml");
      if (document.getElementsByTagName("parsererror").length) throw new Error("电子书章节格式错误。");
      return document;
    };
    const opf = (await xml("META-INF/container.xml")).getElementsByTagNameNS("*", "rootfile")[0]?.getAttribute("full-path");
    if (!opf) throw new Error("未找到电子书目录。");
    const packageDoc = await xml(opf);
    const paths = new Map(Array.from(packageDoc.getElementsByTagNameNS("*", "item"), item => [item.getAttribute("id"), relativePath(opf, item.getAttribute("href") ?? "")]));
    const allowed = new Set("p div span section article h1 h2 h3 h4 h5 h6 blockquote pre code em i strong b sup sub ul ol li table thead tbody tr td th figure figcaption hr br dl dt dd ruby rt".split(" "));
    const imageCache = new Map<string, string>();
    const imagePath = (node: Element, chapter: string) => relativePath(chapter,
      node.getAttribute("src") ?? node.getAttribute("href") ?? node.getAttribute("xlink:href") ?? "");
    const render = (node: Node, chapter: string): string => {
      if (node.nodeType === Node.TEXT_NODE) return escapeMarkup(node.textContent ?? "");
      if (!(node instanceof Element)) return "";
      const tag = node.localName.toLowerCase();
      if (["script", "style", "iframe", "object", "link", "head"].includes(tag)) return "";
      if (tag === "img" || tag === "image") {
        const path = imagePath(node, chapter);
        if (!imageCache.has(path)) return "";
        return `<img alt="${escapeMarkup(node.getAttribute("alt") ?? "")}" src="${imageCache.get(path)}">`;
      }
      const inner = Array.from(node.childNodes, child => render(child, chapter)).join("");
      return allowed.has(tag) ? `<${tag}>${inner}</${tag}>` : inner;
    };
    const pages: DocumentPage[] = [];
    for (const item of packageDoc.getElementsByTagNameNS("*", "itemref")) {
      const path = paths.get(item.getAttribute("idref"));
      if (!path) continue;
      const chapter = await xml(path);
      const body = chapter.getElementsByTagNameNS("*", "body")[0] ?? chapter.documentElement;
      for (const image of body.querySelectorAll("img, image")) {
        if (image.closest("script,style,iframe,object,head")) continue;
        const imageFile = imagePath(image, path);
        const ext = imageFile.split(".").pop()?.toLowerCase();
        if (!ext || !["png", "jpg", "jpeg", "gif", "webp"].includes(ext) || !entries.has(imageFile) || imageCache.has(imageFile)) continue;
        const data = await read(imageFile, "image/" + (ext === "jpg" ? "jpeg" : ext));
        const url = await new Promise<string>((resolve, reject) => {
          const file = new FileReader();
          file.onload = () => resolve(String(file.result));
          file.onerror = () => reject(file.error);
          file.readAsDataURL(data);
        });
        imageCache.set(imageFile, url);
      }
      pages.push({ title: chapter.querySelector("h1,h2,title")?.textContent?.trim() || `第 ${pages.length + 1} 章`, html: render(body, path) });
    }
    if (!pages.length) throw new Error("电子书中没有可阅读章节。");
    return pages;
  } finally {
    await reader.close();
  }
}

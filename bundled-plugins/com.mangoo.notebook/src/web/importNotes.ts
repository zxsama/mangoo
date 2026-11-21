import { parser as markdownParser } from "@lezer/markdown";
import type { SyntaxNode } from "@lezer/common";
import { noteArchiveManifest } from "./noteArchive";
import { rewriteCssImageUrls, rewriteDocumentCss } from "./cssImageUrls";
import { isImageFile } from "./editor/noteFormat";
import { BlobReader, BlobWriter, ZipReader, type FileEntry } from "@zip.js/zip.js/lib/zip-native.js";
import { CoreApiError, createCoreClient } from "@mangoo/web/plugin-host";
import type { NotebookNote, NoteRequestOptions } from "@mangoo/core-client";

const noteText = /\.(md|markdown|mdown|html?|txt)$/i;
const sidecar = /\.(css|js|mjs|png|jpe?g|gif|webp|bmp|svg|avif)$/i;
const imageMimeByExtension: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", svg: "image/svg+xml",
  avif: "image/avif", webp: "image/webp", bmp: "image/bmp", ico: "image/x-icon", tif: "image/tiff", tiff: "image/tiff",
};
export const importFormats =
  ".md,.markdown,.mdown,.html,.htm,.txt,.pdf,.epub,.doc,.docx,.docm,.xls,.xlsx,.xlsm,.ppt,.pptx,.pptm,.odt,.ods,.odp,.rtf,.csv,.json,.png,.jpg,.jpeg,.gif,.webp,.avif,.bmp,.svg,.css,.js,.mjs,.zip";
export function attachmentMarkup(name: string, id: string) {
  return `![${name.replace(/[\\\[\]\r\n]/g, " ")}](mangoo-attachment:${id})`;
}

function imageDataFile(src: string): File | undefined {
  const match = /^data:(image\/(?:png|jpeg|gif|webp|avif|bmp));base64,([\s\S]+)$/i.exec(src);
  if (!match) return;
  const raw = atob(match[2].replace(/\s/g, ""));
  return new File([Uint8Array.from(raw, (c) => c.charCodeAt(0))], `image.${match[1].split("/")[1]}`, {
    type: match[1],
  });
}

async function importArchive(
  file: File,
  options: NoteRequestOptions,
  folderId: string | null,
): Promise<NotebookNote | undefined> {
  const reader = new ZipReader(new BlobReader(file), { useWebWorkers: false, checkSignature: true });
  try {
    const entries = new Map<string, FileEntry>();
    for await (const entry of reader.getEntriesGenerator()) {
      if (entry.directory) continue;
      if (entries.has(entry.filename)) throw new Error("笔记归档含有重名文件。");
      entries.set(entry.filename, entry);
    }
    const entryBlob = async (path: string): Promise<Blob> => {
      const entry = entries.get(path);
      if (!entry) throw new Error("笔记归档缺少 " + path);
      return entry.getData(new BlobWriter(), { signal: options.signal });
    };
    if (!entries.has(noteArchiveManifest)) return;
    const manifest = JSON.parse(await (await entryBlob(noteArchiveManifest)).text());
    if (
      manifest.version !== 1 ||
      !["markdown", "html", "document"].includes(manifest.format) ||
      typeof manifest.title !== "string" ||
      typeof manifest.source !== "string" ||
      !Array.isArray(manifest.resources)
    )
      throw new Error("不支持的笔记归档格式。");
    const wanted = new Set([manifest.source]);
    for (const resource of manifest.resources) {
      if (
        !resource ||
        typeof resource.path !== "string" ||
        !resource.path.startsWith("attachments/") ||
        typeof resource.reference !== "string" ||
        !resource.reference ||
        typeof resource.name !== "string" ||
        typeof resource.mimeType !== "string"
      )
        throw new Error("笔记归档的附件目录不完整。");
      wanted.add(resource.path);
    }
    for (const path of wanted) if (!entries.has(path)) throw new Error("笔记归档缺少 " + path);
    const client = createCoreClient();
    const input = {
      title: manifest.title,
      format: manifest.format,
      styles: manifest.styles ?? {},
      markdown: "",
    };
    const created = await client.createNote({ ...input, folderId }, options);
    try {
      let markdown = await (await entryBlob(manifest.source)).text();
      for (const resource of manifest.resources) {
        const copy = new File([await entryBlob(resource.path)], resource.name, {
          type: resource.mimeType,
        });
        const imported = await client.uploadNoteAttachment(created.id, copy, options);
        markdown = markdown.replaceAll(resource.reference, "mangoo-attachment:" + imported.id);
      }
      return await client.updateNote(
        created.id,
        { ...input, markdown, expectedRevision: created.revision },
        options,
      );
    } catch (cause) {
      await client.deleteNote(created.id, created.revision, options).catch(() => {});
      throw cause;
    }
  } finally {
    await reader.close();
  }
}

export async function importNoteFiles(
  options: NoteRequestOptions,
  files: File[],
  progress: (filename: string) => void,
  folderId: string | null = null,
  onImported?: (note: NotebookNote) => void,
) {
  const client = createCoreClient();
  const consumed = new Set<File>();
  const names = new Map<string, File[]>();
  const paths = new Map<string, File>();
  const missing = new Set<string>();
  const directories = new Map<string, string>();
  const siblingKey = (parent: string | null, name: string) => (parent ?? "") + "/" + name.trim().normalize("NFC").toLowerCase();
  if (files.some(file => file.webkitRelativePath)) {
    const tree = await client.getNotebookTree(options);
    for (const folder of tree.folders) {
      const key = siblingKey(folder.parentId, folder.name);
      if (!directories.has(key)) directories.set(key, folder.id);
    }
  }
  async function parentFor(file: File) {
    let parent = folderId;
    if (!file.webkitRelativePath) return parent;
    for (const part of file.webkitRelativePath.split("/").slice(0, -1)) {
      const key = siblingKey(parent, part);
      let child = directories.get(key);
      if (!child) {
        try {
          const folder = await client.createNotebookFolder({ parentId: parent, name: part }, options);
          child = folder.id;
        } catch (cause) {
          if (!(cause instanceof CoreApiError) || (cause.body as { code?: string } | null)?.code !== "notebook_duplicate_folder_name") throw cause;
          // Another import created this directory after we read the tree.
          const fresh = await client.getNotebookTree(options);
          child = fresh.folders.find(folder => siblingKey(folder.parentId, folder.name) === key)?.id;
          if (!child) throw cause;
        }
        directories.set(key, child);
      }
      parent = child;
    }
    return parent;
  }
  let last: NotebookNote | undefined;
  for (const file of files) {
    const key = file.name.toLowerCase();
    names.set(key, [...(names.get(key) ?? []), file]);
    if (file.webkitRelativePath) paths.set(file.webkitRelativePath, file);
  }
  const sorted = [...files].sort((a, b) => Number(sidecar.test(a.name)) - Number(sidecar.test(b.name)));
  for (const file of sorted) {
    if (consumed.has(file)) continue;
    if (/\.(css|m?js)$/i.test(file.name) && files.length > 1) continue;
    if (
      file.webkitRelativePath &&
      !importFormats.split(",").includes("." + file.name.split(".").pop()?.toLowerCase())
    )
      continue;
    progress(file.name);
    // Only materialize directories for surviving notes/documents. Sidecars
    // consumed as attachments keep their relative links inside the note.
    const parentId = await parentFor(file);
    if (/\.zip$/i.test(file.name)) {
      const imported = await importArchive(file, options, parentId);
      if (imported) {
        last = imported;
        onImported?.(imported);
        continue;
      }
    }
    const html = /\.html?$/i.test(file.name);
    const document = !noteText.test(file.name);
    const title = document ? file.name : file.name.replace(/\.[^.]+$/, "") || file.name;
    const created = await client.createNote(
      { title, folderId: parentId, format: document ? "document" : html ? "html" : "markdown", markdown: "", styles: {} },
      options,
    );
    try {
      const uploads = new Map<File, string>();
      const embedded = new Map<string, string>();
      const find = (src: string, owner = file): File | undefined => {
        if (/^[a-z][\w+.-]*:|^\/\/|^#/i.test(src)) return;
        const base = "https://files.invalid/" + (owner.webkitRelativePath || owner.name).split("/").map(encodeURIComponent).join("/");
        const pathname = new URL(src.replaceAll("\\", "/"), base).pathname.slice(1);
        let path: string;
        try { path = decodeURIComponent(pathname.replace(/%(?![\da-f]{2})/gi, "%25")); }
        catch { return; }
        if (owner.webkitRelativePath) return paths.get(path);
        const byName = names.get(path.split("/").pop()!.toLowerCase());
        return byName?.length === 1 ? byName[0] : undefined;
      };
      const upload = async (linked: File) => {
        const existing = uploads.get(linked);
        if (existing) return existing;
        consumed.add(linked);
        const imageType = imageMimeByExtension[linked.name.split(".").at(-1)?.toLowerCase() ?? ""];
        const normalized = imageType && (!linked.type || linked.type === "application/octet-stream")
          ? new File([linked], linked.name, { type: imageType, lastModified: linked.lastModified }) : linked;
        const attachment = await client.uploadNoteAttachment(created.id, normalized, options);
        const ref = "mangoo-attachment:" + attachment.id;
        uploads.set(linked, ref);
        return ref;
      };
      const collect = async (src: string) => {
        if (embedded.has(src)) return embedded.get(src)!;
        const linked = imageDataFile(src) ?? find(src);
        if (!linked) {
          if (!src.startsWith("mangoo-attachment:"))
            missing.add(src.startsWith("data:") ? "内嵌图像" : src);
          return src;
        }
        const value = await upload(linked);
        if (src.startsWith("data:")) embedded.set(src, value);
        return value;
      };
      const collectCssImage = async (src: string, owner: File) => {
        if (src.startsWith("mangoo-attachment:")) return;
        const linked = find(src, owner);
        if (!linked) {
          if (isImageFile({ name: src.split(/[?#]/)[0], type: "" })) missing.add(src);
          return;
        }
        if (!isImageFile(linked)) return;
        const fragment = src.indexOf("#");
        return await upload(linked) + (fragment < 0 ? "" : src.slice(fragment));
      };
      let markdown: string;
      if (!noteText.test(file.name)) {
        const resource = await client.uploadNoteAttachment(created.id, file, options);
        markdown = attachmentMarkup(file.name, resource.id);
      } else if (html) {
        const doc = new DOMParser().parseFromString(await file.text(), "text/html");
        await rewriteDocumentCss(doc, (src) => collectCssImage(src, file));
        for (const element of doc.querySelectorAll<HTMLImageElement>("img[src]")) {
          element.setAttribute("src", await collect(element.getAttribute("src")!));
          element.removeAttribute("srcset");
        }
        for (const element of doc.querySelectorAll('script[src],link[rel~="stylesheet"][href]')) {
          const script = element.tagName === "SCRIPT";
          const source = element.getAttribute(script ? "src" : "href")!;
          const linked = find(source);
          if (!linked) {
            missing.add(source);
            continue;
          }
          consumed.add(linked);
          const replacement = doc.createElement(script ? "script" : "style");
          if (script && element.getAttribute("type"))
            replacement.setAttribute("type", element.getAttribute("type")!);
          const contents = await linked.text();
          if (!script && element.hasAttribute("media")) replacement.setAttribute("media", element.getAttribute("media")!);
          replacement.textContent = script ? contents : await rewriteCssImageUrls(contents, (src) => collectCssImage(src, linked));
          element.replaceWith(replacement);
        }
        markdown = "<!doctype html>\n" + doc.documentElement.outerHTML;
      } else {
        markdown = await file.text();
        const urls: SyntaxNode[] = [];
        const referenceIds = new Set<string>();
        const definitions = new Map<string, SyntaxNode>();
        const labelKey = (text: string) => text.trim().replace(/\s+/g, " ").toLowerCase();
        if (markdown.includes("![")) markdownParser.parse(markdown).iterate({ enter(cursor) {
          const node = cursor.node;
          if (node.name === "Image") {
            const url = node.getChild("URL");
            if (url) { urls.push(url); return false; }
            const label = node.getChild("LinkLabel");
            const marks = node.getChildren("LinkMark");
            const alt = marks.length > 1 ? markdown.slice(marks[0].to, marks[1].from) : "";
            referenceIds.add(labelKey((label && markdown.slice(label.from + 1, label.to - 1)) || alt));
          } else if (node.name === "LinkReference") {
            const label = node.getChild("LinkLabel"), url = node.getChild("URL");
            if (label && url) {
              const key = labelKey(markdown.slice(label.from + 1, label.to - 1));
              if (!definitions.has(key)) definitions.set(key, url);
            }
          }
        } });
        for (const key of referenceIds) {
          const definition = definitions.get(key);
          if (definition) urls.push(definition);
        }
        for (const url of urls.sort((a, b) => b.from - a.from)) {
          const original = markdown.slice(url.from, url.to);
          const value = await collect(original.replace(/^<|>$/g, "").replace(/\\([()])/g, "$1"));
          markdown = markdown.slice(0, url.from) + value + markdown.slice(url.to);
        }
      }
      last = await client.updateNote(
        created.id,
        { title, format: created.format, markdown, styles: {}, expectedRevision: created.revision },
        options,
      );
      onImported?.(last);
    } catch (cause) {
      await client.deleteNote(created.id, created.revision, options).catch(() => {});
      throw new Error(
        `${file.name} 导入失败：${cause instanceof Error ? cause.message : String(cause)}`,
      );
    }
  }
  return { last, missing: [...missing] };
}

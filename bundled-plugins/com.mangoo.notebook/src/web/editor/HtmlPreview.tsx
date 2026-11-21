import { useDeferredValue, useEffect, useState, type ReactNode } from "react";
import { attachmentId, elementStylesheet, type NoteEditorStyles } from "./noteFormat.ts";
import { imageHelp } from "./clipboard.ts";
import { rewriteDocumentCss } from "../cssImageUrls.ts";

export interface AttachmentReader {
  resolveAttachment(id: string): Promise<Blob>;
  renderAttachment?(id: string, filename: string): ReactNode;
}

function dataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

interface Page {
  source: string;
  attachments: Map<string, string>;
  missing: string[];
}

async function buildNotebookPage(source: string, interactive: boolean, stylesheet: string,
  resolve: AttachmentReader["resolveAttachment"], signal: AbortSignal): Promise<Page> {
  // The policy precedes user content even while the detached document is being parsed.
  const policy = `default-src 'none'; script-src ${interactive ? "'unsafe-inline'" : "'none'"}; style-src 'unsafe-inline'; img-src data:; media-src data:; font-src data:; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'`;
  const doc = new DOMParser().parseFromString(
    `<!doctype html><meta http-equiv="Content-Security-Policy" content="${policy}">${source}`, "text/html");
  doc.querySelectorAll('meta[http-equiv="refresh" i], base, iframe, frame, object, embed, link[rel~="preconnect" i], link[rel~="dns-prefetch" i]').forEach(node => node.remove());
  const css = doc.createElement("style");
  css.textContent = stylesheet;
  doc.head.append(css);
  const attachments = new Map<string, string>();
  doc.querySelectorAll("a,area").forEach(link => {
    const href = link.getAttribute("href") ?? "";
    const id = attachmentId(href);
    if (id) attachments.set(id, link.textContent?.trim() || id);
    if (!href.startsWith("#")) link.removeAttribute("href");
    link.removeAttribute("target");
  });
  const images = new Map<string, Promise<string>>();
  const missing: string[] = [];
  const readImage = (id: string) => {
    const cached = images.get(id);
    if (cached) return cached;
    const load = resolve(id).then(blob => {
      if (!blob.type.startsWith("image/")) throw new Error("Expected image");
      return dataUrl(blob);
    });
    images.set(id, load);
    return load;
  };
  await rewriteDocumentCss(doc, async (src) => {
    const fragment = src.indexOf("#");
    const id = attachmentId(fragment < 0 ? src : src.slice(0, fragment));
    if (!id) return;
    signal.throwIfAborted();
    try {
      return await readImage(id) + (fragment < 0 ? "" : src.slice(fragment));
    } catch {
      missing.push(id);
      return "data:,";
    }
  });
  await Promise.all(Array.from(doc.querySelectorAll("img"), async img => {
    const src = img.getAttribute("src") ?? "";
    const id = attachmentId(src);
    img.removeAttribute("srcset");
    if (!id) {
      if (src && !/^data:image\//i.test(src)) {
        missing.push(img.alt || src);
        img.removeAttribute("src");
      }
      return;
    }
    try {
      const url = await readImage(id);
      if (!signal.aborted) img.setAttribute("src", url);
    } catch {
      img.removeAttribute("src");
      missing.push(img.alt || id);
    }
  }));
  return { source: "<!doctype html>\n" + doc.documentElement.outerHTML, attachments, missing };
}

interface Props extends AttachmentReader {
  source: string;
  interactive: boolean;
  styles: NoteEditorStyles;
  zh: boolean;
}

export function HtmlPreview({ source, interactive, styles, resolveAttachment, renderAttachment, zh }: Props) {
  const [page, setPage] = useState<Page>();
  const [error, setError] = useState("");
  const deferredSource = useDeferredValue(source);
  const stylesheet = elementStylesheet(styles, true);
  useEffect(() => {
    const controller = new AbortController();
    // A burst of source edits should produce one frame navigation.
    const timer = window.setTimeout(() => {
      void buildNotebookPage(deferredSource, interactive, stylesheet, resolveAttachment, controller.signal)
        .then(result => { if (!controller.signal.aborted) { setPage(result); setError(""); } })
        .catch(reason => { if (!controller.signal.aborted) setError(String(reason)); });
    }, 120);
    return () => { controller.abort(); window.clearTimeout(timer); };
  }, [deferredSource, interactive, stylesheet, resolveAttachment]);
  return <div className="nb-html-output">
    {page && <iframe name="mangoo-notebook-preview" key={String(interactive)} title={zh ? "HTML 网页预览" : "HTML page preview"}
      sandbox={interactive ? "allow-scripts" : ""} referrerPolicy="no-referrer"
      srcDoc={page.source} />}
    {error && <p role="alert">{error}</p>}
    {page?.missing.length ? <p role="status">{page.missing.join(", ")} · {imageHelp(zh)}</p> : null}
    {page && renderAttachment && Array.from(page.attachments, ([id, filename]) => <div key={id}>{renderAttachment(id, filename)}</div>)}
  </div>;
}

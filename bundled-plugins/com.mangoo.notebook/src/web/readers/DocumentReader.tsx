import { lazy, Suspense, useEffect, useState } from "react";
import { icons } from "@mangoo/web/plugin-host";
import { escapeMarkup, readerDocument, type DocumentPage } from "./documentPages";
import { readerAssetUrl } from "./readerAssetUrl";
import { readEpub } from "./epub";

const PdfReader = lazy(() => import("./PdfReader"));

async function readPages(blob: Blob, name: string, signal: AbortSignal): Promise<DocumentPage[]> {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "epub") return readEpub(blob, signal);
  if (["html", "htm", "md", "markdown"].includes(ext)) {
    const text = await blob.text();
    const source = ["md", "markdown"].includes(ext) ? await (await import("marked")).marked.parse(text) : text;
    const { default: purify } = await import("dompurify");
    const html = purify.sanitize(source, { FORBID_TAGS: ["form", "a", "base", "style", "link", "svg", "math"], FORBID_ATTR: ["style", "srcset"] });
    return [{ title: "正文", html }];
  }
  if (["txt", "csv", "json", "log"].includes(ext)) return [{ title: "正文", html: "<pre>" + escapeMarkup(await blob.text()) + "</pre>" }];
  if (!/^(docx?|xlsx?|pptx?|od[pts]|rtf|xlsm|docm|pptm)$/.test(ext)) return [];
  const data = await blob.arrayBuffer();
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./office.worker.ts", import.meta.url));
    const stop = () => { worker.terminate(); clearTimeout(timeout); signal.removeEventListener("abort", abort); };
    const abort = () => { stop(); reject(new Error("已关闭")); };
    const timeout = setTimeout(() => { stop(); reject(new Error("文件解析超时，可下载原件。")); }, 45000);
    signal.addEventListener("abort", abort, { once: true });
    worker.onerror = (event) => { stop(); reject(new Error(event.message)); };
    worker.onmessage = event => {
      stop();
      if (event.data.error) reject(new Error(event.data.error));
      else resolve(event.data.pages);
    };
    worker.postMessage({ parserUrl: readerAssetUrl("officeparser.browser.js"), data }, [data]);
  });
}

export function DocumentReader({ name, load, view }: { name: string; load: () => Promise<Blob>; view?: { page?: number } }) {
  const [blob, setBlob] = useState<Blob>();
  const [url, setUrl] = useState("");
  const [pages, setPages] = useState<DocumentPage[]>();
  const [page, setPage] = useState(view?.page ?? 0);
  const [error, setError] = useState("");
  const isPdf = /\.pdf$/i.test(name);
  const image = /\.(png|jpe?g|gif|webp|avif|bmp|svg)$/i.test(name);
  useEffect(() => {
    const abort = new AbortController();
    let objectUrl = "";
    setError(""); setPages(undefined); setPage(view?.page ?? 0); setBlob(undefined); setUrl("");
    void load().then(async data => {
      if (abort.signal.aborted) return;
      objectUrl = URL.createObjectURL(data);
      setBlob(data); setUrl(objectUrl);
      if (!isPdf && !image) {
        const result = await readPages(data, name, abort.signal);
        if (!abort.signal.aborted) {
          setPage(Math.min(view?.page ?? 0, Math.max(0, result.length - 1)));
          setPages(result);
        }
      }
    }).catch(cause => { if (!abort.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { abort.abort(); if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [load, name, isPdf, image, view]);
  return <section className="notebook-reader" aria-label={name}>
    <header><icons.FileText size={16} /><strong>{name}</strong>{url && <a href={url} download={name} title="下载原文件" aria-label={`下载 ${name}`}><icons.Download size={16} /></a>}</header>
    {error ? <p role="alert">预览失败 · {error}</p> : !blob ? <p role="status">正在读取附件…</p> :
      image ? <img src={url} alt={name} /> : isPdf ? <Suspense fallback={<p>正在打开 PDF…</p>}><PdfReader blob={blob} view={view} /></Suspense> :
      !pages ? <p role="status">正在解析文档…</p> : !pages.length ? <p>此格式可下载后查看。</p> :
      <>{pages.length > 1 && <label className="notebook-reader-nav">页面<select aria-label="选择页面" value={page} onChange={e => {
        const selected = Number(e.target.value); if (view) view.page = selected; setPage(selected);
      }}>{pages.map((item, i) => <option value={i} key={i}>{item.title}</option>)}</select></label>}
      <iframe sandbox="" name="mangoo-notebook-preview" title={name} srcDoc={readerDocument(pages[page].html)} /></>}
  </section>;
}

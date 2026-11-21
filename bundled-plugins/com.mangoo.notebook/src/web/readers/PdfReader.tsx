import { useEffect, useRef, useState } from "react";
import { getDocument, GlobalWorkerOptions, type PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { icons } from "@mangoo/web/plugin-host";
import { readerAssetUrl } from "./readerAssetUrl";

GlobalWorkerOptions.workerSrc = workerUrl;

export default function PdfReader({ blob, view }: { blob: Blob; view?: { page?: number } }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [pdf, setPdf] = useState<PDFDocumentProxy>();
  const [page, setPage] = useState(view?.page ?? 1);
  const navigate = (next: number) => { if (view) view.page = next; setPage(next); };
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    let task: ReturnType<typeof getDocument> | undefined;
    void blob.arrayBuffer().then(async data => {
      if (disposed) return;
      task = getDocument({ data, useSystemFonts: true, useWasm: false, useWorkerFetch: true,
        cMapUrl: readerAssetUrl("cmaps/"), cMapPacked: true, standardFontDataUrl: readerAssetUrl("standard_fonts/"),
        iccUrl: readerAssetUrl("iccs/"), wasmUrl: readerAssetUrl("wasm/") });
      const result = await task.promise;
      if (!disposed) { setPage(Math.min(view?.page ?? 1, result.numPages)); setPdf(result); }
    }).catch(cause => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; void task?.destroy(); };
  }, [blob, view]);
  useEffect(() => {
    if (!pdf) return;
    let disposed = false;
    let cancel: (() => void) | undefined;
    void pdf.getPage(page).then(async model => {
      const element = canvas.current;
      if (!element || disposed) return;
      const bounds = model.getViewport({ scale: 1 });
      const viewport = model.getViewport({ scale: Math.min(2, 1800 / Math.max(bounds.width, bounds.height)) });
      element.width = Math.ceil(viewport.width);
      element.height = Math.ceil(viewport.height);
      const task = model.render({ canvas: element, viewport });
      cancel = () => task.cancel();
      await task.promise;
    }).catch(cause => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; cancel?.(); };
  }, [pdf, page]);
  return <div className="notebook-pdf">{error ? <p role="alert">{error}</p> : <><div className="notebook-reader-nav">
    <button type="button" title="上一页" aria-label="上一页" disabled={page <= 1} onClick={() => navigate(page - 1)}><icons.ChevronLeft size={16} /></button>
    <span aria-live="polite">{page} / {pdf?.numPages ?? "…"}</span>
    <button type="button" title="下一页" aria-label="下一页" disabled={!pdf || page >= pdf.numPages} onClick={() => navigate(page + 1)}><icons.ChevronRight size={16} /></button>
  </div><canvas ref={canvas} aria-label={`PDF 第 ${page} 页`} /></>}</div>;
}

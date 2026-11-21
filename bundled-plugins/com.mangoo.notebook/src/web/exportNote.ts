import { noteArchiveManifest } from "./noteArchive";
import { BlobReader, BlobWriter, TextReader, ZipWriter } from "@zip.js/zip.js/lib/zip-native.js";
import type { NotebookNote, NoteRequestOptions } from "@mangoo/core-client";
import { createCoreClient } from "@mangoo/web/plugin-host";

function filename(value: string) {
  return (
    value
      .replace(/[<>:"/\\|?*\u0000-\u001F]/g, "_")
      .replace(/[. ]+$/, "")
      .slice(0, 150) || "笔记"
  );
}

export async function exportNote(note: NotebookNote, options: NoteRequestOptions) {
  const html = note.format === "html";
  const stem = filename(note.title);
  const name = stem + (html ? ".html" : ".md");
  const withFiles = note.format === "document" || note.attachments.length > 0 || Object.keys(note.styles).length > 0;
  const archive = withFiles ? new ZipWriter(new BlobWriter("application/zip"), { level: 0, useWebWorkers: false }) : null;
  const resources: { path: string; reference: string; name: string; mimeType: string }[] = [];
  let source = note.markdown;
  for (const file of note.attachments) {
    const ref = "mangoo-attachment:" + file.id;
    // Separate folders preserve colliding original filenames on Windows.
    const path = "attachments/" + file.id + "/" + filename(file.name);
    const blob = await createCoreClient().getNoteAttachment(note.id, file.id, options);
    await archive!.add(path, new BlobReader(blob), { signal: options.signal });
    const portable = path
      .split("/")
      .map((part) =>
        encodeURIComponent(part).replace(/[()']/g, (c) => "%" + c.charCodeAt(0).toString(16)),
      )
      .join("/");
    resources.push({ path, reference: portable, name: file.name, mimeType: file.mimeType });
    source = source.replaceAll(ref, portable);
  }
  let blob: Blob;
  if (archive) {
    await archive.add(name, new TextReader(source), { signal: options.signal });
    const manifest = { version: 1, title: note.title, format: note.format, source: name, styles: note.styles, resources };
    await archive.add(noteArchiveManifest, new TextReader(JSON.stringify(manifest, null, 2)), { signal: options.signal });
    blob = await archive.close();
  } else {
    blob = new Blob([source], { type: html ? "text/html;charset=utf-8" : "text/markdown;charset=utf-8" });
  }
  const href = URL.createObjectURL(blob);
  const download = document.createElement("a");
  download.href = href;
  download.download = withFiles ? stem + ".zip" : name;
  download.click();
  window.setTimeout(() => URL.revokeObjectURL(href), 30000);
}

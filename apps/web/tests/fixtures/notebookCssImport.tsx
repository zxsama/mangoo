import { createRoot } from "react-dom/client";
import type { NotebookFolder, NotebookNote } from "@mangoo/core-client";
import { importNoteFiles } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/importNotes";
import { exportNote } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/exportNote";
import { HtmlPreview } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/editor/HtmlPreview";

function ensure(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
function file(path: string, body: BlobPart, type: string) {
  const result = new File([body], path.split("/").at(-1)!, { type });
  Object.defineProperty(result, "webkitRelativePath", { value: "site/" + path });
  return result;
}

export async function testCssImportAndArchive() {
  const nativeFetch = window.fetch;
  const nativeClick = HTMLAnchorElement.prototype.click;
  const nativeObjectUrl = URL.createObjectURL;
  const desktop = window.mangooDesktop;
  const stored = new Map<string, File>();
  const notes = new Map<string, NotebookNote>();
  const folders: NotebookFolder[] = [{ id: "parent", parentId: null, name: "Selected", revision: 1 }];
  const parents = new Map<string, string>();
  const options = { libraryId: "css-fixture", contextRevision: 1 };
  const downloads = new Map<string, Blob>();
  let exported: Blob | undefined;
  const json = (data: unknown) => new Response(JSON.stringify(data), { headers: { "content-type": "application/json" } });
  window.mangooDesktop = { ...desktop, coreUrl: location.origin + "/css-fixture" };
  window.fetch = async (input, init) => {
    const url = new URL(String(input));
    if (!url.pathname.startsWith("/css-fixture/")) return nativeFetch(input, init);
    if (url.pathname.endsWith("/tree")) return json({ folders, noteParents: Object.fromEntries(parents) });
    if (url.pathname.endsWith("/folders")) {
      const folder = { ...JSON.parse(String(init?.body)), id: "dir-" + folders.length, revision: 1 };
      folders.push(folder);
      return json(folder);
    }
    const parts = url.pathname.split("/notes")[1].split("/").slice(1);
    if (!parts.length) {
      const id = "note-" + (notes.size + 1);
      const note = { ...JSON.parse(String(init?.body)), id, revision: 1, attachments: [] };
      if (note.folderId) parents.set(id, note.folderId);
      notes.set(id, note); return json(note);
    }
    const note = notes.get(parts[0])!;
    if (parts[1] === "attachments") {
      if (parts[2]) {
        ensure(note.attachments.some(item => item.id === parts[2]), "Cross-note attachment read");
        return new Response(stored.get(parts[2])!);
      }
      const id = "asset-" + (stored.size + 1), image = init!.body as File;
      stored.set(id, image);
      const info = { id, name: image.name, size: image.size, mimeType: image.type };
      note.attachments.push(info); return json(info);
    }
    Object.assign(note, JSON.parse(String(init?.body)));
    return json(note);
  };
  URL.createObjectURL = function(blob) {
    const url = nativeObjectUrl.call(URL, blob);
    if (blob instanceof Blob) downloads.set(url, blob);
    return url;
  };
  HTMLAnchorElement.prototype.click = function() { exported = downloads.get(this.href); };
  const host = document.body.appendChild(document.createElement("div"));
  const root = createRoot(host);
  try {
    const red = '<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>';
    const html = '<link rel="stylesheet" href="theme/nested/site.css"><style>#a{background-image:url("images/same.svg")}</style>'
      + '<div id="a"></div><div id="b" style="background-image:url(images/same.svg)"></div><div id="c"></div>'
      + '<script>onload=()=>Promise.all(["a","b","c"].map(async id=>{const el=document.getElementById(id), match=getComputedStyle(el).backgroundImage.match(/^url\\("(.+)"\\)$/);const im=new Image();im.src=match[1];await im.decode();const canvas=document.createElement("canvas"),ctx=canvas.getContext("2d");ctx.drawImage(im,0,0);return Array.from(ctx.getImageData(0,0,1,1).data).join(",")})).then(colors=>parent.postMessage({cssTest:colors},"*")).catch(e=>parent.postMessage({cssTest:String(e)},"*"))</script>';
    const stylesheet = "/* url(missing.png) */ #c{background-image:url('../images/same.svg')} #c::after{content:'url(missing.png)'} :root{--embedded:url(data:image/png;base64,AA==)}";
    const imported = await importNoteFiles(options, [
      file("index.html",html,"text/html"), file("images/same.svg",red,"image/svg+xml"),
      file("theme/nested/site.css",stylesheet,"text/css"), file("theme/images/same.svg",red.replace('fill="red"', 'fill="blue"'),"")
    ], () => {}, "parent");
    ensure(imported.last && !imported.missing.length && notes.size === 1, "CSS sidecar collection");
    const first = imported.last;
    ensure(folders.length === 2 && folders[1].name === "site" && folders[1].parentId === "parent" &&
      parents.get(first.id) === folders[1].id, "Picker root must be nested below the selected folder; attachment-only directories stay implicit");
    const nested = await importNoteFiles(options, [file("guides/A.md","one","text/markdown"), file("guides/B.md","two","text/markdown")], () => {}, "parent");
    const retry = await importNoteFiles(options, [file("guides/C.md","three","text/markdown")], () => {}, "parent");
    ensure(nested.last && retry.last && folders.map(folder => folder.name).join("/") === "Selected/site/guides" &&
      folders[2].parentId === folders[1].id && [nested.last, retry.last].every(note => parents.get(note.id) === folders[2].id),
      "Sibling imports and repeat imports must reuse the same nested notebook directories");
    ensure(first.attachments.length === 2 && first.markdown.includes("/* url(missing.png) */") && first.markdown.includes("url(data:image/png;base64,AA==)"), "CSS image deduplication and literal preservation");
    await exportNote(first, options);
    ensure(exported, "Missing HTML note archive");
    const restored = await importNoteFiles(options,[new File([exported],"page.zip",{type:"application/zip"})],() => {});
    ensure(restored.last?.attachments.length === 2, "Restored image count");
    const note = restored.last;
    ensure(first.attachments.every(old => !note.markdown.includes("mangoo-attachment:" + old.id)), "Old note IDs retained");
    const colors = new Promise<unknown>((resolve,reject) => {
      const cleanup = () => { clearTimeout(timer); window.removeEventListener("message", listener); };
      const listener = (event: MessageEvent) => {
        if (event.source === host.querySelector("iframe")?.contentWindow && event.origin === "null" && event.data?.cssTest) {
          cleanup(); resolve(event.data.cssTest);
        }
      };
      const timer = setTimeout(() => { cleanup(); reject(new Error("CSS images did not paint in isolated page")); },5000);
      window.addEventListener("message",listener);
    });
    root.render(<HtmlPreview source={note.markdown} styles={{}} interactive zh resolveAttachment={async id => {
      ensure(note.attachments.some(item => item.id === id), "Wrong owner in preview");
      return stored.get(id)!;
    }} />);
    const actual = await colors;
    ensure(JSON.stringify(actual) === '["255,0,0,255","255,0,0,255","0,0,255,255"]', "Incorrect rendered CSS colors " + JSON.stringify(actual));
  } finally {
    root.unmount(); host.remove(); HTMLAnchorElement.prototype.click = nativeClick; URL.createObjectURL = nativeObjectUrl;
    window.fetch = nativeFetch; window.mangooDesktop = desktop;
  }
}

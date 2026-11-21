import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { NotebookFolder, NotebookTree, NoteSummary } from "@mangoo/core-client";
import { NotebookIndex } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/NotebookIndex";

export async function testNotebookTreeMoves() {
  const host = document.body.appendChild(document.createElement("div"));
  host.style.cssText = "position:fixed;top:0;left:0;width:280px;height:350px;display:flex";
  const root = createRoot(host);
  const options = { libraryId: "tree-fixture", contextRevision: 1 };
  const folder = (id: string, parentId: string | null): NotebookFolder => ({ id, name: id, parentId, revision: 1 });
  const source: NotebookTree = { folders: [folder("A", null), folder("B", "A"), folder("C", null)], noteParents: {} };
  const notes: NoteSummary[] = [{ id: "note", title: "Sample", format: "markdown", createdAt: 1, updatedAt: 1, revision: 1 }];
  const nativeFetch = window.fetch;
  const desktop = window.mangooDesktop;
  window.mangooDesktop = { ...desktop, coreUrl: location.origin + "/tree-fixture" };
  let mutations = 0;
  window.fetch = async (input, init) => {
    const path = new URL(String(input)).pathname;
    if (!path.includes("/tree-fixture/")) return nativeFetch(input, init);
    if (init?.method === "DELETE" && path.endsWith("/folders/A")) {
      return Response.json({ code: "note_revision_conflict", error: "请先移出文件夹中的笔记和子文件夹。" }, { status: 409 });
    }
    const body = JSON.parse(String(init?.body));
    if (path.endsWith("/notes/note/folder")) {
      if ((source.noteParents.note ?? null) !== body.expectedParentId) throw new Error("Wrong previous note parent");
      if (body.parentId) source.noteParents.note = body.parentId; else delete source.noteParents.note;
      mutations++;
      return new Response(null, { status: 204 });
    }
    if (path.endsWith("/folders/A")) {
      const item = source.folders[0];
      if (body.expectedRevision !== item.revision) throw new Error("Wrong folder revision");
      item.parentId = body.parentId; item.revision++;
      mutations++;
      return Response.json(item);
    }
    throw new Error("Unexpected tree mutation: " + path);
  };
  const query = (selector: string) => host.querySelector<HTMLElement>(selector);
  const wait = async (condition: () => unknown, label: string) => {
    for (let i = 0; i < 100; i++) {
      if (condition()) return;
      await new Promise(done => setTimeout(done, 15));
    }
    throw new Error(label);
  };
  function Fixture() {
    const [tree, setTree] = useState(() => structuredClone(source));
    const [folderId, selectFolder] = useState<string | null>(null);
    return <NotebookIndex options={options} tree={tree} rows={notes} busy={false} search="" choose={() => {}}
      folderId={folderId} selectFolder={selectFolder} refresh={async () => setTree(structuredClone(source))} />;
  }
  const drag = async (origin: string, target: string, expected: boolean) => {
    const from = query(origin)!, to = query(target)!;
    const dataTransfer = new DataTransfer();
    const fire = (el: HTMLElement, type: string) => el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer }));
    fire(from, "dragstart");
    if ((!fire(to, "dragover")) !== expected) throw new Error("Unexpected drop admission for " + origin + " -> " + target);
    if (expected) {
      await wait(() => to.dataset.dropTarget === "true", "Missing visible drop destination");
      fire(to, "drop");
    }
    fire(from, "dragend");
  };
  try {
    root.render(<Fixture />);
    const a = '[data-folder-id="A"]', b = '[data-folder-id="B"]', c = '[data-folder-id="C"]', n = '[data-note-id="note"]';
    await wait(() => query(n), "Tree did not render");
    const key = (el: HTMLElement, key: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    query(a)!.focus();
    key(query(a)!, "ArrowRight");
    await wait(() => query(b), "Nested folder did not expand");
    key(query(a)!, "ArrowRight");
    await wait(() => document.activeElement === query(b), "Keyboard did not enter child");
    key(query(b)!, "ArrowLeft");
    await wait(() => document.activeElement === query(a), "Keyboard did not return to parent");
    key(query(a)!, "F2");
    await wait(() => query("input[aria-label='文件夹名称']") === document.activeElement, "Keyboard rename did not focus the input");
    key(document.activeElement as HTMLElement, "Escape");
    await drag(a, b, false);
    await drag(b, a, false);
    await drag(n, b, true);
    await wait(() => source.noteParents.note === "B" && query(n)?.getAttribute("aria-level") === "3", "Note did not move into nested folder");
    await drag(a, c, true);
    await wait(() => query(n)?.getAttribute("aria-level") === "4", "Moving a folder lost its descendants");
    await drag(n, "[aria-label='选择根目录']", true);
    await wait(() => query(n)?.getAttribute("aria-level") === "1" && !source.noteParents.note, "Root did not accept the note");
    if (mutations !== 3) throw new Error("Unexpected drag mutation count " + mutations);
    query(a)!.click();
    await wait(() => query('[aria-label="删除空文件夹"]'), "Folder actions did not open");
    query('[aria-label="删除空文件夹"]')!.click();
    await wait(() => query('[aria-label="确认删除空文件夹"]'), "Delete confirmation did not open");
    query('[aria-label="确认删除空文件夹"]')!.click();
    await wait(() => query('[role="alert"]')?.textContent === "请先移出文件夹中的笔记和子文件夹。", "Directory error hid the actionable Core explanation");
  } finally {
    root.unmount(); host.remove();
    window.fetch = nativeFetch; window.mangooDesktop = desktop;
    localStorage.removeItem("mangoo.notebook.expanded:" + options.libraryId);
  }
}

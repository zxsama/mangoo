import { useState } from "react";
import { createRoot } from "react-dom/client";
import { EditorView } from "@codemirror/view";
import { OverlayProvider } from "@mangoo/web/plugin-host";
import { NoteEditor, type NoteTabView } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/editor/NoteEditor.tsx";
import { NotebookTabStrip } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/NotebookTabStrip";
import { NotebookSettings } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/NotebookSettings";
import { readTabs, reorderTab, retainTabs, selectTab, writeTabs } from "../../../../bundled-plugins/com.mangoo.notebook/src/web/notebookTabs";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
let pageError = "";
window.addEventListener("error", event => { pageError = event.error?.stack ?? event.message; });
async function until(check: () => boolean, label: string) {
  for (let i = 0; i < 120; i++) { if (check()) return; await wait(25); }
  throw new Error(label + (pageError ? "\n" + pageError : ""));
}
const noteText = "# Field notes\n\nThis is **bold**.\n\n- [ ] Check\n\n|Left|Right|\n|---|---|\n|A|B|\n";
const htmlText = '<h1 id="status">Page</h1><script>parent.postMessage("notebook-test-ready","*")</script>';
const image = new File(['<svg xmlns="http://www.w3.org/2000/svg" width="6" height="6"><rect width="6" height="6"/></svg>'], "drawing.svg", { type: "image/svg+xml" });
const documents = new Map<string, Blob>();
let saved = "";
let styleCommits = 0;
const markdownView: NoteTabView = {};
function Fixture({ format, initial }: { format: "html" | "markdown"; initial?: string }) {
  const [value, setValue] = useState(initial ?? (format === "html" ? htmlText : noteText));
  const [styles, setStyles] = useState({});
  return <OverlayProvider><NoteEditor format={format} tabView={format === "markdown" ? markdownView : undefined} value={value} styles={styles} language="en"
    onChange={v => { saved = v; setValue(v); }} onStylesChange={next => { styleCommits++; setStyles(next); }}
    addAttachment={async file => { await wait(40); const id = "sample-" + documents.size; documents.set(id, file); return id; }}
    resolveAttachment={async id => { const file = documents.get(id); if (!file) throw new Error("Missing"); return file; }}
    renderAttachment={(_id, name) => <span data-document-view>{name}</span>} /></OverlayProvider>;
}
const root = createRoot(document.querySelector("#root")!);
const query = <T extends HTMLElement>(selector: string) => {
  const el = document.querySelector<T>(selector);
  if (!el) throw new Error("Missing " + selector);
  return el;
};
function setHex(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}
async function testDefaultColorPreview() {
  const key = "mangoo.notebook.preferences.v1";
  const previous = localStorage.getItem(key);
  root.render(<OverlayProvider><NotebookSettings zh={false} /></OverlayProvider>);
  await until(() => !!document.querySelector(".nb-style-sample"), "Default element sample missing");
  const sample = query(".nb-style-sample");
  const original = getComputedStyle(sample).backgroundColor;
  const pick = async () => {
    query("button[aria-label='Background']").click();
    await until(() => document.activeElement?.getAttribute("aria-label") === "HEX color", "Default color editor did not open");
    setHex(query<HTMLInputElement>(".color-picker-popup input[aria-label='HEX color']"), "#345678");
    await until(() => getComputedStyle(sample).backgroundColor === "rgb(52, 86, 120)", "Sample did not preview default color");
    if (localStorage.getItem(key) !== previous) throw Error("Preview wrote notebook defaults");
  };
  await pick();
  window.dispatchEvent(new Event("blur"));
  await until(() => getComputedStyle(sample).backgroundColor === original, "Window blur did not undo default preview");
  await pick();
  query(".color-picker-confirm").click();
  await until(() => JSON.parse(localStorage.getItem(key) ?? "{}").elementStyles?.body?.background === "#345678", "Default color was not saved");
  if (previous === null) localStorage.removeItem(key); else localStorage.setItem(key, previous);
}
const mode = (label: string) => query<HTMLButtonElement>(`.nb-mode-switch button[aria-label="${label}"]`).click();
const tabNotes = ["A", "B", "C"].map(id => ({ id, title: id, format: "markdown" as const }));
function TabFixture() {
  const [layout, update] = useState({ ids: tabNotes.map(note => note.id), activeId: "B" as string | null });
  return <NotebookTabStrip tabs={layout.ids.map(id => tabNotes.find(note => note.id === id)!)} activeId={layout.activeId}
    disabled={false} select={id => update(selectTab(layout, id))} close={id => update(retainTabs(layout, open => open !== id))}
    create={() => {}} reorder={(id, target, side) => {
      const next = reorderTab(layout, id, target, side); writeTabs("drag-fixture", next); update(next);
    }} />;
}
async function testTabActions() {
  root.render(<TabFixture />);
  await until(() => !!document.getElementById("nb-tab-C"), "Tab strip not mounted");
  const tab = (id: string) => query('[data-tab-id="' + id + '"]');
  const button = (id: string) => tab(id).querySelector<HTMLButtonElement>('[role="tab"]')!;
  const order = () => Array.from(document.querySelectorAll<HTMLElement>("[data-tab-id]"), el => el.dataset.tabId).join("");
  const key = (id: string, key: string, shiftKey = true) => button(id).dispatchEvent(new KeyboardEvent("keydown",
    { key, shiftKey, ctrlKey: true, bubbles: true, cancelable: true }));
  const transfer = new DataTransfer();
  const drag = (el: HTMLElement, type: string) => el.dispatchEvent(new DragEvent(type, {
    bubbles: true, cancelable: true, dataTransfer: transfer, clientX: el.getBoundingClientRect().right - 1,
  }));
  transfer.setData("application/x-mangoo-notebook-tree-item", "A");
  if (!drag(tab("C"), "dragover")) throw Error("Tree transfer entered the tab strip");
  transfer.clearData();
  drag(button("A"), "dragstart");
  if (drag(tab("C"), "dragover")) throw Error("Tab transfer rejected");
  await until(() => tab("C").dataset.dropSide === "after", "Insertion position missing");
  drag(tab("C"), "drop");
  await until(() => order() === "BCA" && !document.querySelector("[data-drop-side]"), "Drop did not reorder");
  if (button("B").getAttribute("aria-selected") !== "true" || readTabs("drag-fixture", tabNotes).ids.join("") !== "BCA")
    throw Error("Reordering changed the active document or failed to persist");
  button("B").focus(); key("B", "PageDown");
  await until(() => order() === "CBA" && document.activeElement === button("B"), "Keyboard move lost tab focus");
  key("B", "PageUp"); await until(() => order() === "BCA", "Reverse keyboard reorder failed");
  if (!key("C", "PageUp") || !key("B", "Tab", false)) throw Error("Tab reorder claimed an unrelated shortcut");
  button("A").dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  await until(() => order() === "BC" && button("B").getAttribute("aria-selected") === "true", "Middle click did not close the background tab");
  sessionStorage.removeItem("mangoo.notebook.tabs:drag-fixture");
}
async function run() {
  root.render(<Fixture format="markdown" />);
  await until(() => document.querySelector(".nb-el-h1")?.textContent === "Field notes", "Markdown heading was not rendered");
  const editor = EditorView.findFromDOM(query(".cm-content"))!;
  if (query(".nb-el-strong").textContent !== "bold" || !query("table[aria-label='Edit table']")) throw new Error("Inline Markdown structure");
  editor.focus();
  editor.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "f", code: "KeyF", ctrlKey: true, bubbles: true, cancelable: true }));
  await until(() => Boolean(document.querySelector("input[aria-label='Find']")), "Ctrl+F did not open the note search");
  const field = query<HTMLInputElement>("input[aria-label='Find']");
  field.value = "bold"; field.dispatchEvent(new Event("change", { bubbles: true }));
  query("button[name='next']").click();
  if (editor.state.sliceDoc(editor.state.selection.main.from, editor.state.selection.main.to) !== "bold") throw new Error("Find next did not select note text");
  query("button[name='prev']").click();
  query("button[name='close']").click();
  query<HTMLInputElement>("input[aria-label='Toggle task completion']").click();
  await until(() => saved.includes("- [x] Check"), "Task checkbox did not update Markdown");
  mode("Source");
  await until(() => query(".cm-content").textContent!.includes("**bold**"), "Source mode lost formatting markers");
  await until(() => Boolean(document.querySelector(".cm-lineNumbers")), "Source line numbers missing");
  mode("Write");
  await until(() => Boolean(document.querySelector(".nb-writing")), "Write mode not restored");
  query("button[aria-label='Document outline']").click();
  await until(() => Boolean(document.querySelector(".nb-outline nav button[title='Field notes']")), "Document outline missing heading");
  query(".nb-outline nav button[title='Field notes']").click();
  if (editor.state.selection.main.head !== 0) throw new Error("Outline did not navigate to the selected heading");
  query("button[aria-label='Close outline']").click();
  query("button[aria-label='Element styles']").click();
  await until(() => document.activeElement?.matches("#notebook-element-styles select") === true, "Keyboard-accessible style dialog missing");
  const oldColor = getComputedStyle(query(".cm-content")).color;
  const editsBefore = styleCommits;
  const openColor = async () => {
    query("button[aria-label='Text color']").click();
    await until(() => document.activeElement?.getAttribute("aria-label") === "HEX color", "Nested HEX editor did not receive focus");
    return query<HTMLInputElement>(".color-picker-popup:not([aria-hidden=true]) input[aria-label='HEX color']");
  };
  setHex(await openColor(), "#124578");
  await until(() => getComputedStyle(query(".cm-content")).color === "rgb(18, 69, 120)", "Note did not preview draft color");
  if (styleCommits !== editsBefore) throw new Error("Preview submitted note styles");
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await until(() => getComputedStyle(query(".cm-content")).color === oldColor, "Escape did not restore note color");
  setHex(await openColor(), "#234567");
  query(".color-picker-confirm").click();
  await until(() => styleCommits === editsBefore + 1 && getComputedStyle(query(".cm-content")).color === "rgb(35, 69, 103)", "Confirm did not save exactly one note style edit");
  const weight = Array.from(document.querySelectorAll("select")).find(el => el.parentElement?.textContent?.startsWith("Weight"))!;
  weight.value = "800"; weight.dispatchEvent(new Event("change", { bubbles: true }));
  await until(() => getComputedStyle(query(".cm-content")).fontWeight === "800", "Body style not applied");
  query("button.nb-reset").click();
  await until(() => getComputedStyle(query(".cm-content")).fontWeight !== "800", "Style reset failed");
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  editor.focus();
  editor.dispatch({ selection: { anchor: editor.state.doc.length } });
  const paste = new DataTransfer(); paste.items.add(image);
  editor.contentDOM.dispatchEvent(new ClipboardEvent("paste", { clipboardData: paste, bubbles: true, cancelable: true }));
  editor.dispatch({ changes: { from: 0, insert: "\n" } });
  await until(() => saved.endsWith("![drawing.svg](mangoo-attachment:sample-0)\n") && document.querySelector<HTMLImageElement>(".nb-el-image")?.naturalWidth === 6,
    "Pasted image lost its insertion point or offline preview").catch(error => { throw new Error(error.message + JSON.stringify({saved, doc: editor.state.doc.toString(), dom: editor.contentDOM.innerHTML.slice(-1800)})); });
  const drop = new DataTransfer(); drop.items.add(new File(["fixture"], "paper.pdf", { type: "application/pdf" }));
  editor.contentDOM.dispatchEvent(new DragEvent("drop", { dataTransfer: drop, bubbles: true, cancelable: true }));
  await until(() => saved.includes("![paper.pdf](mangoo-attachment:sample-1)") && document.querySelector("[data-document-view]")?.textContent === "paper.pdf", "Document drop did not render");
  const headingPos = editor.state.doc.toString().indexOf("# Field notes");
  editor.focus(); editor.dispatch({ selection: { anchor: headingPos, head: headingPos + 1 } });
  query("button[aria-label='Italic (Ctrl+I)']").click();
  await until(() => saved.includes("*#* Field notes"), "Formatting toolbar lost selected range");
  const rich = new DataTransfer();
  rich.setData("text/html", '<p>Sample <img alt="embedded" src="data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22%20width=%224%22%20height=%224%22/%3E"></p>');
  editor.dispatch({ selection: { anchor: editor.state.doc.length } });
  editor.contentDOM.dispatchEvent(new ClipboardEvent("paste", { clipboardData: rich, bubbles: true, cancelable: true }));
  await until(() => saved.includes("Sample ![embedded](mangoo-attachment:sample-2)"), "HTML clipboard data URI was not collected with surrounding text");
  const urlOnly = new DataTransfer();
  urlOnly.setData("text/html", '<img src="https://example.invalid/linked.png" alt="remote">');
  editor.contentDOM.dispatchEvent(new ClipboardEvent("paste", { clipboardData: urlOnly, bubbles: true, cancelable: true }));
  await until(() => Boolean(document.querySelector(".nb-editor-status")?.textContent?.includes("Copy the image itself")), "URL-only clipboard image missing guidance");
  mode("Source");
  editor.dispatch({ changes: { from: 0, insert: "Tab undo check\n" }, selection: { anchor: 5 } });
  await until(() => saved.startsWith("Tab undo check") && Boolean(document.querySelector(".nb-source")), "Tab state fixture missing");
  const tabSource = saved;
  let pageMessages = 0;
  window.addEventListener("message", e => {
    if (e.data === "notebook-test-ready" && e.origin === "null" && e.source === document.querySelector("iframe")?.contentWindow) pageMessages++;
  });
  root.render(<Fixture key="html" format="html" />);
  await until(() => Boolean(document.querySelector("iframe")), "HTML preview not mounted");
  const iframe = query<HTMLIFrameElement>("iframe");
  if (iframe.sandbox.length || !iframe.srcdoc.includes("Page")) throw new Error("Static HTML preview policy");
  await wait(150);
  if (pageMessages) throw new Error("Script executed without opt-in");
  query<HTMLInputElement>(".nb-interactive input").click();
  await until(() => pageMessages === 1, "Interactive HTML preview did not run in an opaque origin");
  if (query<HTMLIFrameElement>("iframe").sandbox.value !== "allow-scripts") throw new Error("Unexpected iframe permissions");
  mode("Page");
  await until(() => query(".nb-source").hidden, "Page-only mode kept editor visible");
  mode("Code");
  await until(() => !document.querySelector("iframe") && !query(".nb-source").hidden, "Code mode kept old page running");
  root.render(<Fixture key="restored-markdown" format="markdown" initial={tabSource} />);
  await until(() => document.querySelector(".cm-content")?.textContent?.includes("Tab undo check") === true && Boolean(document.querySelector(".cm-lineNumbers")), "Tab source mode not restored");
  const restored = EditorView.findFromDOM(query(".cm-content"))!;
  if (restored.state.selection.main.head !== 5) throw Error("Tab lost its caret");
  restored.focus();
  restored.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "z", code: "KeyZ", ctrlKey: true, bubbles: true, cancelable: true }));
  await until(() => !saved.startsWith("Tab undo check"), "Tab lost its undo history");
  await (await import("./notebookCssImport")).testCssImportAndArchive();
  await (await import("./notebookTreeInteraction")).testNotebookTreeMoves();
  await testTabActions();
  await testDefaultColorPreview();
}
void run().then(() => Reflect.set(window, "__overlayLifecycleResult", { ok: true }))
  .catch(error => Reflect.set(window, "__overlayLifecycleResult", { ok: false, error: String(error?.stack ?? error) }));

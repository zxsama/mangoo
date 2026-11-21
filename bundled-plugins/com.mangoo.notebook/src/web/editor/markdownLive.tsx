import { syntaxTree } from "@codemirror/language";
import { StateEffect, StateField, type EditorState, type Range } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";
import { createRoot, type Root } from "react-dom/client";
import { attachmentId, isImageFile } from "./noteFormat.ts";
import type { AttachmentReader } from "./HtmlPreview.tsx";
import { imageHelp } from "./clipboard.ts";

export interface LiveOptions extends AttachmentReader { zh: boolean }

// CodeMirror reuses equal widget DOM across distinct widget instances.
// Bind resource ownership to that DOM so scrolling/editing always releases the right object URL.
const mountedEmbeds = new WeakMap<HTMLElement, { root?: Root; url?: string }>();
class EmbedWidget extends WidgetType {
  constructor(readonly src: string, readonly label: string, readonly options: LiveOptions) { super(); }
  eq(other: EmbedWidget) { return other.src === this.src && other.label === this.label && other.options === this.options; }
  toDOM(view: EditorView) {
    const container = document.createElement("span");
    const resources: { root?: Root; url?: string } = {};
    mountedEmbeds.set(container, resources);
    container.className = "nb-embed";
    const image = (url: string) => {
      const img = document.createElement("img");
      img.alt = this.label;
      img.className = "nb-el-image";
      img.onload = () => view.requestMeasure();
      img.src = url;
      container.replaceChildren(img);
    };
    const documentPreview = (id: string) => {
      if (this.options.renderAttachment) {
        resources.root = createRoot(container);
        resources.root.render(this.options.renderAttachment(id, this.label));
      } else container.textContent = this.label;
    };
    const id = attachmentId(this.src);
    if (!id) {
      if (/^data:image\/(?:png|jpeg|webp|gif|avif|svg\+xml)[;,]/i.test(this.src)) image(this.src);
      else {
        container.textContent = (this.label ? this.label + " · " : "") + imageHelp(this.options.zh);
        container.classList.add("nb-image-unavailable");
      }
      return container;
    }
    // Large documents are opened by their dedicated reader without first loading a second copy here.
    if (/\.[a-z\d]+$/i.test(this.label) && !isImageFile({ name: this.label, type: "" })) {
      documentPreview(id);
      return container;
    }
    container.textContent = this.label || (this.options.zh ? "正在读取图片…" : "Loading image…");
    void this.options.resolveAttachment(id).then(blob => {
      if (!mountedEmbeds.has(container)) return;
      if (!blob.type.startsWith("image/") && !isImageFile({ name: this.label, type: blob.type })) { documentPreview(id); return; }
      resources.url = URL.createObjectURL(blob);
      image(resources.url);
    }).catch(() => {
      if (mountedEmbeds.has(container)) {
        container.textContent = (this.options.zh ? "无法读取附件 · " : "Attachment unavailable · ") + this.label;
        container.setAttribute("role", "status");
      }
    });
    return container;
  }
  destroy(dom: HTMLElement) {
    const resources = mountedEmbeds.get(dom);
    mountedEmbeds.delete(dom);
    if (resources?.url) URL.revokeObjectURL(resources.url);
    const root = resources?.root;
    if (root) queueMicrotask(() => root.unmount());
  }
}

class CheckWidget extends WidgetType {
  constructor(readonly checked: boolean, readonly label: string) { super(); }
  eq(other: CheckWidget) { return this.checked === other.checked && this.label === other.label; }
  toDOM(view: EditorView) {
    const check = document.createElement("input");
    check.type = "checkbox"; check.checked = this.checked;
    check.setAttribute("aria-label", this.label);
    check.addEventListener("change", () => {
      const from = view.posAtDOM(check);
      view.dispatch({ changes: { from, to: from + 3, insert: check.checked ? "[x]" : "[ ]" }, userEvent: "input" });
    });
    return check;
  }
}

class TableWidget extends WidgetType {
  constructor(readonly source: string, readonly rows: string[][], readonly label: string) { super(); }
  eq(other: TableWidget) { return this.source === other.source && this.label === other.label; }
  toDOM(view: EditorView) {
    const table = document.createElement("table");
    table.className = "nb-el-table";
    table.tabIndex = 0;
    table.setAttribute("aria-label", this.label);
    this.rows.forEach((cells, index) => {
      const row = table.insertRow();
      for (const cell of cells) {
        const td = document.createElement(index === 0 ? "th" : "td");
        td.textContent = cell;
        row.append(td);
      }
    });
    const edit = () => {
      const from = view.posAtDOM(table);
      view.focus();
      view.dispatch({ selection: { anchor: from + 1 } });
    };
    table.addEventListener("click", edit);
    table.addEventListener("keydown", event => { if (event.key === "Enter") { event.preventDefault(); edit(); } });
    return table;
  }
}

const focusEffect = StateEffect.define<boolean>();
const viewportEffect = StateEffect.define<{ from: number; to: number }>();
const liveViewport = StateField.define<{ from: number; to: number }>({
  create: state => ({ from: 0, to: state.doc.line(Math.min(100, state.doc.lines)).to }),
  update(range, tr) {
    for (const effect of tr.effects) if (effect.is(viewportEffect)) return effect.value;
    if (!tr.docChanged) return range;
    const from = tr.changes.mapPos(range.from), to = tr.changes.mapPos(range.to, 1);
    const previousLines = tr.startState.doc.lineAt(range.to).number - tr.startState.doc.lineAt(range.from).number;
    const first = tr.newDoc.lineAt(from).number;
    const last = Math.min(tr.newDoc.lines, first + previousLines + 50);
    return { from, to: Math.min(to, tr.newDoc.line(last).to) };
  }
});

// Block widgets need a direct decorations field. Keep its coverage ahead of
// scrolling, so moving the caret never rebuilds an entire long manuscript.
const observeViewport = ViewPlugin.fromClass(class {
  frame = 0;
  constructor(readonly view: EditorView) { this.schedule(); }
  update(update: ViewUpdate) { if (update.viewportChanged || update.docChanged) this.schedule(); }
  schedule() {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      const { state, viewport } = this.view;
      const { doc } = state;
      const stored = state.field(liveViewport);
      const first = doc.lineAt(viewport.from).number, last = doc.lineAt(viewport.to).number;
      if (stored.from <= doc.line(Math.max(1, first - 12)).from && stored.to >= doc.line(Math.min(doc.lines, last + 12)).to) return;
      this.view.dispatch({ effects: viewportEffect.of({
        from: doc.line(Math.max(1, first - 50)).from,
        to: doc.line(Math.min(doc.lines, last + 50)).to
      }) });
    });
  }
  destroy() { cancelAnimationFrame(this.frame); }
});
const activeField = StateField.define<boolean>({
  create: () => false,
  update(active, tr) {
    for (const effect of tr.effects) if (effect.is(focusEffect)) return effect.value;
    return active;
  }
});

function markdownDecorations(state: EditorState, options: LiveOptions): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const { doc } = state;
  const visible = state.field(liveViewport);
  const selection = state.selection.main;
  const active = (from: number, to: number) => state.field(activeField) && selection.from <= to && selection.to >= from;
  const mark = (from: number, to: number, className: string) => {
    if (to > from) ranges.push(Decoration.mark({ class: className }).range(from, to));
  };
  const hide = (from: number, to: number) => {
    if (to > from) ranges.push(Decoration.replace({}).range(from, to));
  };
  const lines = (from: number, to: number, className: string) => {
    for (let n = doc.lineAt(Math.max(from, visible.from)).number, end = doc.lineAt(Math.min(to, visible.to)).number; n <= end; n++) {
      ranges.push(Decoration.line({ class: className }).range(doc.line(n).from));
    }
  };
  const slice = (node: SyntaxNode) => doc.sliceString(node.from, node.to);
  const tree = syntaxTree(state);
  let definitions: Map<string, string> | undefined;
  const normalizeLabel = (label: string) => label.trim().replace(/\s+/g, " ").toLowerCase();
  const referenceUrl = (label: string) => {
    if (!definitions) {
      definitions = new Map();
      tree.iterate({ enter(ref) {
        if (ref.name !== "LinkReference") return;
        const id = ref.node.getChild("LinkLabel");
        const url = ref.node.getChild("URL");
        if (id && url) {
          const key = normalizeLabel(slice(id).slice(1, -1));
          if (!definitions!.has(key)) definitions!.set(key, slice(url));
        }
        return false;
      } });
    }
    return definitions.get(normalizeLabel(label));
  };
  tree.iterate({ from: visible.from, to: visible.to, enter(ref) {
    const { from, to, name } = ref;
    const heading = /^(?:ATX|Setext)Heading([1-6])$/.exec(name);
    if (heading) lines(from, to, `nb-el-h${heading[1]}`);
    switch (name) {
      case "StrongEmphasis": mark(from, to, "nb-el-strong"); break;
      case "Emphasis": mark(from, to, "nb-el-em"); break;
      case "Strikethrough": mark(from, to, "nb-el-strike"); break;
      case "InlineCode": mark(from, to, "nb-el-code"); break;
      case "Blockquote": lines(from, to, "nb-el-quote"); break;
      case "ListItem": lines(from, to, "nb-el-list"); break;
      case "FencedCode": case "CodeBlock": lines(from, to, "nb-el-code nb-code-line"); break;
      case "HorizontalRule": lines(from, to, "nb-rule"); break;
      case "Table": {
        if (active(from, to)) break;
        const rows = [ref.node.getChild("TableHeader"), ...ref.node.getChildren("TableRow")]
          .filter((row): row is SyntaxNode => Boolean(row))
          .map(row => row.getChildren("TableCell").map(cell => slice(cell).trim()));
        ranges.push(Decoration.replace({ block: true, widget: new TableWidget(doc.sliceString(from, to), rows, options.zh ? "编辑表格" : "Edit table") }).range(from, to));
        return false;
      }
      case "Image": {
        const url = ref.node.getChild("URL");
        const brackets = ref.node.getChildren("LinkMark");
        if (brackets.length < 2 || (active(from, to) && selection.head > from && selection.head < to)) break;
        const label = doc.sliceString(brackets[0].to, brackets[1].from).replace(/\\([\\\[\]])/g, "$1");
        const reference = ref.node.getChild("LinkLabel");
        const src = url ? slice(url) : referenceUrl((reference && slice(reference).slice(1, -1)) || label);
        if (!src) break;
        const ownLine = doc.lineAt(from).from === from && doc.lineAt(to).to === to;
        ranges.push(Decoration.replace({ block: ownLine, widget: new EmbedWidget(src.replace(/^<|>$/g, ""), label, options) }).range(from, to));
        return false;
      }
      case "TaskMarker": {
        if (!active(from, to)) ranges.push(Decoration.replace({ widget: new CheckWidget(
          doc.sliceString(from, to).toLowerCase() === "[x]", options.zh ? "切换任务状态" : "Toggle task completion") }).range(from, to));
        break;
      }
      case "ListMark": {
        const task = ref.node.parent?.getChild("Task")?.getChild("TaskMarker");
        if (task && !active(from, task.to)) hide(from, task.from);
        break;
      }
      case "Link": {
        mark(from, to, "nb-el-link");
        if (!active(from, to)) {
          const brackets = ref.node.getChildren("LinkMark");
          if (brackets.length > 1) { hide(from, brackets[0].to); hide(brackets[1].from, to); }
        }
        break;
      }
      case "HeaderMark": case "QuoteMark": case "EmphasisMark": case "StrikethroughMark": case "CodeMark": {
        const parent = ref.node.parent;
        const line = doc.lineAt(from);
        const focused = parent && name !== "QuoteMark" ? active(parent.from, parent.to) : active(line.from, line.to);
        if (focused) { mark(from, to, "nb-syntax-mark"); break; }
        if (parent?.name === "FencedCode") { mark(from, to, "nb-syntax-mark"); break; }
        hide(from, (name === "HeaderMark" || name === "QuoteMark") && doc.sliceString(to, to + 1) === " " ? to + 1 : to);
        break;
      }
    }
  } });
  return Decoration.set(ranges, true);
}

export function liveMarkdown(options: LiveOptions) {
  const field = StateField.define<DecorationSet>({
    create: state => markdownDecorations(state, options),
    update(previous, tr) {
      if (tr.docChanged || tr.selection || tr.effects.some(e => e.is(viewportEffect)) || tr.startState.field(activeField, false) !== tr.state.field(activeField)
        || syntaxTree(tr.startState) !== syntaxTree(tr.state)) return markdownDecorations(tr.state, options);
      return previous;
    },
    provide: f => EditorView.decorations.from(f)
  });
  return [activeField, liveViewport, observeViewport, EditorView.focusChangeEffect.of((_state, focused) => focusEffect.of(focused)), field];
}

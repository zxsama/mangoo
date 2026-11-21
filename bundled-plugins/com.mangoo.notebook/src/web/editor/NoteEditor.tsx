import { useCallback, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Annotation, Compartment, EditorState, StateEffect, StateField, Transaction } from "@codemirror/state";
import { EditorView, getPanel, keymap, lineNumbers, placeholder, showPanel } from "@codemirror/view";
import { defaultKeymap, history, historyField, historyKeymap } from "@codemirror/commands";
import { openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { syntaxHighlighting } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { classHighlighter } from "@lezer/highlight";
import { icons, type AppIcon } from "@mangoo/web/plugin-host";
import { HtmlPreview, type AttachmentReader } from "./HtmlPreview.tsx";
import { NoteStylePanel } from "./NoteStylePanel.tsx";
import { NoteOutline } from "./NoteOutline";
import { liveMarkdown } from "./markdownLive.tsx";
import { attachmentMarkup, elementStylesheet, mergeNoteEditorStyles, type NoteEditorStyles } from "./noteFormat.ts";
import { useNotebookPreferences } from "../notebookPreferences";
import { imageClipboardContent, imageHelp } from "./clipboard.ts";
import "./editor.css";

export type { NoteEditorStyles } from "./noteFormat.ts";
export interface NoteTabView {
  anchor?: number;
  head?: number;
  scrollTop?: number;
  sourceMode?: boolean;
  htmlMode?: "code" | "split" | "preview";
  interactive?: boolean;
  outline?: boolean;
  annotating?: boolean;
  page?: number;
  revision?: number;
  undo?: { length: number; history: typeof historyField extends StateField<infer T> ? T : never };
}
export interface NoteEditorProps extends AttachmentReader {
  tabView?: NoteTabView;
  format: "markdown" | "html";
  value: string;
  onChange(value: string): void;
  addAttachment(file: File): Promise<string>;
  onPendingInsert?(operation: Promise<void>): void;
  styles: Record<string, unknown>;
  onStylesChange(styles: NoteEditorStyles): void;
  language?: string;
}

// Track pending insertions through intervening edits, undo, and concurrent drops.
const anchorEffect = StateEffect.define<{ id: number; pos?: number }>();
const externalValue = Annotation.define<boolean>();
const insertionPoints = StateField.define<ReadonlyMap<number, number>>({
  create: () => new Map(),
  update(points, tr) {
    if (!points.size && !tr.effects.some(e => e.is(anchorEffect))) return points;
    const mapped = new Map(Array.from(points, ([id, pos]) => [id, tr.changes.mapPos(pos, 1)]));
    for (const effect of tr.effects) if (effect.is(anchorEffect)) {
      const { id, pos } = effect.value;
      if (pos === undefined) mapped.delete(id); else mapped.set(id, pos);
    }
    return mapped;
  }
});

function wrapSelection(view: EditorView, left: string, right: string) {
  const { from, to } = view.state.selection.main;
  const doc = view.state.doc;
  const unwrap = from >= left.length && doc.sliceString(from - left.length, from) === left && doc.sliceString(to, to + right.length) === right;
  const changeFrom = unwrap ? from - left.length : from;
  const changeTo = unwrap ? to + right.length : to;
  const text = doc.sliceString(from, to);
  const start = unwrap ? changeFrom : from + left.length;
  view.dispatch({ changes: { from: changeFrom, to: changeTo, insert: unwrap ? text : left + text + right },
    selection: { anchor: start, head: start + text.length }, userEvent: "input", scrollIntoView: true });
  view.focus();
  return true;
}

function prefixLine(view: EditorView, prefix: string) {
  const { from, to } = view.state.selection.main;
  const start = view.state.doc.lineAt(from).number;
  const end = view.state.doc.lineAt(to).number;
  const changes = [];
  for (let i = start; i <= end; i++) {
    const line = view.state.doc.line(i);
    changes.push(line.text.startsWith(prefix) ? { from: line.from, to: line.from + prefix.length } : { from: line.from, insert: prefix });
  }
  view.dispatch({ changes, userEvent: "input", scrollIntoView: true });
  view.focus();
}

function Tool({ icon: Icon, label, onClick, pressed }: { icon: AppIcon; label: string; onClick(): void; pressed?: boolean }) {
  return <button className="nb-tool" type="button" aria-label={label} title={label} aria-pressed={pressed}
    onMouseDown={e => e.preventDefault()} onClick={onClick}><Icon size={16} aria-hidden="true" /></button>;
}

export function NoteEditor(props: NoteEditorProps) {
  const { value, format, language, styles, onStylesChange } = props;
  const zh = !language?.toLowerCase().startsWith("en");
  const latest = useRef(props);
  latest.current = props;
  const host = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const sequence = useRef(0);
  const compartment = useMemo(() => new Compartment(), []);
  const preferences = useNotebookPreferences();
  const [tabSourceMode, setSourceMode] = useState(props.tabView?.sourceMode);
  const [tabHtmlMode, setHtmlMode] = useState(props.tabView?.htmlMode);
  const [tabOutline, setOutline] = useState(props.tabView?.outline);
  const sourceMode = tabSourceMode ?? (preferences.markdownMode === "source");
  const htmlMode = tabHtmlMode ?? preferences.htmlMode;
  const outline = tabOutline ?? preferences.outlineOpen;
  const [interactive, setInteractive] = useState(props.tabView?.interactive ?? false);
  useLayoutEffect(() => {
    if (props.tabView) Object.assign(props.tabView, { sourceMode: tabSourceMode, htmlMode: tabHtmlMode, interactive, outline: tabOutline });
  }, [props.tabView, tabSourceMode, tabHtmlMode, interactive, tabOutline]);
  const [styleAnchor, setStyleAnchor] = useState<HTMLElement | null>(null);
  const [previewStyles, setPreviewStyles] = useState<NoteEditorStyles | null>(null);
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(0);
  const [cursor, setCursor] = useState({ offset: 0, line: 1, column: 1, lines: 1 });
  const lastEditorValue = useRef(value);
  const outlineButton = useRef<HTMLButtonElement>(null);
  const resolveAttachment = useCallback((id: string) => latest.current.resolveAttachment(id), []);
  const renderAttachment = useCallback((id: string, name: string): ReactNode =>
    latest.current.renderAttachment?.(id, name) ?? name, []);
  const live = useMemo(() => liveMarkdown({ zh, resolveAttachment, renderAttachment }), [zh, resolveAttachment, renderAttachment]);
  const isLive = format === "markdown" && !sourceMode;
  const showEditor = format === "markdown" || htmlMode !== "preview";
  const showHtml = format === "html" && htmlMode !== "code";
  const inheritedStyles = format === "markdown" ? preferences.elementStyles : undefined;
  const displayedStyles = previewStyles ?? styles;
  const currentStyles = useMemo(() => inheritedStyles ? mergeNoteEditorStyles(inheritedStyles, displayedStyles) : displayedStyles, [inheritedStyles, displayedStyles]);

  const uploadAndInsert = useCallback(async (files: File[], pos?: number, clipboard?: NonNullable<ReturnType<typeof imageClipboardContent>>) => {
    const view = viewRef.current;
    if (!view || (!files.length && !clipboard)) return;
    const id = ++sequence.current;
    const selected = view.state.selection.main;
    const selectedText = clipboard && pos === undefined ? view.state.sliceDoc(selected.from, selected.to) : "";
    view.dispatch({ effects: [anchorEffect.of({ id, pos: pos ?? selected.to }), anchorEffect.of({ id: -id, pos: selected.from })] });
    setBusy(n => n + 1);
    setStatus("");
    const failed: string[] = [];
    const markup: string[] = [];
    const ids: Array<string | undefined> = [];
    for (const file of files) {
      if (viewRef.current !== view) return;
      try {
        const attachment = await latest.current.addAttachment(file);
        ids.push(attachment);
        markup.push(attachmentMarkup(format, attachment, file));
      } catch (reason) { ids.push(undefined); failed.push(`${file.name} · ${reason instanceof Error ? reason.message : String(reason)}`); }
    }
    if (viewRef.current !== view) return;
    const at = view.state.field(insertionPoints).get(id);
    if (at === undefined) return;
    // Standalone lines give embedded documents room to render. The user's current caret is mapped normally.
    const before = view.state.doc.sliceString(Math.max(0, at - 2), at);
    const after = view.state.doc.sliceString(at, at + 1);
    const prefix = !before || before === "\n" || before.endsWith("\n\n") ? "" : before.endsWith("\n") ? "\n" : "\n\n";
    const suffix = after && after !== "\n" ? "\n\n" : "\n";
    const insert = clipboard ? clipboard.render(ids) : markup.length ? prefix + markup.join("\n\n") + suffix : "";
    const start = view.state.field(insertionPoints).get(-id) ?? at;
    const from = clipboard && selectedText && view.state.sliceDoc(start, at) === selectedText ? start : at;
    view.dispatch({ changes: { from, to: at, insert }, effects: [anchorEffect.of({ id }), anchorEffect.of({ id: -id })], userEvent: "input" });
    setBusy(n => n - 1);
    if (clipboard?.linkedOnly) failed.push(imageHelp(zh));
    if (failed.length) setStatus(failed.join(" · "));
  }, [format, zh]);
  const insertFiles = useCallback((...args: Parameters<typeof uploadAndInsert>) => {
    const operation = uploadAndInsert(...args);
    latest.current.onPendingInsert?.(operation);
    return operation;
  }, [uploadAndInsert]);

  useLayoutEffect(() => {
    if (!host.current) return;
    const saved = latest.current.tabView;
    const source = latest.current.value;
    const undo = saved?.undo;
    const selection = saved?.anchor === undefined ? undefined : {
      anchor: Math.min(saved.anchor, source.length), head: Math.min(saved.head ?? saved.anchor, source.length),
    };
    const view = new EditorView({
      parent: host.current,
      state: EditorState.create({ doc: source, selection, extensions: [
        format === "markdown" ? markdown({ base: markdownLanguage }) : html(),
        compartment.of(format === "markdown" ? live : syntaxHighlighting(classHighlighter)),
        history(), undo?.length === source.length ? historyField.init(() => undo.history) : [],
        insertionPoints, search({ top: true }),
        zh ? EditorState.phrases.of({
          Find: "查找", Replace: "替换为", next: "下一个", previous: "上一个", all: "全选",
          "match case": "区分大小写", regexp: "正则", "by word": "整词", replace: "替换", "replace all": "全部替换",
          close: "关闭", "current match": "当前匹配", "on line": "位于行", "replaced match on line $": "已替换第 $ 行的匹配",
          "replaced $ matches": "已替换 $ 个匹配", "Go to line": "跳转至行", go: "跳转"
        }) : [],
        EditorView.contentAttributes.of({ "aria-label": zh ? "笔记正文" : "Note body", "aria-multiline": "true",
          spellcheck: format === "markdown" ? "true" : "false" }),
        placeholder(zh ? (format === "html" ? "输入 HTML…" : "开始书写…") : (format === "html" ? "Write HTML…" : "Start writing…")),
        keymap.of([...(format === "markdown" ? [
          { key: "Mod-b", run: (editor: EditorView) => wrapSelection(editor, "**", "**") },
          { key: "Mod-i", run: (editor: EditorView) => wrapSelection(editor, "*", "*") }
        ] : []), ...searchKeymap, ...historyKeymap, ...defaultKeymap]),
        EditorView.updateListener.of(update => {
          // CodeMirror creates its search and go-to-line controls outside React.
          const panels = update.state.facet(showPanel);
          if (panels !== update.startState.facet(showPanel)) {
            for (const create of panels) {
              const panel = create && getPanel(update.view, create);
              for (const button of panel?.dom.querySelectorAll("button") ?? []) {
                const label = button.firstChild;
                if (label?.nodeType !== Node.TEXT_NODE || label.nextSibling) continue;
                const content = button.ownerDocument.createElement("span");
                content.dataset.buttonContent = "";
                button.replaceChild(content, label);
                content.append(label);
              }
            }
          }
          if (update.docChanged && update.transactions.some(t => !t.annotation(externalValue))) {
            lastEditorValue.current = update.state.doc.toString();
            latest.current.onChange(lastEditorValue.current);
          }
          if (update.selectionSet || update.docChanged) {
            const offset = update.state.selection.main.head;
            const line = update.state.doc.lineAt(offset);
            setCursor({ offset, line: line.number, column: offset - line.from + 1, lines: update.state.doc.lines });
          }
        }),
        EditorView.domEventHandlers({
          paste(event) {
            const clipboard = event.clipboardData && imageClipboardContent(event.clipboardData, format);
            if (clipboard) {
              event.preventDefault();
              void insertFiles(clipboard.files, undefined, clipboard);
              return true;
            }
            const files = Array.from(event.clipboardData?.files ?? []);
            if (!files.length) return false;
            event.preventDefault();
            void insertFiles(files);
            return true;
          },
          dragover(event) {
            if (!event.dataTransfer?.types.includes("Files")) return false;
            event.preventDefault();
            event.dataTransfer.dropEffect = "copy";
            return true;
          },
          drop(event, editor) {
            const position = editor.posAtCoords({ x: event.clientX, y: event.clientY }) ?? editor.state.selection.main.to;
            const clipboard = event.dataTransfer && imageClipboardContent(event.dataTransfer, format);
            if (clipboard) {
              event.preventDefault(); event.stopPropagation();
              void insertFiles(clipboard.files, position, clipboard);
              return true;
            }
            const files = Array.from(event.dataTransfer?.files ?? []);
            if (!files.length) return false;
            event.preventDefault(); event.stopPropagation();
            void insertFiles(files, position);
            return true;
          }
        })
      ] })
    });
    viewRef.current = view;
    const offset = view.state.selection.main.head;
    const line = view.state.doc.lineAt(offset);
    setCursor({ offset, line: line.number, column: offset - line.from + 1, lines: view.state.doc.lines });
    const restore = requestAnimationFrame(() => {
      if (saved?.scrollTop) view.scrollDOM.scrollTop = saved.scrollTop;
    });
    return () => {
      cancelAnimationFrame(restore);
      if (saved) {
        const { anchor, head } = view.state.selection.main;
        Object.assign(saved, { anchor, head, scrollTop: view.scrollDOM.scrollTop });
        // Store undo deltas, not a copy of the full note. The workspace checks the
        // persisted revision before installing this history in a remounted editor.
        saved.undo = { length: view.state.doc.length, history: view.state.field(historyField) };
      }
      viewRef.current = null; view.destroy();
    };
  }, [compartment, format, insertFiles, live, zh]);

  useLayoutEffect(() => {
    const extensions = isLive ? [live, EditorView.lineWrapping] :
      [syntaxHighlighting(classHighlighter), lineNumbers(), preferences.wrapSource ? EditorView.lineWrapping : []];
    viewRef.current?.dispatch({ effects: compartment.reconfigure(extensions) });
    viewRef.current?.requestMeasure();
  }, [compartment, isLive, live, showEditor, htmlMode, preferences.wrapSource]);

  useLayoutEffect(() => {
    const editor = viewRef.current;
    if (editor && lastEditorValue.current !== value) {
      lastEditorValue.current = value;
      if (editor.state.doc.toString() !== value) editor.dispatch({
        changes: { from: 0, to: editor.state.doc.length, insert: value },
        annotations: [externalValue.of(true), Transaction.addToHistory.of(false)] });
    }
  }, [value]);

  const apply = (action: (editor: EditorView) => void) => { if (viewRef.current) action(viewRef.current); };
  const styleSheet = useMemo(() => elementStylesheet(currentStyles, false), [currentStyles]);
  return <section className="nb-doc-editor" aria-label={zh ? "笔记编辑器" : "Note editor"}>
    <div className="nb-editor-tools" role="group" aria-label={zh ? "编辑工具" : "Editor tools"}>
      <div className="nb-mode-switch" role="group" aria-label={zh ? "编辑模式" : "Editor mode"}>
        {format === "markdown" ? <>
          <Tool icon={icons.Pencil} label={zh ? "写作" : "Write"} pressed={!sourceMode} onClick={() => setSourceMode(false)} />
          <Tool icon={icons.Code} label={zh ? "源码" : "Source"} pressed={sourceMode} onClick={() => setSourceMode(true)} />
        </> : (["code", "split", "preview"] as const).map((mode, index) =>
          <Tool key={mode} icon={[icons.Code, icons.GalleryVerticalEnd, icons.Eye][index]}
            label={(zh ? ["代码", "对照", "网页"] : ["Code", "Split", "Page"])[index]}
            pressed={htmlMode === mode} onClick={() => setHtmlMode(mode)} />)}
      </div>
      {format === "markdown" && <div className="nb-format-tools">
        <Tool icon={icons.Heading2} label={zh ? "二级标题" : "Heading 2"} onClick={() => apply(v => prefixLine(v, "## "))} />
        <Tool icon={icons.Bold} label={zh ? "粗体 (Ctrl+B)" : "Bold (Ctrl+B)"} onClick={() => apply(v => wrapSelection(v, "**", "**"))} />
        <Tool icon={icons.Italic} label={zh ? "斜体 (Ctrl+I)" : "Italic (Ctrl+I)"} onClick={() => apply(v => wrapSelection(v, "*", "*"))} />
        <Tool icon={icons.Quote} label={zh ? "引用" : "Block quote"} onClick={() => apply(v => prefixLine(v, "> "))} />
        <Tool icon={icons.ListBullets} label={zh ? "项目列表" : "Bullet list"} onClick={() => apply(v => prefixLine(v, "- "))} />
        <Tool icon={icons.ListTodo} label={zh ? "任务列表" : "Task list"} onClick={() => apply(v => prefixLine(v, "- [ ] "))} />
      </div>}
      <div className="nb-editor-end">
        {format === "html" && showHtml && <label className="nb-interactive"><input type="checkbox" checked={interactive}
          onChange={e => setInteractive(e.target.checked)} />{zh ? "运行脚本" : "Run scripts"}</label>}
        <Tool icon={icons.Search} label={zh ? "查找正文 (Ctrl+F)" : "Find in note (Ctrl+F)"} onClick={() => {
          if (!showEditor) setHtmlMode("split");
          requestAnimationFrame(() => apply(openSearchPanel));
        }} />
        <Tool icon={icons.Paperclip} label={zh ? "插入图片或文件" : "Insert image or file"} onClick={() => input.current?.click()} />
        <input ref={input} type="file" multiple hidden onChange={e => {
          const files = Array.from(e.currentTarget.files ?? []); e.currentTarget.value = ""; void insertFiles(files);
        }} />
        <button className="nb-tool" type="button" title={zh ? "元素样式" : "Element styles"} aria-label={zh ? "元素样式" : "Element styles"}
          aria-haspopup="dialog" aria-expanded={Boolean(styleAnchor)} onClick={e => setStyleAnchor(e.currentTarget)}><icons.Palette size={16} /></button>
        {format === "markdown" && <button ref={outlineButton} className="nb-tool" type="button"
          aria-label={zh ? "文档大纲" : "Document outline"} title={zh ? "文档大纲" : "Document outline"}
          aria-expanded={outline} onClick={() => setOutline(!outline)}><icons.LayoutList size={16} /></button>}
      </div>
    </div>
    {(busy > 0 || status) && <div className="nb-editor-status" role="status">{status || (zh ? "正在保存附件…" : "Saving attachments…")}</div>}
    <div className="nb-editor-panes" data-split={showEditor && showHtml}>
      <div ref={host} hidden={!showEditor} className={isLive ? "nb-writing" : "nb-source"} />
      {showHtml && <HtmlPreview source={value} interactive={interactive} styles={currentStyles} zh={zh}
        resolveAttachment={resolveAttachment} renderAttachment={renderAttachment} />}
      {format === "markdown" && outline && <NoteOutline source={value} zh={zh} cursor={cursor.offset}
        onJump={position => apply(editor => {
          editor.dispatch({ selection: { anchor: position }, effects: EditorView.scrollIntoView(position, { y: "start", yMargin: 14 }) });
          editor.focus();
        })}
        onClose={() => { setOutline(false); outlineButton.current?.focus(); }} />}
    </div>
    <footer className="nb-editor-foot">
      <span>{format === "html" ? "HTML" : "Markdown"}</span>
      <span>{cursor.lines.toLocaleString()} {zh ? "行" : "lines"}</span>
      {showEditor && <span>{zh ? `第 ${cursor.line} 行，第 ${cursor.column} 列` : `Line ${cursor.line}, Col ${cursor.column}`}</span>}
    </footer>
    <style>{styleSheet}</style>
    <NoteStylePanel anchor={styleAnchor} zh={zh} styles={styles} inheritedStyles={inheritedStyles}
      onChange={onStylesChange} onPreview={setPreviewStyles} onClose={() => setStyleAnchor(null)} />
  </section>;
}

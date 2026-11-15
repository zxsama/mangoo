import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { icons } from "@mangoo/web/plugin-host";

import { asSingleUrl, blockRule, inlineRule, looksLikeMarkdown, markdownToHtml, trailingUrl, type InlineMark } from "../shared/markdown.ts";
import { openLink } from "./bridge.ts";
import { attachmentUrl, imageFiles } from "./images.ts";
import { applyBlock, applyInline, insertAttachmentImage, insertLink, sanitizeNotes, textToHtml, type BlockFormat, type InlineFormat } from "./richText.ts";

const INLINE_TAG: Record<InlineMark, string> = { bold: "b", italic: "i", strike: "s", code: "code", highlight: "mark" };
const escapeHtml = (value: string) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
/** Zero-width space: puts the caret after a just-made mark (stripped on save). */
const CARET_GAP = String.fromCharCode(0x200b);
/** Blocks where Markdown block prefixes apply (not inside a list item / heading / code). */
const PLAIN_BLOCKS = new Set(["P", "DIV"]);

/** The caret's text node and the text of its block up to the caret. */
function caretContext(root: HTMLElement): { node: Text; offset: number; block: HTMLElement; before: string } | null {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0 || !selection.isCollapsed) return null;
  const node = selection.anchorNode;
  if (!node || node.nodeType !== Node.TEXT_NODE || !root.contains(node)) return null;
  let block: HTMLElement | null = node.parentElement;
  while (block && block !== root && !/^(P|DIV|LI|H1|H2|H3|BLOCKQUOTE|PRE)$/.test(block.tagName)) block = block.parentElement;
  if (!block) return null;
  const range = document.createRange();
  range.setStart(block, 0);
  range.setEnd(node, selection.anchorOffset);
  return { node: node as Text, offset: selection.anchorOffset, block, before: range.toString() };
}

/** Select `length` characters before the caret inside its text node. */
function selectBefore(node: Text, offset: number, length: number): boolean {
  if (length <= 0 || offset < length) return false;
  const range = document.createRange();
  range.setStart(node, offset - length);
  range.setEnd(node, offset);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
  return true;
}
import type { TodoStore } from "./store.ts";

/**
 * Fill `<img data-attachment>` sources: synchronously from the store's cache
 * when the image was seen before (no blank frame, no broken-image icon),
 * otherwise once it loads. Images without pixels stay invisible (CSS) instead
 * of showing the browser's placeholder.
 */
export function fillAttachmentImages(root: HTMLElement, store: TodoStore): () => void {
  let cancelled = false;
  for (const image of Array.from(root.querySelectorAll<HTMLImageElement>("img[data-attachment]"))) {
    if (image.getAttribute("src")) continue;
    const id = image.getAttribute("data-attachment") ?? "";
    const cached = store.cachedAttachmentUrl(id);
    if (cached) {
      image.src = cached;
      continue;
    }
    void store.attachment(id).then((attachment) => {
      if (cancelled || !attachment) return;
      image.src = attachmentUrl(attachment);
    });
  }
  return () => {
    cancelled = true;
  };
}

function useAttachmentImages(root: HTMLElement | null, store: TodoStore, html: string): void {
  useLayoutEffect(() => {
    if (!root) return;
    return fillAttachmentImages(root, store);
  }, [root, store, html]);
}

/** Links inside rendered notes open externally (never navigate the window). */
function handleLinkClick(event: React.MouseEvent<HTMLElement>): boolean {
  const anchor = (event.target as HTMLElement).closest("a[href]");
  if (!anchor) return false;
  event.preventDefault();
  void openLink(anchor.getAttribute("href") ?? "");
  return true;
}

/** Read-only rendering of stored notes (detail preview, pinned notes). */
export function NotesView({ html, store, className = "" }: { html: string; store: TodoStore; className?: string }) {
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const clean = sanitizeNotes(html);
  useAttachmentImages(root, store, clean);
  if (!clean) return null;
  return (
    <div
      className={`todo-notes-view todo-rich ${className}`}
      dangerouslySetInnerHTML={{ __html: clean }}
      onClick={(event) => handleLinkClick(event)}
      ref={setRoot}
    />
  );
}

interface ToolButton {
  id: string;
  label: string;
  icon: ReactNode;
  shortcut?: string;
  run(): void;
}

interface RichTextEditorProps {
  /** Identity of the edited document; a change resets the editor. */
  documentKey: string;
  value: string;
  store: TodoStore;
  /** Focus the editor when it mounts. */
  autoFocus?: boolean;
  placeholder?: string;
  onChange(html: string): void;
  /** Upload an image; resolves to the attachment id and its displayable URL. */
  onImage(file: File): Promise<{ id: string; url: string } | null>;
}

/**
 * contenteditable editor for item notes: headings, emphasis, lists and
 * checklists, quotes, code, highlight, links and inline images, written with
 * the toolbar or with Markdown (typed or pasted); a pasted URL is a link. The DOM is
 * owned by the browser while editing (React never re-renders its content), so
 * the caret and the native undo stack survive; a synchronized change from
 * another device replaces the content only while the editor is not focused.
 */
export function RichTextEditor({ documentKey, value, store, autoFocus = false, placeholder = "描述…", onChange, onImage }: RichTextEditorProps) {
  const editorRef = useRef<HTMLDivElement | null>(null);
  const [editor, setEditor] = useState<HTMLDivElement | null>(null);
  const lastEmitted = useRef<string>(value);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [empty, setEmpty] = useState(!value);
  const [busy, setBusy] = useState(false);

  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  /** The writer bound to the document currently loaded in the editor. */
  const emitRef = useRef(onChange);

  const flush = useCallback(() => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    const element = editorRef.current;
    if (!element) return;
    const html = sanitizeNotes(element.innerHTML);
    if (html !== lastEmitted.current) {
      lastEmitted.current = html;
      emitRef.current(html);
    }
  }, []);

  // New document: replace content. The cleanup is a LAYOUT cleanup so it
  // runs before the next document is loaded: pending edits are written with
  // the writer of the document they belong to, never into the next one.
  useLayoutEffect(() => {
    const element = editorRef.current;
    if (!element) return;
    emitRef.current = onChangeRef.current;
    element.innerHTML = sanitizeNotes(value);
    fillAttachmentImages(element, store);
    lastEmitted.current = sanitizeNotes(value);
    setEmpty(!lastEmitted.current);
    return () => flush();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentKey]);

  // External (synchronized) change while not editing.
  useEffect(() => {
    const element = editorRef.current;
    if (!element || document.activeElement === element) return;
    const clean = sanitizeNotes(value);
    if (clean !== lastEmitted.current) {
      element.innerHTML = clean;
      fillAttachmentImages(element, store);
      lastEmitted.current = clean;
      setEmpty(!clean);
    }
  }, [value]);

  useAttachmentImages(editor, store, documentKey + value);
  const [focused, setFocused] = useState(false);
  useEffect(() => {
    if (!autoFocus) return;
    const element = editorRef.current;
    if (!element) return;
    element.focus();
    const range = document.createRange();
    range.selectNodeContents(element);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const schedule = () => {
    setEmpty(!(editorRef.current?.textContent ?? "").trim() && !editorRef.current?.querySelector("img"));
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, 600);
  };

  const run = (action: () => void) => {
    editorRef.current?.focus();
    action();
    schedule();
  };

  const tools: ToolButton[] = [
    { id: "h1", label: "标题（# ）", icon: <icons.Heading1 size={14} />, run: () => run(() => applyBlock("h1" as BlockFormat)) },
    { id: "bold", label: "加粗", shortcut: "Ctrl+B", icon: <icons.Bold size={14} />, run: () => run(() => applyInline("bold")) },
    { id: "italic", label: "斜体", shortcut: "Ctrl+I", icon: <icons.Italic size={14} />, run: () => run(() => applyInline("italic")) },
    { id: "strike", label: "删除线", shortcut: "Ctrl+Shift+X", icon: <icons.Strikethrough size={14} />, run: () => run(() => applyInline("strikeThrough")) },
    { id: "highlight", label: "高亮", shortcut: "Ctrl+Shift+H", icon: <icons.Highlighter size={14} />, run: () => run(() => applyInline("highlight" as InlineFormat)) },
    { id: "ul", label: "项目符号列表", icon: <icons.ListBullets size={14} />, run: () => run(() => applyBlock("ul")) },
    { id: "ol", label: "编号列表", icon: <icons.ListNumbers size={14} />, run: () => run(() => applyBlock("ol")) },
    { id: "check", label: "清单", icon: <icons.CheckSquare size={14} />, run: () => run(() => applyBlock("checklist")) },
    { id: "quote", label: "引用", icon: <icons.Quote size={14} />, run: () => run(() => applyBlock("blockquote")) },
    { id: "clear", label: "清除格式", icon: <icons.Eraser size={14} />, run: () => run(() => document.execCommand("removeFormat")) }
  ];

  const insertImages = async (files: File[]) => {
    if (files.length === 0) return;
    setBusy(true);
    try {
      for (const file of files.slice(0, 6)) {
        // Uploaded first, then inserted WITH its pixels: never an empty
        // <img> (the broken-image icon) waiting for a round trip.
        const image = await onImage(file);
        if (!image) continue;
        editorRef.current?.focus();
        insertAttachmentImage(image.id, file.name || "图片", image.url);
      }
      schedule();
    } finally {
      setBusy(false);
    }
  };

  const onPaste = (event: React.ClipboardEvent<HTMLDivElement>) => {
    const files = imageFiles(event.clipboardData);
    if (files.length) {
      event.preventDefault();
      void insertImages(files);
      return;
    }
    event.preventDefault();
    const html = event.clipboardData.getData("text/html");
    const text = event.clipboardData.getData("text/plain");
    // A pasted address becomes a link; over a selection it links that text.
    const url = asSingleUrl(text);
    if (url) {
      insertLink(url, text.trim());
      schedule();
      return;
    }
    // Markdown from a plain-text source converts; rich HTML keeps its formatting.
    const richHtml = Boolean(html) && /<(b|strong|em|i|h[1-3]|ul|ol|li|blockquote|a)\b/i.test(html);
    const clean = looksLikeMarkdown(text) && !richHtml
      ? sanitizeNotes(markdownToHtml(text))
      : html ? sanitizeNotes(html) : textToHtml(text);
    if (clean) document.execCommand("insertHTML", false, clean);
    schedule();
  };

  /**
   * Markdown as you type. Block prefixes convert on the space after them
   * ("# ", "- ", "1. ", "[ ] ", "> ", "``` "); a URL followed by a space
   * becomes a link. Everything goes through editing commands, so Ctrl+Z
   * gives back the typed characters.
   */
  const markdownOnSpace = (): boolean => {
    const root = editorRef.current;
    if (!root) return false;
    const context = caretContext(root);
    if (!context) return false;
    const inLink = context.node.parentElement?.closest("a");
    const link = inLink ? null : trailingUrl(context.node.data.slice(0, context.offset));
    if (link && selectBefore(context.node, context.offset, context.offset - link.start)) {
      const trailing = context.node.data.slice(link.start + link.url.length, context.offset);
      document.execCommand("insertHTML", false, `<a href="${escapeHtml(link.href)}">${escapeHtml(link.url)}</a>${escapeHtml(trailing)}&nbsp;`);
      return true;
    }
    if (context.block !== root && !PLAIN_BLOCKS.has(context.block.tagName)) return false;
    const prefix = context.before.replace(/\u00a0/g, " ");
    const rule = blockRule(prefix);
    if (!rule || rule === "hr") return false;
    if (context.before.length !== context.offset || !selectBefore(context.node, context.offset, context.offset)) return false;
    document.execCommand("delete");
    if (rule === "checked") {
      applyBlock("checklist");
      window.getSelection()?.anchorNode?.parentElement?.closest("li")?.setAttribute("data-checked", "true");
    } else applyBlock(rule === "h3" ? "h2" : (rule as BlockFormat));
    return true;
  };

  /** Inline marks complete on their closing delimiter: **b**, *i*, ~~s~~, ==m==, `c`. */
  const markdownOnInput = (event: React.FormEvent<HTMLDivElement>) => {
    const native = event.nativeEvent as InputEvent;
    if (native.inputType !== "insertText" || !native.data || !/[*_~=`]/.test(native.data)) return;
    const root = editorRef.current;
    if (!root) return;
    const context = caretContext(root);
    if (!context || context.node.parentElement?.closest("code, pre")) return;
    const match = inlineRule(context.node.data.slice(0, context.offset));
    if (!match || match.start < 0) return;
    if (!selectBefore(context.node, context.offset, context.offset - match.start)) return;
    const tag = INLINE_TAG[match.mark];
    document.execCommand("insertHTML", false, `<${tag}>${escapeHtml(match.content)}</${tag}>${CARET_GAP}`);
  };

  const onDrop = (event: React.DragEvent<HTMLDivElement>) => {
    const files = imageFiles(event.dataTransfer);
    if (!files.length) return;
    event.preventDefault();
    void insertImages(files);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const mod = event.ctrlKey || event.metaKey;
    if (event.key === " " && !mod && !event.altKey && !event.nativeEvent.isComposing) {
      if (markdownOnSpace()) {
        event.preventDefault();
        schedule();
      }
      return;
    }
    // Enter on an empty quote / code line leaves the block, as lists already do.
    if (event.key === "Enter" && !mod && !event.shiftKey && !event.nativeEvent.isComposing) {
      const selection = window.getSelection();
      const anchor = selection?.anchorNode ?? null;
      const element = anchor ? (anchor.nodeType === Node.ELEMENT_NODE ? anchor as Element : anchor.parentElement) : null;
      const block = element?.closest("blockquote, pre");
      if (block && editorRef.current?.contains(block)) {
        const inner = element?.closest("p, div");
        const line = inner && block.contains(inner) && inner !== block ? inner : block;
        if (!(line.textContent ?? "").split(CARET_GAP).join("").trim()) {
          event.preventDefault();
          if (line !== block) line.remove();
          const paragraph = document.createElement("p");
          paragraph.appendChild(document.createElement("br"));
          block.after(paragraph);
          if (!(block.textContent ?? "").trim()) block.remove();
          const range = document.createRange();
          range.setStart(paragraph, 0);
          range.collapse(true);
          selection?.removeAllRanges();
          selection?.addRange(range);
          schedule();
          return;
        }
      }
    }
    if (!mod) return;
    const key = event.key.toLowerCase();
    if (event.shiftKey && key === "x") {
      event.preventDefault();
      run(() => applyInline("strikeThrough"));
    } else if (event.shiftKey && key === "h") {
      event.preventDefault();
      run(() => applyInline("highlight"));
    } else if (event.shiftKey && key === "7") {
      event.preventDefault();
      run(() => applyBlock("ol"));
    } else if (event.shiftKey && key === "8") {
      event.preventDefault();
      run(() => applyBlock("ul"));
    } else if (event.shiftKey && key === "9") {
      event.preventDefault();
      run(() => applyBlock("checklist"));
    } else if (key === "s") {
      event.preventDefault();
      flush();
    }
  };

  // Clicking a checklist box toggles it (the box is the li's ::before).
  const onMouseDown = (event: React.MouseEvent<HTMLDivElement>) => {
    const item = (event.target as HTMLElement).closest("ul[data-type='checklist'] > li");
    if (!item) return;
    const rect = item.getBoundingClientRect();
    if (event.clientX - rect.left > 22) return;
    event.preventDefault();
    if (item.getAttribute("data-checked") === "true") item.removeAttribute("data-checked");
    else item.setAttribute("data-checked", "true");
    schedule();
  };

  const onClick = (event: React.MouseEvent<HTMLDivElement>) => {
    // Ctrl+click follows a link; a plain click edits it.
    if (event.ctrlKey || event.metaKey) handleLinkClick(event);
  };

  return (
    <div
      className="todo-editor"
      data-busy={busy ? "true" : "false"}
      data-focused={focused ? "true" : "false"}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false);
      }}
      onFocus={() => setFocused(true)}
    >
      <div aria-label="格式" className="todo-editor-toolbar" role="toolbar">
        {tools.map((tool) => (
          <button
            aria-label={tool.label}
            className="todo-tool"
            key={tool.id}
            onClick={tool.run}
            onMouseDown={(event) => event.preventDefault()}
            title={tool.shortcut ? `${tool.label}（${tool.shortcut}）` : tool.label}
            type="button"
          >
            {tool.icon}
          </button>
        ))}
      </div>
      <div
        aria-label="备注"
        aria-multiline="true"
        className="todo-editor-content todo-rich"
        contentEditable
        data-empty={empty ? "true" : "false"}
        data-placeholder={placeholder}
        onBlur={flush}
        onClick={onClick}
        onDragOver={(event) => {
          if (imageFiles(event.dataTransfer).length || event.dataTransfer.types.includes("Files")) event.preventDefault();
        }}
        onDrop={onDrop}
        onInput={(event) => {
          markdownOnInput(event);
          schedule();
        }}
        onKeyDown={onKeyDown}
        onMouseDown={onMouseDown}
        onPaste={onPaste}
        ref={(element) => {
          editorRef.current = element;
          setEditor(element);
        }}
        role="textbox"
        spellCheck={false}
        suppressContentEditableWarning
      />
    </div>
  );
}

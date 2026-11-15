import { useEffect, useLayoutEffect, useRef, type ClipboardEvent, type KeyboardEvent as ReactKeyboardEvent } from "react";

import { asSingleUrl, inlineRule, type InlineMark } from "../shared/markdown.ts";
import { MAX_TITLE_LENGTH, escapeTitle } from "../shared/model.ts";
import { openLink } from "./bridge.ts";
import { applyInline, insertLink, sanitizeTitle } from "./richText.ts";

const INLINE_TAG: Record<InlineMark, string> = { bold: "b", italic: "i", strike: "s", code: "code", highlight: "mark" };
const CARET_GAP = String.fromCharCode(0x200b);
const escapeHtml = (value: string) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

export function cleanTitle(html: string): string {
  return sanitizeTitle(html, MAX_TITLE_LENGTH);
}

/** A stored title, rendered (read-only). Links open externally. */
export function TitleView({ html, placeholder, className = "" }: { html: string; placeholder?: string; className?: string }) {
  const clean = cleanTitle(html);
  if (!clean) return placeholder ? <span className={`todo-muted ${className}`}>{placeholder}</span> : null;
  return (
    <span
      className={`todo-title-rich ${className}`}
      dangerouslySetInnerHTML={{ __html: clean }}
      onClick={(event) => {
        const anchor = (event.target as HTMLElement).closest("a[href]");
        if (!anchor) return;
        event.preventDefault();
        event.stopPropagation();
        void openLink(anchor.getAttribute("href") ?? "");
      }}
    />
  );
}

interface InlineEditorProps {
  /** Identity of the edited record; a change reloads the editor. */
  documentKey: string;
  value: string;
  placeholder: string;
  className?: string;
  ariaLabel: string;
  autoFocus?: boolean;
  disabled?: boolean;
  /** Called with sanitized inline HTML after a pause / on blur / on Enter. */
  onCommit(html: string): void;
  onEnter?(): void;
  /** Backspace in an empty field. */
  onEmptyBackspace?(): void;
}

/**
 * One-line rich-text field for titles (notes and subtasks): Ctrl+B / I / U,
 * Ctrl+Shift+X / H, Markdown marks as you type (**b**, *i*, ~~s~~, ==m==,
 * `c`), pasted URLs become links. Enter never inserts a line.
 */
export function InlineRichEditor({ documentKey, value, placeholder, className = "", ariaLabel, autoFocus = false, disabled = false, onCommit, onEnter, onEmptyBackspace }: InlineEditorProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const last = useRef(cleanTitle(value));
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const commitRef = useRef(onCommit);
  commitRef.current = onCommit;
  /** The writer bound to the record currently loaded. */
  const bound = useRef(onCommit);

  const flush = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const element = ref.current;
    if (!element) return;
    const html = cleanTitle(element.innerHTML);
    if (html !== last.current) {
      last.current = html;
      bound.current(html);
    }
  };

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    bound.current = commitRef.current;
    last.current = cleanTitle(value);
    element.innerHTML = last.current;
    element.dataset.empty = last.current ? "false" : "true";
    return () => flush();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentKey]);

  // A synchronized change while not editing.
  useEffect(() => {
    const element = ref.current;
    if (!element || document.activeElement === element) return;
    const clean = cleanTitle(value);
    if (clean !== last.current) {
      last.current = clean;
      element.innerHTML = clean;
      element.dataset.empty = clean ? "false" : "true";
    }
  }, [value]);

  useEffect(() => {
    if (!autoFocus) return;
    const element = ref.current;
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
    const element = ref.current;
    if (element) element.dataset.empty = (element.textContent ?? "").split(CARET_GAP).join("").trim() ? "false" : "true";
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, 450);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const mod = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();
    if (event.key === "Enter" && !event.nativeEvent.isComposing) {
      event.preventDefault();
      flush();
      onEnter?.();
      return;
    }
    if (event.key === "Backspace" && onEmptyBackspace && !(ref.current?.textContent ?? "").split(CARET_GAP).join("")) {
      event.preventDefault();
      onEmptyBackspace();
      return;
    }
    if (!mod) return;
    const command = key === "b" ? "bold" : key === "i" ? "italic" : key === "u" ? "underline" : event.shiftKey && key === "x" ? "strikeThrough" : event.shiftKey && key === "h" ? "highlight" : null;
    if (command) {
      event.preventDefault();
      applyInline(command);
      schedule();
    }
  };

  const onInput = (event: React.FormEvent<HTMLDivElement>) => {
    const native = event.nativeEvent as InputEvent;
    if (native.inputType === "insertText" && native.data && /[*_~=`]/.test(native.data)) {
      const selection = window.getSelection();
      const node = selection?.anchorNode;
      if (selection && node && node.nodeType === Node.TEXT_NODE && !node.parentElement?.closest("code")) {
        const text = (node as Text).data.slice(0, selection.anchorOffset);
        const match = inlineRule(text);
        if (match && match.start >= 0) {
          const range = document.createRange();
          range.setStart(node, match.start);
          range.setEnd(node, selection.anchorOffset);
          selection.removeAllRanges();
          selection.addRange(range);
          const tag = INLINE_TAG[match.mark];
          document.execCommand("insertHTML", false, `<${tag}>${escapeHtml(match.content)}</${tag}>${CARET_GAP}`);
        }
      }
    }
    schedule();
  };

  const onPaste = (event: ClipboardEvent<HTMLDivElement>) => {
    event.preventDefault();
    const text = event.clipboardData.getData("text/plain");
    const url = asSingleUrl(text);
    if (url) insertLink(url, text.trim());
    else {
      const html = event.clipboardData.getData("text/html");
      const clean = html ? cleanTitle(html) : escapeTitle(text.replace(/[\r\n]+/g, " "));
      if (clean) document.execCommand("insertHTML", false, clean);
    }
    schedule();
  };

  return (
    <div
      aria-label={ariaLabel}
      aria-disabled={disabled || undefined}
      className={`todo-inline-editor todo-title-rich ${className}`}
      contentEditable={!disabled}
      data-empty="true"
      data-placeholder={placeholder}
      onBlur={flush}
      onClick={(event) => {
        if (!(event.ctrlKey || event.metaKey)) return;
        const anchor = (event.target as HTMLElement).closest("a[href]");
        if (anchor) {
          event.preventDefault();
          void openLink(anchor.getAttribute("href") ?? "");
        }
      }}
      onDrop={(event) => event.preventDefault()}
      onInput={onInput}
      onKeyDown={onKeyDown}
      onPaste={onPaste}
      ref={ref}
      role="textbox"
      spellCheck={false}
      suppressContentEditableWarning
    />
  );
}

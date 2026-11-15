/**
 * Rich-text notes are stored as a small, sanitized HTML subset. Everything
 * the editor produces, pastes or receives through sync passes through
 * `sanitizeNotes` before it is stored AND before it is rendered, so a record
 * edited by another device or build can never inject markup, styles, event
 * handlers, remote resources or `javascript:` links.
 */

import { linkifyText } from "../shared/markdown.ts";
import { isSafeUrl, MAX_NOTES_BYTES } from "../shared/model.ts";

const BLOCK_TAGS = new Set(["P", "DIV", "H1", "H2", "H3", "UL", "OL", "LI", "BLOCKQUOTE", "PRE"]);
const INLINE_TAGS = new Set(["B", "STRONG", "I", "EM", "U", "S", "STRIKE", "DEL", "CODE", "MARK", "A", "BR", "SPAN"]);
const RENAME: Record<string, string> = { STRONG: "B", EM: "I", STRIKE: "S", DEL: "S", DIV: "P" };
/** Attachment ids are UUIDs; images are only ever local attachments. */
const ATTACHMENT_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function cleanChildren(source: Node, target: Node, doc: Document, depth: number): void {
  for (const child of Array.from(source.childNodes)) cleanNode(child, target, doc, depth);
}

function cleanNode(node: Node, target: Node, doc: Document, depth: number): void {
  if (depth > 24) return;
  if (node.nodeType === Node.TEXT_NODE) {
    // Zero-width spaces only exist to step the caret out of a just-made mark.
    const text = (node.textContent ?? "").replace(/\u200b/g, "");
    if (text) target.appendChild(doc.createTextNode(text));
    return;
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return;
  const element = node as Element;
  const tag = element.tagName.toUpperCase();
  if (tag === "IMG") {
    const id = element.getAttribute("data-attachment") ?? "";
    if (ATTACHMENT_ID.test(id)) {
      const image = doc.createElement("img");
      image.setAttribute("data-attachment", id);
      const alt = element.getAttribute("alt");
      if (alt) image.setAttribute("alt", alt.slice(0, 200));
      target.appendChild(image);
    }
    return;
  }
  if (["SCRIPT", "STYLE", "IFRAME", "OBJECT", "EMBED", "TEMPLATE", "SVG", "MATH", "HEAD", "TITLE", "META", "LINK"].includes(tag)) return;
  if (!BLOCK_TAGS.has(tag) && !INLINE_TAGS.has(tag)) {
    // Unknown wrappers (font, section, table cells…) keep only their content.
    cleanChildren(element, target, doc, depth + 1);
    return;
  }
  const name = RENAME[tag] ?? tag;
  if (name === "SPAN") {
    cleanChildren(element, target, doc, depth + 1);
    return;
  }
  const clean = doc.createElement(name.toLowerCase());
  if (name === "A") {
    const href = element.getAttribute("href") ?? "";
    if (!isSafeUrl(href)) {
      cleanChildren(element, target, doc, depth + 1);
      return;
    }
    clean.setAttribute("href", href);
  }
  if (name === "UL" && element.getAttribute("data-type") === "checklist") clean.setAttribute("data-type", "checklist");
  if (name === "LI" && element.getAttribute("data-checked") === "true") clean.setAttribute("data-checked", "true");
  cleanChildren(element, clean, doc, depth + 1);
  target.appendChild(clean);
}

/**
 * A title's stored form: the INLINE subset only (bold, italic, underline,
 * strike, code, highlight, links), one line. Block markup collapses to text
 * and line breaks become spaces.
 */
export function sanitizeTitle(html: string, maxLength: number): string {
  if (!html) return "";
  const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
  const output = document.implementation.createHTMLDocument("");
  const root = output.createElement("div");
  const walk = (source: Node, target: Node, depth: number) => {
    if (depth > 12) return;
    for (const node of Array.from(source.childNodes)) {
      if (node.nodeType === Node.TEXT_NODE) {
        const text = (node.textContent ?? "").replace(/\u200b/g, "").replace(/[\r\n]+/g, " ");
        if (text) target.appendChild(output.createTextNode(text));
        continue;
      }
      if (node.nodeType !== Node.ELEMENT_NODE) continue;
      const element = node as Element;
      const tag = RENAME[element.tagName.toUpperCase()] ?? element.tagName.toUpperCase();
      if (tag === "BR") {
        target.appendChild(output.createTextNode(" "));
        continue;
      }
      if (["SCRIPT", "STYLE", "IFRAME", "OBJECT", "EMBED", "TEMPLATE", "SVG", "MATH", "IMG"].includes(tag)) continue;
      if (!TITLE_TAGS.has(tag) || (tag === "A" && !isSafeUrl(element.getAttribute("href") ?? ""))) {
        walk(element, target, depth + 1);
        if (BLOCK_TAGS.has(tag)) target.appendChild(output.createTextNode(" "));
        continue;
      }
      const clean = output.createElement(tag.toLowerCase());
      if (tag === "A") clean.setAttribute("href", element.getAttribute("href") ?? "");
      walk(element, clean, depth + 1);
      if (clean.textContent) target.appendChild(clean);
    }
  };
  walk(doc.body, root, 0);
  if (!(root.textContent ?? "").trim()) return "";
  let result = root.innerHTML.replace(/\s{2,}/g, " ").trim();
  while (result.length > maxLength && root.lastChild) {
    root.removeChild(root.lastChild);
    result = root.innerHTML.trim();
  }
  return result;
}

const TITLE_TAGS = new Set(["B", "I", "U", "S", "CODE", "MARK", "A"]);

export function sanitizeNotes(html: string): string {
  if (!html) return "";
  const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
  const output = document.implementation.createHTMLDocument("");
  const root = output.createElement("div");
  cleanChildren(doc.body, root, output, 0);
  // Invalid nesting (a list typed inside a paragraph) leaves empty <p></p>
  // fragments behind; deliberate blank lines keep their <br>.
  for (const paragraph of Array.from(root.querySelectorAll("p"))) {
    if (!(paragraph.textContent ?? "").trim() && !paragraph.querySelector("br, img")) paragraph.remove();
  }
  // Collapse an editor that holds only empty paragraphs to "".
  const text = (root.textContent ?? "").trim();
  if (!text && !root.querySelector("img")) return "";
  let result = root.innerHTML;
  if (new TextEncoder().encode(result).length > MAX_NOTES_BYTES) {
    result = result.slice(0, MAX_NOTES_BYTES);
    return sanitizeNotes(result.slice(0, result.lastIndexOf("<")));
  }
  return result;
}

/** Plain text → safe paragraphs, bare URLs as links (pasting without formatting). */
export function textToHtml(text: string): string {
  return text
    .split(/\r?\n/)
    .map((line) => `<p>${line ? linkifyText(line) : "<br>"}</p>`)
    .join("");
}

export type InlineFormat = "bold" | "italic" | "underline" | "strikeThrough" | "code" | "highlight";
export type BlockFormat = "p" | "h1" | "h2" | "blockquote" | "pre" | "ul" | "ol" | "checklist";

/**
 * Formatting through the browser's editing commands (undo stack included),
 * normalized afterwards by `sanitizeNotes`.
 */
export function applyInline(format: InlineFormat): void {
  if (format === "code" || format === "highlight") {
    const tag = format === "code" ? "code" : "mark";
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return;
    const range = selection.getRangeAt(0);
    const existing = (range.commonAncestorContainer.nodeType === Node.ELEMENT_NODE ? range.commonAncestorContainer as Element : range.commonAncestorContainer.parentElement)?.closest(tag);
    if (existing) {
      // Toggle off: unwrap.
      const parent = existing.parentNode;
      while (existing.firstChild) parent?.insertBefore(existing.firstChild, existing);
      parent?.removeChild(existing);
      return;
    }
    const text = range.toString();
    document.execCommand("insertHTML", false, `<${tag}>${text.replace(/&/g, "&amp;").replace(/</g, "&lt;")}</${tag}>`);
    return;
  }
  document.execCommand(format);
}

export function applyBlock(format: BlockFormat): void {
  if (format === "ul") document.execCommand("insertUnorderedList");
  else if (format === "ol") document.execCommand("insertOrderedList");
  else if (format === "checklist") {
    document.execCommand("insertUnorderedList");
    const selection = window.getSelection();
    const node = selection?.anchorNode;
    const list = (node?.nodeType === Node.ELEMENT_NODE ? node as Element : node?.parentElement)?.closest("ul");
    if (list) {
      if (list.getAttribute("data-type") === "checklist") list.removeAttribute("data-type");
      else list.setAttribute("data-type", "checklist");
    }
  } else document.execCommand("formatBlock", false, format);
}

export function insertLink(url: string, label: string): void {
  const selection = window.getSelection();
  if (selection && !selection.isCollapsed) {
    document.execCommand("createLink", false, url);
    return;
  }
  const safe = (value: string) => value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
  document.execCommand("insertHTML", false, `<a href="${safe(url)}">${safe(label || url)}</a>&nbsp;`);
}

/** `src` is display-only (a data URL); the sanitizer stores just the id. */
export function insertAttachmentImage(attachmentId: string, alt: string, src: string): void {
  const safeSrc = /^data:image\/(webp|png|jpeg|gif);base64,[A-Za-z0-9+/=]+$/.test(src) ? src : "";
  document.execCommand("insertHTML", false, `<img data-attachment="${attachmentId}" alt="${alt.replace(/["<>&]/g, "")}"${safeSrc ? ` src="${safeSrc}"` : ""}><br>`);
}

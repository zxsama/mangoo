/**
 * Markdown for notes. Notes stay stored as the sanitized HTML subset (so old
 * notes, sync and the pinned view are unchanged); Markdown is how you WRITE
 * them: block prefixes and inline marks typed in the editor turn into
 * formatting as you type, and Markdown text pasted from elsewhere is
 * converted. Pure string functions (no DOM), so they are unit-tested.
 */

export type MarkdownBlock = "h1" | "h2" | "h3" | "ul" | "ol" | "checklist" | "checked" | "blockquote" | "pre" | "hr";

/**
 * Block shortcut for the text typed at the start of a block, checked when
 * the user types the space after it: "# ", "- ", "1. ", "[ ] ", "> "…
 * ("```" and "---" convert on their own, without a space.)
 */
export function blockRule(prefix: string): MarkdownBlock | null {
  switch (prefix) {
    case "#": return "h1";
    case "##": return "h2";
    case "###": return "h3";
    case "-": case "*": case "+": return "ul";
    case "[]": case "[ ]": case "- []": case "- [ ]": return "checklist";
    case "[x]": case "[X]": case "- [x]": case "- [X]": return "checked";
    case ">": return "blockquote";
    case "```": return "pre";
    case "---": case "***": return "hr";
    default: return /^\d{1,3}[.)]$/.test(prefix) ? "ol" : null;
  }
}

export type InlineMark = "bold" | "italic" | "strike" | "code" | "highlight";

export interface InlineMatch {
  mark: InlineMark;
  /** Index in the text where the opening delimiter starts. */
  start: number;
  /** The text between the delimiters. */
  content: string;
}

const INLINE_RULES: Array<{ mark: InlineMark; pattern: RegExp }> = [
  { mark: "bold", pattern: /\*\*([^*\s](?:[^*]*[^*\s])?)\*\*$/ },
  { mark: "bold", pattern: /__([^_\s](?:[^_]*[^_\s])?)__$/ },
  { mark: "strike", pattern: /~~([^~\s](?:[^~]*[^~\s])?)~~$/ },
  { mark: "highlight", pattern: /==([^=\s](?:[^=]*[^=\s])?)==$/ },
  { mark: "code", pattern: /`([^`]+)`$/ },
  { mark: "italic", pattern: /(?:^|[^*\w])\*([^*\s](?:[^*]*[^*\s])?)\*$/ },
  { mark: "italic", pattern: /(?:^|[^_\w])_([^_\s](?:[^_]*[^_\s])?)_$/ }
];

/**
 * Inline mark completed by the character just typed (the text up to the
 * caret ends with the closing delimiter): "**bold**", "`code`", "~~gone~~".
 */
export function inlineRule(beforeCaret: string): InlineMatch | null {
  for (const rule of INLINE_RULES) {
    const match = rule.pattern.exec(beforeCaret);
    if (!match) continue;
    const content = match[1];
    const delimiter = rule.mark === "italic" || rule.mark === "code" ? 1 : 2;
    const start = beforeCaret.length - content.length - delimiter * 2;
    return { mark: rule.mark, start, content };
  }
  return null;
}

const URL_PATTERN = /\b((?:https?:\/\/|mailto:|www\.)[^\s<>"'，。；、）】」]+[^\s<>"'，。；、）】」.,;:!?)\]])/gi;

/** A whole string that is just one link (what a pasted URL looks like). */
export function asSingleUrl(text: string): string | null {
  const value = text.trim();
  if (!value || /\s/.test(value)) return null;
  if (/^(https?:\/\/|mailto:)/i.test(value)) return value;
  if (/^www\.[^\s]+\.[a-z]{2,}/i.test(value)) return `https://${value}`;
  return null;
}

/** The URL that ends `beforeCaret` (typed then followed by a space), if any. */
export function trailingUrl(beforeCaret: string): { start: number; url: string; href: string } | null {
  const match = /(?:^|\s)((?:https?:\/\/|www\.)[^\s]+)$/i.exec(beforeCaret);
  if (!match) return null;
  let url = match[1];
  while (/[.,;:!?)\]，。；、]$/.test(url)) url = url.slice(0, -1);
  const href = asSingleUrl(url);
  if (!href) return null;
  return { start: beforeCaret.length - match[1].length, url, href };
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Escape text and turn bare URLs into links. */
export function linkifyText(text: string): string {
  let out = "";
  let last = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    const index = match.index ?? 0;
    out += escapeHtml(text.slice(last, index));
    const href = asSingleUrl(match[1]) ?? match[1];
    out += `<a href="${escapeHtml(href)}">${escapeHtml(match[1])}</a>`;
    last = index + match[1].length;
  }
  return out + escapeHtml(text.slice(last));
}

/** Inline Markdown → HTML (on already-split lines). */
export function inlineMarkdown(text: string): string {
  // Code spans first, protected from the other rules.
  const codes: string[] = [];
  let source = text.replace(/`([^`]+)`/g, (_match, code: string) => {
    codes.push(`<code>${escapeHtml(code)}</code>`);
    return `\u0000${codes.length - 1}\u0000`;
  });
  // [label](url)
  const links: string[] = [];
  source = source.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (match, label: string, url: string) => {
    const href = asSingleUrl(url);
    if (!href) return match;
    links.push(`<a href="${escapeHtml(href)}">${escapeHtml(label)}</a>`);
    return `\u0001${links.length - 1}\u0001`;
  });
  let html = linkifyText(source);
  html = html
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/__([^_]+)__/g, "<b>$1</b>")
    .replace(/~~([^~]+)~~/g, "<s>$1</s>")
    .replace(/==([^=]+)==/g, "<mark>$1</mark>")
    .replace(/(^|[^*\w])\*([^*\s][^*]*?)\*(?!\w)/g, "$1<i>$2</i>")
    .replace(/(^|[^_\w])_([^_\s][^_]*?)_(?!\w)/g, "$1<i>$2</i>");
  html = html.replace(/\u0001(\d+)\u0001/g, (_match, index: string) => links[Number(index)]);
  return html.replace(/\u0000(\d+)\u0000/g, (_match, index: string) => codes[Number(index)]);
}

/** Does pasted plain text look like Markdown (worth converting)? */
export function looksLikeMarkdown(text: string): boolean {
  return /^(#{1,3} |[-*+] |\d{1,3}[.)] |> |```|- \[[ xX]\] |\[[ xX]?\] )/m.test(text)
    || /\*\*[^*]+\*\*|~~[^~]+~~|`[^`]+`|\[[^\]]+\]\([^)]+\)/.test(text);
}

/**
 * Markdown text → the notes HTML subset (headings, lists, checklists,
 * quotes, code blocks, rules, inline marks, links). The result still goes
 * through the sanitizer before it is stored.
 */
export function markdownToHtml(markdown: string): string {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  let list: { tag: "ul" | "ol"; checklist: boolean } | null = null;
  let quote: string[] | null = null;
  const closeList = () => {
    if (list) out.push(`</${list.tag}>`);
    list = null;
  };
  const closeQuote = () => {
    if (quote) out.push(`<blockquote>${quote.map((line) => `<p>${inlineMarkdown(line) || "<br>"}</p>`).join("")}</blockquote>`);
    quote = null;
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (/^```/.test(line.trim())) {
      closeList();
      closeQuote();
      const code: string[] = [];
      index += 1;
      while (index < lines.length && !/^```/.test(lines[index].trim())) code.push(lines[index++]);
      out.push(`<pre>${escapeHtml(code.join("\n"))}</pre>`);
      continue;
    }
    const quoted = /^>\s?(.*)$/.exec(line);
    if (quoted) {
      closeList();
      (quote ??= []).push(quoted[1]);
      continue;
    }
    closeQuote();
    const heading = /^(#{1,3})\s+(.*)$/.exec(line);
    if (heading) {
      closeList();
      const level = Math.min(heading[1].length, 2);
      out.push(`<h${level}>${inlineMarkdown(heading[2])}</h${level}>`);
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      closeList();
      continue;
    }
    const check = /^\s*(?:[-*+]\s+)?\[([ xX]?)\]\s+(.*)$/.exec(line);
    const bullet = /^\s*[-*+]\s+(.*)$/.exec(line);
    const ordered = /^\s*\d{1,3}[.)]\s+(.*)$/.exec(line);
    if (check || bullet || ordered) {
      const tag = ordered && !check ? "ol" : "ul";
      const checklist = Boolean(check);
      if (!list || list.tag !== tag || list.checklist !== checklist) {
        closeList();
        out.push(checklist ? `<ul data-type="checklist">` : `<${tag}>`);
        list = { tag, checklist };
      }
      const text = check ? check[2] : (bullet ?? ordered)?.[1] ?? "";
      const checked = check && check[1].toLowerCase() === "x";
      out.push(`<li${checked ? ` data-checked="true"` : ""}>${inlineMarkdown(text)}</li>`);
      continue;
    }
    closeList();
    // Blank lines separate Markdown blocks; they are not empty paragraphs.
    // Only a run of two or more keeps one deliberate gap.
    if (!line.trim()) {
      if (index > 0 && !lines[index - 1].trim() && out[out.length - 1] !== "<p><br></p>") out.push("<p><br></p>");
      continue;
    }
    out.push(`<p>${inlineMarkdown(line)}</p>`);
  }
  closeList();
  closeQuote();
  return out.join("");
}

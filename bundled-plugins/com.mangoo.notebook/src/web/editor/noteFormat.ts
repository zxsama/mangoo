export const attachmentScheme = "mangoo-attachment:";

export function attachmentId(url: string): string | undefined {
  const match = /^mangoo-attachment:([\w.-]+)$/.exec(url);
  return match?.[1];
}

export function isImageFile(file: Pick<File, "type" | "name">): boolean {
  return file.type.startsWith("image/") || /\.(?:avif|bmp|gif|ico|jpe?g|png|svg|tiff?|webp)$/i.test(file.name);
}

export function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function attachmentMarkup(format: "html" | "markdown", id: string, file: File): string {
  const url = attachmentScheme + id;
  const name = (file.name || "image").replace(/[\r\n]/g, " ");
  if (format === "markdown") return `![${name.replace(/[\\\[\]]/g, "\\$&")}](${url})`;
  return isImageFile(file)
    ? `<img src="${escapeHtml(url)}" alt="${escapeHtml(name)}">`
    : `<a href="${escapeHtml(url)}">${escapeHtml(name)}</a>`;
}

export const elements = {
  body: ["正文", "Body", "body"], h1: ["一级标题", "Heading 1", "h1"],
  h2: ["二级标题", "Heading 2", "h2"], h3: ["三级标题", "Heading 3", "h3"],
  h4: ["四级标题", "Heading 4", "h4"], h5: ["五级标题", "Heading 5", "h5"],
  h6: ["六级标题", "Heading 6", "h6"], strong: ["粗体", "Bold", "strong,b"],
  em: ["斜体", "Italic", "em,i"], strike: ["删除线", "Strikethrough", "del,s"],
  quote: ["引用", "Block quote", "blockquote"], code: ["代码", "Code", "pre,code"],
  list: ["列表", "List", "li"], link: ["链接", "Link", "a"],
  table: ["表格", "Table", "table"], image: ["图片", "Image", "img"]
} as const;
export type StyleElement = keyof typeof elements;
export type ElementStyle = {
  fontSize?: number; lineHeight?: number; color?: string; fontWeight?: number; width?: number;
  fontFamily?: "sans-serif" | "serif" | "monospace"; background?: string; customCss?: string;
};
export type NoteEditorStyles = Partial<Record<StyleElement, ElementStyle>>;

function numberIn(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= min && value <= max;
}

function normalizeElementStyle(entry: unknown): ElementStyle {
  if (!entry || typeof entry !== "object") return {};
  const raw = entry as Record<string, unknown>;
  const style: ElementStyle = {};
  if (numberIn(raw.fontSize, 8, 96)) style.fontSize = raw.fontSize;
  if (numberIn(raw.lineHeight, 1, 3)) style.lineHeight = raw.lineHeight;
  if (numberIn(raw.fontWeight, 100, 900)) style.fontWeight = raw.fontWeight;
  if (numberIn(raw.width, 10, 100)) style.width = raw.width;
  for (const color of ["color", "background"] as const) {
    const hex = raw[color];
    if (typeof hex === "string" && /^#[a-f\d]{6}$/i.test(hex)) style[color] = hex;
  }
  if (raw.fontFamily === "sans-serif" || raw.fontFamily === "serif" || raw.fontFamily === "monospace") {
    style.fontFamily = raw.fontFamily;
  }
  if (typeof raw.customCss === "string" && raw.customCss) style.customCss = raw.customCss;
  return style;
}

export function normalizeNoteEditorStyles(value: unknown): NoteEditorStyles {
  const normalized: NoteEditorStyles = {};
  if (!value || typeof value !== "object") return normalized;
  for (const key of Object.keys(elements) as StyleElement[]) {
    const entry = (value as Record<string, unknown>)[key];
    const style = normalizeElementStyle(entry);
    if (Object.keys(style).length) normalized[key] = style;
  }
  return normalized;
}

const cssPropertyForField = {
  color: "color", background: "background-color", width: "width", fontFamily: "font-family",
  fontSize: "font-size", fontWeight: "font-weight", lineHeight: "line-height"
} as const;

export function mergeNoteEditorStyles(defaults: NoteEditorStyles, overrides: unknown): NoteEditorStyles {
  const merged = normalizeNoteEditorStyles(defaults);
  const localStyles = normalizeNoteEditorStyles(overrides);
  for (const element of Object.keys(localStyles) as StyleElement[]) {
    const local = localStyles[element]!;
    const inherited = merged[element];
    const style = { ...inherited, ...local };
    if (inherited?.customCss) {
      // A per-note field also overrides the same property in the device's custom CSS.
      const overridden = new Set(Object.entries(cssPropertyForField)
        .filter(([field]) => Object.hasOwn(local, field)).map(([, property]) => String(property)));
      const inheritedCss = inherited.customCss.split(";").filter(declaration =>
        !overridden.has(declaration.split(":", 1)[0].trim().toLowerCase()));
      style.customCss = [...inheritedCss, local.customCss].filter(Boolean).join(";");
    }
    merged[element] = style;
  }
  return merged;
}

const editableProperties = /^(?:color|background-color|font-(?:family|size|style|weight|variant)|line-height|letter-spacing|word-spacing|text-(?:align|decoration(?:-color|-style|-thickness)?|indent|transform)|margin(?:-(?:top|right|bottom|left|block|inline)(?:-start|-end)?)?|padding(?:-(?:top|right|bottom|left|block|inline)(?:-start|-end)?)?|border(?:-(?:top|right|bottom|left|inline-start|inline-end|block-start|block-end|radius|color|width|style))?|width|max-width)$/;

// Only typography, spacing and element borders can enter the host stylesheet.
// Browser validation permits installed font names and ordinary CSS length/color functions.
export function customDeclarations(input: unknown): string[] {
  if (typeof input !== "string") return [];
  return input.split(";").flatMap(entry => {
    const match = /^\s*([a-z-]+)\s*:\s*(.+?)\s*$/iu.exec(entry);
    if (!match) return [];
    const property = match[1].toLowerCase(), value = match[2];
    if (!editableProperties.test(property) || !/^[\p{L}\p{N}\s#%.,()/"'+*\-]+$/u.test(value) ||
      /(?:url|image|var|attr|env)\s*\(/i.test(value) || !CSS.supports(property, value)) return [];
    return [property + ":" + value];
  });
}
export function styleDeclarations(style?: ElementStyle): string {
  if (!style || typeof style !== "object") return "";
  const css: string[] = [];
  const normalized = normalizeElementStyle(style);
  for (const field of Object.keys(cssPropertyForField) as (keyof typeof cssPropertyForField)[]) {
    const value = normalized[field];
    if (value !== undefined) {
      const suffix = field === "width" ? "%" : field === "fontSize" ? "px" : "";
      css.push(cssPropertyForField[field] + ":" + value + suffix);
    }
  }
  css.push(...customDeclarations(style.customCss));
  return css.join(";");
}

export function elementStylesheet(styles: NoteEditorStyles, html: boolean): string {
  return (Object.keys(elements) as StyleElement[]).map((key) => {
    const css = styleDeclarations(styles?.[key]);
    if (!css) return "";
    const selector = html ? elements[key][2] : key === "body" ? ".nb-writing .cm-content" : `.nb-writing .nb-el-${key}`;
    return `${selector}{${css}}`;
  }).join("\n");
}

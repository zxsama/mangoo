// Match CSS strings and comments as complete tokens, so their literal "url(...)"
// text stays untouched. URL arguments may use CSS escapes, including hex escapes.
const tokens = /\/\*[\s\S]*?(?:\*\/|$)|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|(?<![-\w\u0080-\uFFFF])url\(\s*(?:"((?:\\[\s\S]|[^"\\])*)"|'((?:\\[\s\S]|[^'\\])*)'|((?:\\(?:[\da-f]{1,6}\s?|[\s\S])|[^)\s'"\\])*))\s*\)/gi;

function cssUnescape(value: string): string {
  return value.replace(/\\(?:\r\n|[\n\r\f]|([\da-f]{1,6})\s?|([\s\S]))/gi, (_, hex: string | undefined, literal: string | undefined) => {
    if (!hex) return literal ?? "";
    const code = parseInt(hex, 16);
    return String.fromCodePoint(code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff) ? code : 0xfffd);
  });
}

type Rewrite = (url: string) => Promise<string | undefined>;

export async function rewriteCssImageUrls(css: string, rewrite: Rewrite): Promise<string> {
  let output = "", offset = 0;
  for (const match of css.matchAll(tokens)) {
    const argument = match[1] ?? match[2] ?? match[3];
    if (!argument || /^(?:data:|#)/i.test(argument)) continue;
    const changed = await rewrite(cssUnescape(argument));
    if (changed === undefined) continue;
    const quoted = changed.replace(/["\\]/g, "\\$&").replace(/[\r\n\f]/g, "");
    output += css.slice(offset, match.index) + 'url("' + quoted + '")';
    offset = match.index + match[0].length;
  }
  return offset ? output + css.slice(offset) : css;
}

export async function rewriteDocumentCss(doc: Document, rewrite: Rewrite): Promise<void> {
  for (const element of doc.querySelectorAll("style")) {
    element.textContent = await rewriteCssImageUrls(element.textContent ?? "", rewrite);
  }
  for (const element of doc.querySelectorAll("[style]")) {
    element.setAttribute("style", await rewriteCssImageUrls(element.getAttribute("style")!, rewrite));
  }
}

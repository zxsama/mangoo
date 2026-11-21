export interface DocumentPage {
  title: string;
  html: string;
}

export function escapeMarkup(text: string): string {
  return text.replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
}

export function readerDocument(html: string): string {
  const css = `html{color-scheme:light}body{margin:0;padding:18px 24px;color:#242831;background:#faf9f6;font:16px/1.65 system-ui,sans-serif;overflow-wrap:anywhere}h1,h2,h3{line-height:1.3}table{border-collapse:collapse}td,th{border:1px solid #ccd1d2;padding:.3em .65em;white-space:pre-wrap}img{max-width:100%;height:auto}pre{white-space:pre-wrap}blockquote{margin-inline:0;border-left:3px solid #a8b4bd;padding-left:1em}p{margin:.45em 0}`;
  return `<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><style>${css}</style><body>${html}</body>`;
}

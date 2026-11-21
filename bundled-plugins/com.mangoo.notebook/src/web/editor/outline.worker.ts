import { parser } from "@lezer/markdown";

export interface OutlineHeading {
  from: number;
  level: number;
  text: string;
}

// Parse outside the editor so a long document does not block a keystroke.
self.onmessage = ({ data }: MessageEvent<{ source: string; version: number }>) => {
  const { source, version } = data;
  const headings: OutlineHeading[] = [];
  parser.parse(source).iterate({ enter(node) {
    const match = /^(?:ATX|Setext)Heading([1-6])$/.exec(node.name);
    if (!match) return;
    const heading = source.slice(node.from, node.to)
      .replace(/\r?\n[ \t]*[=\-]+[ \t]*$/, "")
      .replace(/^ {0,3}#{1,6}(?:[ \t]+|$)/, "")
      .replace(/[ \t]+#+[ \t]*$/, "")
      .replace(/!?\[([^\]]+)\]\([^)]+\)/g, "$1")
      .replace(/(\*\*|__|`)(.+?)\1/g, "$2")
      .replace(/\s+/g, " ").trim();
    if (heading) headings.push({ from: node.from, level: Number(match[1]), text: heading });
    return false;
  } });
  self.postMessage({ version, headings });
};

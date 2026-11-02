import type { MediaRecord } from "../../core/types";

export type FileCategory = "image" | "video" | "document" | "archive" | "code" | "audio" | "font" | "model" | "file";

const categories: ReadonlyArray<readonly [FileCategory, readonly string[]]> = [
  ["image", ["jpg", "jpeg", "png", "gif", "webp", "tif", "tiff", "bmp", "avif", "heic", "heif", "svg", "psd", "ai", "ico", "ppm"]],
  ["video", ["mp4", "mov", "mkv", "webm", "avi", "m4v", "wmv"]],
  ["document", ["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "odt", "epub"]],
  ["archive", ["zip", "7z", "rar", "tar", "gz", "bz2", "xz"]],
  ["code", ["js", "jsx", "ts", "tsx", "rs", "py", "html", "css", "json", "yaml", "yml", "xml", "md", "sh", "toml", "c", "cpp", "h", "java", "cs"]],
  ["audio", ["mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "aiff"]],
  ["font", ["ttf", "otf", "woff", "woff2"]],
  ["model", ["obj", "stl", "fbx", "gltf", "glb", "blend", "3ds"]]
];
const palette = ["#77b9fa", "#c5a0f5", "#f2919c", "#e8bb72", "#81c9b1", "#e99ed3", "#a8b8f4", "#a6c977"];

/** Presentation only: these categories never decide decoder or original-read eligibility. */
export function resolveFileFormat(file: Pick<MediaRecord, "name" | "ext">) {
  const name = file.name.split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  const suffix = file.ext?.trim().replace(/^\.+/, "") || (dot > 0 ? name.slice(dot + 1) : "");
  // Control and directional formatting characters must not leak into an overlay label.
  const extension = suffix.replace(/[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, "").trim().toLowerCase();
  const category = categories.find(([, extensions]) => extensions.includes(extension))?.[0] ?? "file";
  let hash = 0;
  for (const character of extension) hash = (hash * 31 + character.codePointAt(0)!) >>> 0;
  return {
    extension,
    label: extension ? extension.toUpperCase() : "FILE",
    showBadge: !["jpg", "jpeg", "png"].includes(extension),
    category,
    color: palette[hash % palette.length]
  };
}

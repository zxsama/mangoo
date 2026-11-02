import { Box, File, FileArchive, FileAudio, FileCode2, FileImage, FileText, FileType, FileVideo } from "../../design/icons";
import type { CSSProperties } from "react";
import type { MediaRecord } from "../../core/types";
import { resolveFileFormat } from "./fileFormat";

const icons = { image: FileImage, video: FileVideo, document: FileText, archive: FileArchive, code: FileCode2, audio: FileAudio, font: FileType, model: Box, file: File };
type FormatFile = Pick<MediaRecord, "name" | "ext">;
type FormatStyle = CSSProperties & { "--file-format-color": string };

export function FileFormatBadge({ file }: { file: FormatFile }) {
  const format = resolveFileFormat(file);
  if (!format.showBadge) return null;
  return <span className="file-format-badge" style={{ "--file-format-color": format.color } as FormatStyle} title={format.label}>{format.label}</span>;
}

/** Bundled SVG artwork: available immediately and without network or decoder work. */
export function FileFormatArtwork({ file }: { file: FormatFile }) {
  const format = resolveFileFormat(file);
  const Icon = icons[format.category];
  return (
    <span className="file-format-artwork" data-file-category={format.category} role="img" aria-label={`${format.label} file`} style={{ "--file-format-color": format.color } as FormatStyle}>
      <Icon aria-hidden="true" weight="light" />
      <span className="file-format-artwork-label" title={format.label}>{format.label}</span>
    </span>
  );
}

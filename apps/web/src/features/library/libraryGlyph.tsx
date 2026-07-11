import {
  Aperture,
  Archive,
  Bookmark,
  Box,
  Camera,
  Film,
  FolderHeart,
  Heart,
  Image,
  Layers,
  Library,
  Music,
  Palette,
  Sparkles,
  Star,
  Video
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

// Preset visual identity for libraries. Each library stores an `icon` (one of
// these keys) plus a `color`. Kept as a small curated set so the picker stays
// simple and the sidebar/switcher render a known component (no arbitrary SVG).
export const LIBRARY_ICONS: { key: string; Icon: LucideIcon }[] = [
  { key: "library", Icon: Library },
  { key: "image", Icon: Image },
  { key: "camera", Icon: Camera },
  { key: "film", Icon: Film },
  { key: "video", Icon: Video },
  { key: "music", Icon: Music },
  { key: "palette", Icon: Palette },
  { key: "aperture", Icon: Aperture },
  { key: "layers", Icon: Layers },
  { key: "archive", Icon: Archive },
  { key: "box", Icon: Box },
  { key: "bookmark", Icon: Bookmark },
  { key: "star", Icon: Star },
  { key: "heart", Icon: Heart },
  { key: "folder-heart", Icon: FolderHeart },
  { key: "sparkles", Icon: Sparkles }
];

export const LIBRARY_COLORS: string[] = [
  "#7cc4ff",
  "#5ce1a6",
  "#ffd166",
  "#ff8c69",
  "#f06595",
  "#b197fc",
  "#4dd0e1",
  "#a3e635",
  "#ff922b",
  "#e5e7eb"
];

export const DEFAULT_LIBRARY_ICON = "library";
export const DEFAULT_LIBRARY_COLOR = "#7cc4ff";

const ICON_BY_KEY = new Map(LIBRARY_ICONS.map(({ key, Icon }) => [key, Icon]));

export function resolveLibraryIcon(icon?: string): LucideIcon {
  return (icon && ICON_BY_KEY.get(icon)) || Library;
}

export function LibraryGlyph({
  icon,
  color,
  size = 15
}: {
  icon?: string;
  color?: string;
  size?: number;
}) {
  const Icon = resolveLibraryIcon(icon);
  return (
    <Icon
      size={size}
      aria-hidden="true"
      style={{ color: color || DEFAULT_LIBRARY_COLOR }}
    />
  );
}

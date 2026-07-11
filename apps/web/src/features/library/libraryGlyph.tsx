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

// Preset icon + color grid, anchored under whatever opens it. Shared by the
// library switcher popup and the Settings library-management tab so both edit
// the same preset set identically. `onPick` receives the full (icon, color)
// pair each click — icons keep the current color, swatches keep the current
// icon — so the parent just persists it.
export function LibraryIconPicker({
  icon,
  color,
  onPick
}: {
  icon?: string;
  color?: string;
  onPick: (icon: string, color: string) => void;
}) {
  const currentIcon = icon ?? DEFAULT_LIBRARY_ICON;
  const currentColor = color ?? DEFAULT_LIBRARY_COLOR;
  return (
    <div className="library-icon-picker">
      <div className="library-icon-picker-grid">
        {LIBRARY_ICONS.map(({ key }) => (
          <button
            key={key}
            type="button"
            className={`library-icon-picker-cell${key === currentIcon ? " selected" : ""}`}
            aria-label={key}
            onClick={() => onPick(key, currentColor)}
          >
            <LibraryGlyph icon={key} color={currentColor} size={16} />
          </button>
        ))}
      </div>
      <div className="library-icon-picker-colors">
        {LIBRARY_COLORS.map((c) => (
          <button
            key={c}
            type="button"
            className={`library-icon-picker-swatch${c === currentColor ? " selected" : ""}`}
            aria-label={c}
            style={{ background: c }}
            onClick={() => onPick(currentIcon, c)}
          />
        ))}
      </div>
    </div>
  );
}

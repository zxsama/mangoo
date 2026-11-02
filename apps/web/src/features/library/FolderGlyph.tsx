import { IdentityArtwork, IDENTITY_ICONS } from "./identityArtwork";
import { FOLDER_OPEN_FLUENT_PATHS } from "./identityFluentPaths";

export const DEFAULT_FOLDER_ICON = "folder";
export const DEFAULT_FOLDER_COLOR = "#ffffff";
const keys = new Set<string>(IDENTITY_ICONS.map(({ key }) => key));

/** Unknown keys are presentation fallbacks only; persisted metadata is retained. */
export function FolderGlyph({ icon, color, size = 16, open = false }: {
  icon?: string | null;
  color?: string | null;
  size?: number;
  open?: boolean;
}) {
  const resolvedIcon = icon && keys.has(icon) ? icon : DEFAULT_FOLDER_ICON;
  if (open && resolvedIcon === DEFAULT_FOLDER_ICON) {
    return <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" focusable="false" style={{ color: color ?? DEFAULT_FOLDER_COLOR }} data-identity-icon="folder-open" data-identity-variant="symbol">
      {FOLDER_OPEN_FLUENT_PATHS.map(({ d, fillRule }, index) => <path key={index} d={d} fillRule={fillRule} />)}
    </svg>;
  }
  return <IdentityArtwork color={color ?? DEFAULT_FOLDER_COLOR} icon={resolvedIcon} size={size} variant="symbol" />;
}

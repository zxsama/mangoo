import { IDENTITY_FLUENT_PATHS, type IdentityPath } from "./identityFluentPaths";
import { resolveIdentityIcon } from "./identityCatalog";
import { IDENTITY_TYPOGRAPHIC_PATHS } from "./identityTypographicPaths";

export { IDENTITY_CATEGORIES, IDENTITY_ICONS, matchesIdentityIcon, resolveIdentityIcon } from "./identityCatalog";
export type { IdentityCategory, IdentityIconDescriptor } from "./identityCatalog";

export interface IdentityArtworkProps {
  icon?: string;
  color?: string;
  size?: number;
  variant?: "library" | "symbol";
  className?: string;
}

function Paths({ paths }: { paths: readonly IdentityPath[] }) {
  return <>{paths.map(({ d, fillRule }, index) => <path key={index} d={d} fillRule={fillRule} />)}</>;
}

/** Object faces, never a tile or control boundary around the identity. */
function ObjectDetails({ icon }: { icon: string }) {
  switch (icon) {
    case "library":
      return <>
        <path fill="#000" opacity=".2" d="M3.5 3H5.5C6.328 3 7 3.672 7 4.5V19.5C7 20.328 6.328 21 5.5 21H3.5C2.672 21 2 20.328 2 19.5V4.5C2 3.672 2.672 3 3.5 3Z" />
        <path fill="#000" opacity=".1" d="M18.781 6.1238L21.995 18.6428C22.201 19.4448 21.718 20.2628 20.915 20.4688L19.039 20.9498C18.236 21.1558 17.419 20.6728 17.213 19.8698L13.999 7.3528C13.793 6.5498 14.276 5.7328 15.079 5.5268L16.955 5.0438C17.758 4.8388 18.575 5.3228 18.781 6.1238Z" />
        <path fill="#fff" opacity=".65" d="M3.5 5H5.5V6H3.5ZM9.5 5H11.5V6H9.5Z" />
        <path fill="#000" opacity=".22" d="M3.5 16H5.5V17H3.5ZM9.5 16H11.5V17H9.5Z" />
      </>;
    case "archive":
      return <>
        <path fill="#000" opacity=".18" d="M4 9H20V10.5H4ZM4.5 3H19.5C20.328 3 21 3.672 21 4.5V6C21 6.828 20.328 7.5 19.5 7.5H4.5C3.672 7.5 3 6.828 3 6V4.5C3 3.672 3.672 3 4.5 3Z" />
        <path fill="#fff" opacity=".5" d="M5 4H19V5H5Z" />
      </>;
    case "box":
      return <path fill="#000" opacity=".18" d="M22 7.19118L12.7498 10.7651V21.6883C12.9736 21.6426 13.1943 21.5762 13.4089 21.4892L20.9075 18.4493C21.5679 18.1815 22 17.5401 22 16.8275V7.19118Z" />;
    case "book":
    case "notebook":
      return <path fill="#000" opacity=".16" d="M5 4H7V19H5Z" />;
    default:
      return null;
  }
}

/** Offline, tintable identity artwork; no SVG IDs or renderer-owned selection. */
export function IdentityArtwork({ icon, color, size = 24, variant = "library", className }: IdentityArtworkProps) {
  const entry = resolveIdentityIcon(icon);
  const paths = entry.sourceKind === "typographic"
    ? { filled: IDENTITY_TYPOGRAPHIC_PATHS[entry.key], regular: IDENTITY_TYPOGRAPHIC_PATHS[entry.key] }
    : IDENTITY_FLUENT_PATHS[entry.key];
  const layered = variant === "library";
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
      className={className}
      style={{ color: color || "#ffffff" }}
      data-identity-icon={entry.key}
      data-identity-variant={variant}
    >
      <g data-identity-layer="body"><Paths paths={paths.filled} /></g>
      {layered && <g data-identity-layer="detail">
        <g fill="#000" opacity=".12"><Paths paths={paths.regular} /></g>
        <ObjectDetails icon={entry.key} />
      </g>}
      {entry.key === "folder-heart" && <g transform="translate(6 7) scale(.5)">
        <g fill="#000" opacity={layered ? ".26" : ".4"}><Paths paths={IDENTITY_FLUENT_PATHS.heart.filled} /></g>
      </g>}
      {entry.key === "aperture" && <g fill="currentColor">
        <path fillRule="evenodd" d="M18.2 12A6.2 6.2 0 1 1 5.8 12A6.2 6.2 0 1 1 18.2 12ZM15 12L13.5 14.598L10.5 14.598L9 12L10.5 9.402L13.5 9.402Z" />
        {[0, 60, 120, 180, 240, 300].map((angle) => <path key={angle} transform={`rotate(${angle} 12 12)`} fill="none" stroke="#000" opacity=".32" strokeWidth=".8" d="M22 12L13.5 14.598" />)}
      </g>}
    </svg>
  );
}

const LIBRARY_GRID_PREFERENCES_STORAGE_KEY = "megle.library.grid-preferences";

export interface LibraryGridPreferences {
  folderEdgeShadowAlpha: number;
  folderTileGap: number;
  // Real CSS line-height (px) for a folder tile's name — the name renders on up
  // to two lines with that line-height, so descenders are never clipped.
  folderTileLabelLineHeight: number;
  // Vertical gap (px) between a folder thumbnail and its name block.
  folderTileLabelGap: number;
  tileGap: number;
  // Real CSS line-height (px) for a media tile's name (two-line clamp).
  tileLabelLineHeight: number;
  // Vertical gap (px) between a media thumbnail and its name block.
  tileLabelGap: number;
}

export const DEFAULT_LIBRARY_GRID_PREFERENCES: LibraryGridPreferences = {
  folderEdgeShadowAlpha: 25,
  folderTileGap: 7,
  folderTileLabelLineHeight: 15,
  folderTileLabelGap: 4,
  tileGap: 7,
  tileLabelLineHeight: 15,
  tileLabelGap: 4
};

export const LIBRARY_GRID_PREFERENCE_LIMITS = {
  folderEdgeShadowAlpha: { min: 0, max: 75, step: 1 },
  folderTileGap: { min: 4, max: 20, step: 1 },
  folderTileLabelLineHeight: { min: 12, max: 26, step: 1 },
  folderTileLabelGap: { min: 0, max: 16, step: 1 },
  tileGap: { min: 4, max: 20, step: 1 },
  tileLabelLineHeight: { min: 12, max: 26, step: 1 },
  tileLabelGap: { min: 0, max: 16, step: 1 }
} as const;

export function readStoredLibraryGridPreferences(): LibraryGridPreferences {
  try {
    const stored = window.localStorage.getItem(LIBRARY_GRID_PREFERENCES_STORAGE_KEY);
    if (!stored) {
      return DEFAULT_LIBRARY_GRID_PREFERENCES;
    }
    const parsed = JSON.parse(stored) as Partial<LibraryGridPreferences> | null;
    return normalizeLibraryGridPreferences(parsed ?? {});
  } catch {
    return DEFAULT_LIBRARY_GRID_PREFERENCES;
  }
}

export function storeLibraryGridPreferences(value: LibraryGridPreferences) {
  try {
    window.localStorage.setItem(
      LIBRARY_GRID_PREFERENCES_STORAGE_KEY,
      JSON.stringify(normalizeLibraryGridPreferences(value))
    );
  } catch {
    // Ignore storage failures in hardened/browser-restricted environments.
  }
}

export function normalizeLibraryGridPreferences(
  value: Partial<LibraryGridPreferences>
): LibraryGridPreferences {
  const tileGap = clampField(value.tileGap, "tileGap");
  const tileLabelLineHeight = clampField(value.tileLabelLineHeight, "tileLabelLineHeight");
  const tileLabelGap = clampField(value.tileLabelGap, "tileLabelGap");
  return {
    folderEdgeShadowAlpha: clampField(value.folderEdgeShadowAlpha, "folderEdgeShadowAlpha"),
    folderTileGap: clampField(value.folderTileGap, "folderTileGap"),
    folderTileLabelLineHeight: clampField(
      value.folderTileLabelLineHeight,
      "folderTileLabelLineHeight"
    ),
    folderTileLabelGap: clampField(value.folderTileLabelGap, "folderTileLabelGap"),
    tileGap,
    tileLabelLineHeight,
    tileLabelGap
  };
}

function clampField(value: number | undefined, key: keyof LibraryGridPreferences) {
  return clampPreference(
    value,
    LIBRARY_GRID_PREFERENCE_LIMITS[key].min,
    LIBRARY_GRID_PREFERENCE_LIMITS[key].max,
    DEFAULT_LIBRARY_GRID_PREFERENCES[key]
  );
}

function clampPreference(
  value: number | undefined,
  min: number,
  max: number,
  fallback: number
) {
  if (typeof value !== "number" || Number.isNaN(value)) {
    return fallback;
  }
  return Math.min(max, Math.max(min, Math.round(value)));
}

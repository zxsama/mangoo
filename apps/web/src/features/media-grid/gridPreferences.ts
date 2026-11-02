const LIBRARY_GRID_PREFERENCES_STORAGE_KEY = "mangoo.library.grid-preferences";

// One unified set of grid controls shared by BOTH the media tile grid and the
// folder cover grid (they used to be two parallel sets). Only the folder edge
// shadow is folder-specific.
export interface LibraryGridPreferences {
  // Gap (px) between tiles/covers in the grid.
  tileGap: number;
  // Real CSS line-height (px) for a name line. Names render on up to two lines
  // with this line-height; it controls line spacing ONLY and never moves the
  // name toward or away from the thumbnail (that is labelGap's job).
  labelLineHeight: number;
  // Vertical distance (px) between a thumbnail/cover and its name. Controls the
  // name's position ONLY; independent of line-height.
  labelGap: number;
  // Folder-cover edge shadow strength (%), folder grid only.
  folderEdgeShadowAlpha: number;
}

export const DEFAULT_LIBRARY_GRID_PREFERENCES: LibraryGridPreferences = {
  tileGap: 7,
  labelLineHeight: 15,
  labelGap: 4,
  folderEdgeShadowAlpha: 25
};

export const LIBRARY_GRID_PREFERENCE_LIMITS = {
  tileGap: { min: 4, max: 20, step: 1 },
  labelLineHeight: { min: 12, max: 26, step: 1 },
  labelGap: { min: 0, max: 16, step: 1 },
  folderEdgeShadowAlpha: { min: 0, max: 75, step: 1 }
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
  return {
    tileGap: clampField(value.tileGap, "tileGap"),
    labelLineHeight: clampField(value.labelLineHeight, "labelLineHeight"),
    labelGap: clampField(value.labelGap, "labelGap"),
    folderEdgeShadowAlpha: clampField(value.folderEdgeShadowAlpha, "folderEdgeShadowAlpha")
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

const PREVIEW_PREFERENCES_STORAGE_KEY = "megle.preview.preferences";

export const DEFAULT_PREVIEW_BUFFER_LIMIT_MB = 1200;
export const DEFAULT_THUMBNAIL_CACHE_LIMIT_MB = 5120;

// How Megle generates and persists grid thumbnails.
//   - "renderedOnly" (default): only thumbnails that were actually generated and
//     shown on screen get written to the DB. Off-screen tiles are not generated
//     ahead into the persistent store; background write load is minimal.
//   - "allPersist": Megle generates the WHOLE library's thumbnails in the
//     background and persists them, so every folder is instant later.
//   - "cacheOnly": thumbnails are generated for display and kept in the
//     in-memory object-URL cache for this session only — never written to the
//     SQLite thumb_blobs store, so the library file does not grow.
export type ThumbnailGenerationMode = "renderedOnly" | "allPersist" | "cacheOnly";

export const DEFAULT_THUMBNAIL_GENERATION_MODE: ThumbnailGenerationMode = "renderedOnly";

export interface PreviewPreferences {
  previewBufferLimitMb: number;
  thumbnailCacheLimitMb: number;
  persistentThumbnailCacheAutoRefresh: boolean;
  // How thumbnails are generated and whether they are persisted. See
  // `ThumbnailGenerationMode` and the thumbnail-status settings tab.
  thumbnailGenerationMode: ThumbnailGenerationMode;
}

export const PREVIEW_PREFERENCE_LIMITS = {
  previewBufferLimitMb: { min: 0, max: 8192, step: 100 },
  thumbnailCacheLimitMb: { min: 0, max: 32768, step: 256 }
} as const;

export const DEFAULT_PREVIEW_PREFERENCES: PreviewPreferences = {
  previewBufferLimitMb: DEFAULT_PREVIEW_BUFFER_LIMIT_MB,
  thumbnailCacheLimitMb: DEFAULT_THUMBNAIL_CACHE_LIMIT_MB,
  persistentThumbnailCacheAutoRefresh: false,
  thumbnailGenerationMode: DEFAULT_THUMBNAIL_GENERATION_MODE
};

export function readStoredPreviewPreferences(): PreviewPreferences {
  try {
    const stored = window.localStorage.getItem(PREVIEW_PREFERENCES_STORAGE_KEY);
    if (!stored) {
      return DEFAULT_PREVIEW_PREFERENCES;
    }
    const parsed = JSON.parse(stored) as Partial<PreviewPreferences> | null;
    return normalizePreviewPreferences(parsed ?? {});
  } catch {
    return DEFAULT_PREVIEW_PREFERENCES;
  }
}

export function storePreviewPreferences(value: PreviewPreferences) {
  try {
    window.localStorage.setItem(
      PREVIEW_PREFERENCES_STORAGE_KEY,
      JSON.stringify(normalizePreviewPreferences(value))
    );
  } catch {
    // Ignore storage failures in hardened/browser-restricted environments.
  }
}

export function normalizePreviewPreferences(
  value: Partial<PreviewPreferences> & {
    // Legacy boolean field migrated to `thumbnailGenerationMode`.
    thumbnailAutoGenerateAll?: boolean;
  }
): PreviewPreferences {
  return {
    previewBufferLimitMb: clampPreference(
      value.previewBufferLimitMb,
      PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.min,
      PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.max,
      DEFAULT_PREVIEW_PREFERENCES.previewBufferLimitMb
    ),
    thumbnailCacheLimitMb: clampPreference(
      value.thumbnailCacheLimitMb,
      PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.min,
      PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.max,
      DEFAULT_PREVIEW_PREFERENCES.thumbnailCacheLimitMb
    ),
    persistentThumbnailCacheAutoRefresh: value.persistentThumbnailCacheAutoRefresh === true,
    thumbnailGenerationMode: normalizeThumbnailGenerationMode(value)
  };
}

function normalizeThumbnailGenerationMode(
  value: Partial<PreviewPreferences> & { thumbnailAutoGenerateAll?: boolean }
): ThumbnailGenerationMode {
  if (
    value.thumbnailGenerationMode === "renderedOnly" ||
    value.thumbnailGenerationMode === "allPersist" ||
    value.thumbnailGenerationMode === "cacheOnly"
  ) {
    return value.thumbnailGenerationMode;
  }
  // Migrate the legacy boolean: true meant "generate all in background".
  if (value.thumbnailAutoGenerateAll === true) {
    return "allPersist";
  }
  return DEFAULT_THUMBNAIL_GENERATION_MODE;
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

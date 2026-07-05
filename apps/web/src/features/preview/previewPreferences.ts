const PREVIEW_PREFERENCES_STORAGE_KEY = "megle.preview.preferences";

export const DEFAULT_PREVIEW_BUFFER_LIMIT_MB = 1200;
export const DEFAULT_THUMBNAIL_CACHE_LIMIT_MB = 5120;

export interface PreviewPreferences {
  previewBufferLimitMb: number;
  thumbnailCacheLimitMb: number;
  persistentThumbnailCacheAutoRefresh: boolean;
  // When true, Megle automatically generates ALL thumbnails in the background
  // (whole library) instead of only what enters the viewport. Default false:
  // only thumbnails you actually see are persisted, keeping background write
  // load low. See the thumbnail-status settings tab.
  thumbnailAutoGenerateAll: boolean;
}

export const PREVIEW_PREFERENCE_LIMITS = {
  previewBufferLimitMb: { min: 0, max: 8192, step: 100 },
  thumbnailCacheLimitMb: { min: 0, max: 32768, step: 256 }
} as const;

export const DEFAULT_PREVIEW_PREFERENCES: PreviewPreferences = {
  previewBufferLimitMb: DEFAULT_PREVIEW_BUFFER_LIMIT_MB,
  thumbnailCacheLimitMb: DEFAULT_THUMBNAIL_CACHE_LIMIT_MB,
  persistentThumbnailCacheAutoRefresh: false,
  thumbnailAutoGenerateAll: false
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
  value: Partial<PreviewPreferences>
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
    thumbnailAutoGenerateAll: value.thumbnailAutoGenerateAll === true
  };
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

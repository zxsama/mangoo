import type { ThumbnailGenerationMode } from "@megle/core-client";

import {
  DEFAULT_PREVIEW_CHANNEL,
  DEFAULT_PREVIEW_RENDER_FILTER,
  PREVIEW_CHANNELS,
  PREVIEW_RENDER_FILTERS,
  type PreviewChannel,
  type PreviewRenderFilter
} from "./previewRenderSettings";

const PREVIEW_PREFERENCES_STORAGE_KEY = "megle.preview.preferences";

export const DEFAULT_PREVIEW_BUFFER_LIMIT_MB = 1200;
export const DEFAULT_THUMBNAIL_CACHE_LIMIT_MB = 5120;

// How Megle generates and persists grid thumbnails. The type is the shared
// contract type (`@megle/core-client`) so it stays aligned with the
// priority-scope sync request that carries the mode to Core.
//   - "renderedOnly" (default): only thumbnails that were actually generated and
//     shown on screen get written to the DB. Off-screen tiles are not generated
//     ahead into the persistent store; background write load is minimal.
//   - "allPersist": Megle generates the WHOLE library's thumbnails in the
//     background and persists them, so every folder is instant later.
//   - "cacheOnly": thumbnails are generated for display and kept in the
//     in-memory object-URL cache for this session only — never written to the
//     SQLite thumb_blobs store, so the library file does not grow.
export type { ThumbnailGenerationMode };

export const DEFAULT_THUMBNAIL_GENERATION_MODE: ThumbnailGenerationMode = "renderedOnly";

export interface PreviewPreferences {
  previewBufferLimitMb: number;
  thumbnailCacheLimitMb: number;
  persistentThumbnailCacheAutoRefresh: boolean;
  // How thumbnails are generated and whether they are persisted. See
  // `ThumbnailGenerationMode` and the thumbnail-status settings tab.
  thumbnailGenerationMode: ThumbnailGenerationMode;
  // How the preview resamples when zoomed, and which colour channel it shows.
  // Global and persisted; see previewRenderSettings.ts for what each maps to.
  previewRenderFilter: PreviewRenderFilter;
  previewChannel: PreviewChannel;
  // Whether the preview scrubber stays visible instead of auto-hiding.
  // BandiView calls the equivalent `lockControlBar`.
  previewScrubberPinned: boolean;
}

export const PREVIEW_PREFERENCE_LIMITS = {
  previewBufferLimitMb: { min: 0, max: 8192, step: 100 },
  thumbnailCacheLimitMb: { min: 0, max: 32768, step: 256 }
} as const;

export const DEFAULT_PREVIEW_PREFERENCES: PreviewPreferences = {
  previewBufferLimitMb: DEFAULT_PREVIEW_BUFFER_LIMIT_MB,
  thumbnailCacheLimitMb: DEFAULT_THUMBNAIL_CACHE_LIMIT_MB,
  persistentThumbnailCacheAutoRefresh: false,
  thumbnailGenerationMode: DEFAULT_THUMBNAIL_GENERATION_MODE,
  previewRenderFilter: DEFAULT_PREVIEW_RENDER_FILTER,
  previewChannel: DEFAULT_PREVIEW_CHANNEL,
  previewScrubberPinned: false
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
    thumbnailGenerationMode: normalizeThumbnailGenerationMode(value),
    // Validated against the option lists rather than trusted: an unknown stored
    // string would otherwise reach CSS as `image-rendering: lanczos` or a
    // filter id that does not exist.
    previewRenderFilter: PREVIEW_RENDER_FILTERS.some(
      (option) => option.value === value.previewRenderFilter
    )
      ? (value.previewRenderFilter as PreviewRenderFilter)
      : DEFAULT_PREVIEW_RENDER_FILTER,
    previewChannel: PREVIEW_CHANNELS.some((option) => option.value === value.previewChannel)
      ? (value.previewChannel as PreviewChannel)
      : DEFAULT_PREVIEW_CHANNEL,
    // `=== true` rather than a truthiness check: a stored string like "yes"
    // must normalise to false, not sneak through as truthy.
    previewScrubberPinned: value.previewScrubberPinned === true
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

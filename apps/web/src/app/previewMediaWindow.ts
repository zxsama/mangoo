export interface PreviewMediaIdentity {
  id: number;
}

export interface PreviewMediaWindow<T extends PreviewMediaIdentity> {
  loadedMedia: T[];
  selectedAbsoluteIndex: number;
  selectedLoadedIndex: number;
}

export interface PendingPreviewSelection {
  index: number;
  mediaGeneration: number;
}

export type PendingPreviewSelectionResolution<T> =
  | { status: "idle" }
  | { status: "wait" }
  | { status: "discard" }
  | { status: "apply"; media: T };

const PREVIEW_LANDING_SPAN = 24;

/** Preserve absolute slot indexes while exposing the loaded rows in slot order. */
export function resolvePreviewMediaWindow<T extends PreviewMediaIdentity>(
  mediaSlots: Map<number, T>,
  selectedMediaId: number | null
): PreviewMediaWindow<T> {
  const entries = Array.from(mediaSlots.entries()).sort(
    ([leftIndex], [rightIndex]) => leftIndex - rightIndex
  );
  const selectedLoadedIndex = entries.findIndex(
    ([, item]) => item.id === selectedMediaId
  );

  return {
    loadedMedia: entries.map(([, item]) => item),
    selectedAbsoluteIndex:
      selectedLoadedIndex >= 0 ? entries[selectedLoadedIndex]![0] : -1,
    selectedLoadedIndex
  };
}

export function resolvePendingPreviewSelection<T extends PreviewMediaIdentity>({
  pending,
  previewOpen,
  mediaGeneration,
  mediaSlots
}: {
  pending: PendingPreviewSelection | null;
  previewOpen: boolean;
  mediaGeneration: number;
  mediaSlots: ReadonlyMap<number, T>;
}): PendingPreviewSelectionResolution<T> {
  if (pending === null) return { status: "idle" };
  if (!previewOpen || pending.mediaGeneration !== mediaGeneration) {
    return { status: "discard" };
  }
  const media = mediaSlots.get(pending.index);
  return media ? { status: "apply", media } : { status: "wait" };
}

/** Request a neighbourhood so a preview landing can continue in either direction. */
export function previewLandingWindow(
  index: number,
  itemCount: number
): [number, number] {
  const start = Math.max(0, index - PREVIEW_LANDING_SPAN);
  const end =
    itemCount > 0
      ? Math.min(itemCount, index + PREVIEW_LANDING_SPAN + 1)
      : index + 1;
  return [start, Math.max(end, index + 1)];
}

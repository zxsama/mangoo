export interface PreviewTitlePosition {
  current: number;
  total: number;
}

interface MediaPositionItem {
  id: number;
}

export function formatPreviewTitle(
  selectedName: string,
  position: PreviewTitlePosition | null | undefined
): string {
  if (!position || !isValidPosition(position.current) || !isValidPosition(position.total)) {
    return selectedName;
  }
  return `${selectedName} [${position.current}/${Math.max(position.current, position.total)}]`;
}

export function resolvePreviewTitlePosition<T extends MediaPositionItem>({
  media,
  mediaSlots,
  mediaTotalCount,
  selectedMediaId
}: {
  media: readonly T[];
  mediaSlots: ReadonlyMap<number, T>;
  mediaTotalCount: number;
  selectedMediaId: number | null;
}): PreviewTitlePosition | null {
  if (selectedMediaId === null) {
    return null;
  }

  for (const [slotIndex, item] of mediaSlots) {
    if (item.id === selectedMediaId && Number.isFinite(slotIndex) && slotIndex >= 0) {
      const current = Math.trunc(slotIndex) + 1;
      return {
        current,
        total: resolvePreviewTotal(mediaTotalCount, mediaSlots.size, media.length, current)
      };
    }
  }

  const loadedIndex = media.findIndex((item) => item.id === selectedMediaId);
  if (loadedIndex < 0) {
    return null;
  }

  const current = loadedIndex + 1;
  return {
    current,
    total: resolvePreviewTotal(mediaTotalCount, mediaSlots.size, media.length, current)
  };
}

function resolvePreviewTotal(
  mediaTotalCount: number,
  slotCount: number,
  loadedCount: number,
  current: number
): number {
  return Math.max(
    normalizeCount(mediaTotalCount),
    normalizeCount(slotCount),
    normalizeCount(loadedCount),
    current
  );
}

function normalizeCount(value: number): number {
  return Number.isFinite(value) && value > 0 ? Math.trunc(value) : 0;
}

function isValidPosition(value: number): boolean {
  return Number.isFinite(value) && value >= 1;
}

import type { FolderRecord } from "@mangoo/core-client";

export function replaceFolderAppearance(records: FolderRecord[], saved: FolderRecord): FolderRecord[] {
  return records.map((record) => record.id === saved.id
    ? { ...record, icon: saved.icon, color: saved.color }
    : record);
}

export function replaceFolderAppearanceInMap(folders: Record<number, FolderRecord[]>, saved: FolderRecord): Record<number, FolderRecord[]> {
  return Object.fromEntries(Object.entries(folders).map(([parent, records]) => [parent, replaceFolderAppearance(records, saved)]));
}

/** A read begun before a save must not repaint the just-confirmed appearance. */
export function reconcileFolderAppearanceRead(records: FolderRecord[], startedVersion: number, saved: Map<number, { version: number; folder: FolderRecord }>): FolderRecord[] {
  return records.map((record) => {
    const newer = saved.get(record.id);
    return newer && newer.version > startedVersion
      ? { ...record, icon: newer.folder.icon, color: newer.folder.color }
      : record;
  });
}

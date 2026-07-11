import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

export interface LibraryEntry {
  id: string;
  name: string;
  dbPath: string;
  lastOpenedAt: number;
  // Optional visual identity: a preset Lucide icon name + a color, shown left of
  // the library name in the sidebar/switcher. Absent → a default icon/color.
  icon?: string;
  color?: string;
}

export interface LibraryRegistry {
  activeLibraryId: string | null;
  libraries: LibraryEntry[];
}

const EMPTY: LibraryRegistry = { activeLibraryId: null, libraries: [] };

export function registryFilePath(userDataDir: string): string {
  return path.join(userDataDir, "libraries.json");
}

export function loadRegistry(userDataDir: string): LibraryRegistry {
  try {
    const raw = fs.readFileSync(registryFilePath(userDataDir), "utf8");
    const parsed = JSON.parse(raw) as Partial<LibraryRegistry>;
    const libraries = Array.isArray(parsed.libraries)
      ? parsed.libraries.filter(
          (l): l is LibraryEntry =>
            !!l && typeof l.id === "string" && typeof l.name === "string" && typeof l.dbPath === "string"
        )
      : [];
    const activeLibraryId =
      typeof parsed.activeLibraryId === "string" &&
      libraries.some((l) => l.id === parsed.activeLibraryId)
        ? parsed.activeLibraryId
        : null;
    return { activeLibraryId, libraries };
  } catch {
    return { ...EMPTY };
  }
}

export function saveRegistry(userDataDir: string, registry: LibraryRegistry): void {
  fs.writeFileSync(registryFilePath(userDataDir), JSON.stringify(registry), "utf8");
}

export function addLibrary(
  registry: LibraryRegistry,
  input: { name: string; dbPath: string; now: number }
): { registry: LibraryRegistry; entry: LibraryEntry } {
  const existing = registry.libraries.find((l) => l.dbPath === input.dbPath);
  if (existing) {
    return { registry, entry: existing };
  }
  const entry: LibraryEntry = {
    id: `lib_${crypto.randomBytes(4).toString("hex")}`,
    name: input.name,
    dbPath: input.dbPath,
    lastOpenedAt: input.now
  };
  return {
    registry: { ...registry, libraries: [...registry.libraries, entry] },
    entry
  };
}

export function removeLibrary(registry: LibraryRegistry, id: string): LibraryRegistry {
  return {
    activeLibraryId: registry.activeLibraryId === id ? null : registry.activeLibraryId,
    libraries: registry.libraries.filter((l) => l.id !== id)
  };
}

export function setActive(registry: LibraryRegistry, id: string, now: number): LibraryRegistry {
  return {
    activeLibraryId: id,
    libraries: registry.libraries.map((l) => (l.id === id ? { ...l, lastOpenedAt: now } : l))
  };
}

export function renameLibrary(registry: LibraryRegistry, id: string, name: string): LibraryRegistry {
  return {
    ...registry,
    libraries: registry.libraries.map((l) => (l.id === id ? { ...l, name } : l))
  };
}

export function setLibraryIcon(
  registry: LibraryRegistry,
  id: string,
  icon: string,
  color: string
): LibraryRegistry {
  return {
    ...registry,
    libraries: registry.libraries.map((l) => (l.id === id ? { ...l, icon, color } : l))
  };
}

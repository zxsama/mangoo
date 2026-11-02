import type { LibraryContentRootInput } from "@mangoo/core-client";
import type { LibraryCreationPaths } from "@mangoo/notification-contract";

export interface ContentFolderDraft extends LibraryContentRootInput {
  id: number;
  source: "default" | "existing" | "manual";
}
export interface LibraryPathDraft {
  databasePath: string;
  databaseSource: "automatic" | "manual";
  roots: ContentFolderDraft[];
}
export function initialLibraryPathDraft(): LibraryPathDraft {
  return { databasePath: "", databaseSource: "automatic", roots: [{ id: 0, path: "", createIfMissing: true, source: "default" }] };
}
export function applyLibraryCreationPaths(draft: LibraryPathDraft, paths: LibraryCreationPaths): LibraryPathDraft {
  return {
    ...draft,
    databasePath: draft.databaseSource === "automatic" ? paths.databasePath : draft.databasePath,
    roots: draft.roots.map((root) => root.source === "default" ? { ...root, path: paths.defaultContentPath } : root)
  };
}
export function libraryContentInputs(draft: LibraryPathDraft): LibraryContentRootInput[] {
  return draft.roots.map(({ path, createIfMissing }) => ({ path, createIfMissing }));
}

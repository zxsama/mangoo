import { useCallback, useState } from "react";
import type { FolderRecord, MediaRecord } from "@mangoo/core-client";
import type { FileOpResult, LibraryState } from "../../core/useLibraryData";

export type FileOpsTarget =
  | { kind: "file"; file: MediaRecord }
  | { kind: "folder"; folder: FolderRecord }
  | { kind: "files"; fileIds: number[]; sampleName?: string }
  | { kind: "mixed"; fileIds: number[]; folderIds: number[] };

export interface FileOpsController {
  rename: { target: FileOpsTarget | null; busy: boolean; serverError: string | null };
  move: {
    target: FileOpsTarget | null;
    busy: boolean;
    serverError: string | null;
    serverErrorCode: string | null;
  };
  remove: {
    target: FileOpsTarget | null;
    permanent: boolean;
    busy: boolean;
    serverError: string | null;
  };
  openRename: (target: FileOpsTarget) => void;
  openMove: (target: FileOpsTarget) => void;
  openDelete: (target: FileOpsTarget, permanent: boolean) => void;
  closeAll: () => void;
  submitRename: (newName: string) => Promise<void>;
  submitMove: (targetFolderId: number) => Promise<void>;
  submitDelete: () => Promise<void>;
}

interface DialogState<T> {
  target: T | null;
  busy: boolean;
  serverError: string | null;
  serverErrorCode?: string | null;
}

export function useFileOpsController(library: LibraryState): FileOpsController {
  const [rename, setRename] = useState<DialogState<FileOpsTarget>>({
    target: null,
    busy: false,
    serverError: null
  });
  const [move, setMove] = useState<DialogState<FileOpsTarget>>({
    target: null,
    busy: false,
    serverError: null,
    serverErrorCode: null
  });
  const [remove, setRemove] = useState<{
    target: FileOpsTarget | null;
    permanent: boolean;
    busy: boolean;
    serverError: string | null;
  }>({
    target: null,
    permanent: false,
    busy: false,
    serverError: null
  });

  const openRename = useCallback((target: FileOpsTarget) => {
    setRename({ target, busy: false, serverError: null });
  }, []);

  const openMove = useCallback((target: FileOpsTarget) => {
    setMove({ target, busy: false, serverError: null, serverErrorCode: null });
  }, []);

  const openDelete = useCallback((target: FileOpsTarget, permanent: boolean) => {
    setRemove({ target, permanent, busy: false, serverError: null });
  }, []);

  const closeAll = useCallback(() => {
    setRename({ target: null, busy: false, serverError: null });
    setMove({ target: null, busy: false, serverError: null, serverErrorCode: null });
    setRemove({ target: null, permanent: false, busy: false, serverError: null });
  }, []);

  const submitRename = useCallback(
    async (newName: string) => {
      const target = rename.target;
      if (!target) return;
      setRename((current) => ({ ...current, busy: true, serverError: null }));
      let result: FileOpResult;
      if (target.kind === "file") {
        result = await library.renameFile(target.file.id, newName);
      } else if (target.kind === "folder") {
        result = await library.renameFolder(target.folder.id, newName);
      } else {
        setRename({ target: null, busy: false, serverError: null });
        return;
      }
      if (result.ok) {
        setRename({ target: null, busy: false, serverError: null });
      } else {
        setRename((current) => ({
          ...current,
          busy: false,
          serverError: result.message ?? "Rename failed"
        }));
      }
    },
    [library, rename.target]
  );

  const submitMove = useCallback(
    async (targetFolderId: number) => {
      const target = move.target;
      if (!target) return;
      setMove((current) => ({
        ...current,
        busy: true,
        serverError: null,
        serverErrorCode: null
      }));
      const fileIds = collectFileIds(target);
      const folderIds = collectFolderIds(target);
      const result = await library.moveItems({
        fileIds,
        folderIds,
        targetFolderId
      });
      if (result.ok) {
        setMove({ target: null, busy: false, serverError: null, serverErrorCode: null });
      } else {
        setMove((current) => ({
          ...current,
          busy: false,
          serverError: result.message ?? "Move failed",
          serverErrorCode: result.code ?? null
        }));
      }
    },
    [library, move.target]
  );

  const submitDelete = useCallback(async () => {
    const target = remove.target;
    if (!target) return;
    setRemove((current) => ({ ...current, busy: true, serverError: null }));
    const fileIds = collectFileIds(target);
    const folderIds = collectFolderIds(target);
    const result = await library.deleteItems({
      fileIds,
      folderIds,
      permanent: remove.permanent
    });
    if (result.ok) {
      setRemove({ target: null, permanent: false, busy: false, serverError: null });
    } else {
      setRemove((current) => ({
        ...current,
        busy: false,
        serverError: result.message ?? "Delete failed"
      }));
    }
  }, [library, remove.permanent, remove.target]);

  return {
    rename: { target: rename.target, busy: rename.busy, serverError: rename.serverError },
    move: {
      target: move.target,
      busy: move.busy,
      serverError: move.serverError,
      serverErrorCode: move.serverErrorCode ?? null
    },
    remove,
    openRename,
    openMove,
    openDelete,
    closeAll,
    submitRename,
    submitMove,
    submitDelete
  };
}

export function collectFileIds(target: FileOpsTarget): number[] {
  switch (target.kind) {
    case "file":
      return [target.file.id];
    case "folder":
      return [];
    case "files":
      return [...target.fileIds];
    case "mixed":
      return [...target.fileIds];
  }
}

export function collectFolderIds(target: FileOpsTarget): number[] {
  switch (target.kind) {
    case "file":
      return [];
    case "folder":
      return [target.folder.id];
    case "files":
      return [];
    case "mixed":
      return [...target.folderIds];
  }
}

export function targetCounts(target: FileOpsTarget): { files: number; folders: number } {
  return {
    files: collectFileIds(target).length,
    folders: collectFolderIds(target).length
  };
}

export function targetSampleName(target: FileOpsTarget): string {
  switch (target.kind) {
    case "file":
      return target.file.name;
    case "folder":
      return target.folder.name;
    case "files":
      return target.sampleName ?? "";
    case "mixed":
      return "";
  }
}

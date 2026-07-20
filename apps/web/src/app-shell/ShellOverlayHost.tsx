import { ContextMenu, type ContextMenuItem } from "../features/file-ops/ContextMenu";
import { DeleteConfirm } from "../features/file-ops/DeleteConfirm";
import { MoveDialog } from "../features/file-ops/MoveDialog";
import { RenameDialog } from "../features/file-ops/RenameDialog";
import {
  collectFileIds,
  collectFolderIds,
  targetCounts,
  targetSampleName,
  type FileOpsController
} from "../features/file-ops/useFileOps";
import { NotificationPopover } from "../features/notifications/NotificationPopover";
import type { CompactNotificationModel } from "../features/notifications/notificationModel";
import type { LibraryState } from "../core/useLibraryData";

export interface ShellContextMenuState {
  x: number;
  y: number;
  items: ContextMenuItem[];
}

interface ShellOverlayHostProps {
  contextMenu: ShellContextMenuState | null;
  fileOps: FileOpsController;
  library: LibraryState;
  notificationModel: CompactNotificationModel;
  notificationsOpen: boolean;
  onCloseContextMenu: () => void;
  onOpenNotificationCenter: () => void;
}

export function ShellOverlayHost({
  contextMenu,
  fileOps,
  library,
  notificationModel,
  notificationsOpen,
  onCloseContextMenu,
  onOpenNotificationCenter
}: ShellOverlayHostProps) {
  const renameTarget = fileOps.rename.target;
  const moveTarget = fileOps.move.target;
  const removeTarget = fileOps.remove.target;

  return (
    <>
      {notificationsOpen ? (
        <NotificationPopover
          model={notificationModel}
          onOpenCenter={onOpenNotificationCenter}
        />
      ) : null}

      {contextMenu ? (
        <ContextMenu
          items={contextMenu.items}
          onClose={onCloseContextMenu}
          x={contextMenu.x}
          y={contextMenu.y}
        />
      ) : null}

      <RenameDialog
        busy={fileOps.rename.busy}
        currentName={renameTarget ? targetSampleName(renameTarget) : ""}
        kind={renameTarget?.kind === "folder" ? "folder" : "file"}
        onCancel={fileOps.closeAll}
        onSubmit={(newName) => void fileOps.submitRename(newName)}
        open={renameTarget !== null}
        serverError={fileOps.rename.serverError}
      />

      <MoveDialog
        busy={fileOps.move.busy}
        fileIds={moveTarget ? collectFileIds(moveTarget) : []}
        folderIds={moveTarget ? collectFolderIds(moveTarget) : []}
        library={library}
        onCancel={fileOps.closeAll}
        onSubmit={(folderId) => void fileOps.submitMove(folderId)}
        open={moveTarget !== null}
        serverError={fileOps.move.serverError}
        serverErrorCode={fileOps.move.serverErrorCode}
      />

      <DeleteConfirm
        busy={fileOps.remove.busy}
        fileCount={removeTarget ? targetCounts(removeTarget).files : 0}
        folderCount={removeTarget ? targetCounts(removeTarget).folders : 0}
        onCancel={fileOps.closeAll}
        onConfirm={() => void fileOps.submitDelete()}
        open={removeTarget !== null}
        permanent={fileOps.remove.permanent}
        serverError={fileOps.remove.serverError}
      />
    </>
  );
}

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
import type { NotificationWindowIntent } from "@megle/notification-contract";
import type { LibraryState } from "../core/useLibraryData";
import type { RefObject } from "react";

export interface ShellContextMenuState {
  x: number;
  y: number;
  items: ContextMenuItem[];
}

interface ShellOverlayHostProps {
  contextMenu: ShellContextMenuState | null;
  contextMenuReturnFocusRef: RefObject<HTMLElement | null>;
  fileOps: FileOpsController;
  library: LibraryState;
  notificationAnchor: HTMLElement | null;
  notificationAutoFocus: boolean;
  notificationModel: CompactNotificationModel;
  notificationsOpen: boolean;
  onDismissContextMenu: () => void;
  onOpenNotificationCenter: (intent?: NotificationWindowIntent) => void;
}

export interface ShellNotificationOverlayProps {
  notificationAnchor: HTMLElement | null;
  notificationAutoFocus: boolean;
  notificationModel: CompactNotificationModel;
  notificationsOpen: boolean;
  onOpenNotificationCenter: (intent?: NotificationWindowIntent) => void;
}

export function ShellNotificationOverlay({
  notificationAnchor,
  notificationAutoFocus,
  notificationModel,
  notificationsOpen,
  onOpenNotificationCenter
}: ShellNotificationOverlayProps) {
  return notificationsOpen && notificationAnchor ? (
    <NotificationPopover
      anchor={notificationAnchor}
      autoFocus={notificationAutoFocus}
      model={notificationModel}
      onOpenCenter={onOpenNotificationCenter}
    />
  ) : null;
}

export function ShellOverlayHost({
  contextMenu,
  contextMenuReturnFocusRef,
  fileOps,
  library,
  notificationAnchor,
  notificationAutoFocus,
  notificationModel,
  notificationsOpen,
  onDismissContextMenu,
  onOpenNotificationCenter
}: ShellOverlayHostProps) {
  const renameTarget = fileOps.rename.target;
  const moveTarget = fileOps.move.target;
  const removeTarget = fileOps.remove.target;

  return (
    <>
      <ShellNotificationOverlay
        notificationAnchor={notificationAnchor}
        notificationAutoFocus={notificationAutoFocus}
        notificationModel={notificationModel}
        notificationsOpen={notificationsOpen}
        onOpenNotificationCenter={onOpenNotificationCenter}
      />

      {contextMenu ? (
        <ContextMenu
          items={contextMenu.items}
          onDismiss={onDismissContextMenu}
          restoreFocusTarget={contextMenuReturnFocusRef.current}
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
        restoreFocusRef={contextMenuReturnFocusRef}
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
        restoreFocusRef={contextMenuReturnFocusRef}
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
        restoreFocusRef={contextMenuReturnFocusRef}
        serverError={fileOps.remove.serverError}
      />
    </>
  );
}

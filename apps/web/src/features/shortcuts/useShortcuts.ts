import { useEffect, type MutableRefObject } from "react";
import type { LibraryState } from "../../core/useLibraryData";
import { useOverlayStack, type OverlayStackApi } from "../../design/overlays";
import type { FileOpsController } from "../file-ops/useFileOps";
import {
  matchShortcut,
  useShortcutBindings
} from "./shortcutBindings";

export interface UseShortcutsOptions {
  library: LibraryState;
  fileOps: FileOpsController;
  previewOpen: boolean;
  onClosePreview: () => void;
  onPreviewNext?: () => void;
  onPreviewPrevious?: () => void;
  onToggleSidebars: () => void;
  fileOperationRestoreFocusRef: MutableRefObject<HTMLElement | null>;
  onDismissContextMenu: () => void;
}

interface BeginFileOperationOverlayOptions {
  fileOperationRestoreFocusRef: MutableRefObject<HTMLElement | null>;
  onDismissContextMenu: () => void;
  openOperation: () => void;
  overlays: Pick<OverlayStackApi, "openRoot">;
  restoreFocusTarget?: HTMLElement | null;
}

export function beginFileOperationOverlay({
  fileOperationRestoreFocusRef,
  onDismissContextMenu,
  openOperation,
  overlays,
  restoreFocusTarget,
}: BeginFileOperationOverlayOptions): void {
  const focusOwner = restoreFocusTarget === undefined
    ? fileOperationRestoreFocusRef.current
    : restoreFocusTarget;
  overlays.openRoot("file-operation");
  onDismissContextMenu();
  fileOperationRestoreFocusRef.current = focusOwner;
  openOperation();
}

/**
 * Wires global keyboard shortcuts. Suppresses when an editable element has
 * focus so the user can type freely in inputs, textareas, and rich-text
 * surfaces without triggering destructive actions.
 *
 * Defaults preserve the original bindings while allowing users to edit them
 * locally from Settings.
 */
export function useShortcuts({
  fileOps,
  library,
  onClosePreview,
  onPreviewNext,
  onPreviewPrevious,
  onToggleSidebars,
  fileOperationRestoreFocusRef,
  onDismissContextMenu,
  previewOpen
}: UseShortcutsOptions): void {
  const { bindings } = useShortcutBindings();
  const overlays = useOverlayStack();

  useEffect(() => {
    function handler(event: KeyboardEvent) {
      // Registered overlay surfaces own their keyboard interaction. In particular,
      // Tab must navigate their controls instead of toggling the Library sidebars.
      if (event.defaultPrevented ||
        (event.target instanceof HTMLElement && overlays.getContainingLayer(event.target)) ||
        (document.activeElement instanceof HTMLElement && overlays.getContainingLayer(document.activeElement))) {
        return;
      }
      if (isShortcutCaptureTarget(event.target)) {
        return;
      }

      if (isEditableTarget(event.target)) {
        // Esc still useful inside inputs to clear selection / close dialogs.
        if (!matchShortcut(event, bindings, "closeOrReturn")) {
          return;
        }
      }

      const dialogOpen =
        fileOps.rename.target !== null ||
        fileOps.move.target !== null ||
        fileOps.remove.target !== null;

      if (matchShortcut(event, bindings, "closeOrReturn")) {
        if (dialogOpen) {
          // Existing dialogs handle their own Esc; bail so we don't double-close.
          return;
        }
        if (previewOpen) {
          event.preventDefault();
          onClosePreview();
          return;
        }
        if (library.selectedMediaId !== null) {
          event.preventDefault();
          library.setSelectedMediaId(null);
        }
        return;
      }

      // Modal dialogs already trap interaction; suppress global shortcuts.
      if (dialogOpen) {
        return;
      }

      if (matchShortcut(event, bindings, "toggleSidebars")) {
        event.preventDefault();
        onToggleSidebars();
        return;
      }

      if (matchShortcut(event, bindings, "focusSearch")) {
        const search = document.querySelector<HTMLInputElement>(
          '[aria-label="Search library"]'
        );
        if (search) {
          event.preventDefault();
          search.focus();
          search.select();
        }
        return;
      }

      const selected = library.selectedMedia;

      if (matchShortcut(event, bindings, "previewPrevious")) {
        if (previewOpen) {
          event.preventDefault();
          onPreviewPrevious?.();
        }
        return;
      }

      if (matchShortcut(event, bindings, "previewNext")) {
        if (previewOpen) {
          event.preventDefault();
          onPreviewNext?.();
        }
        return;
      }

      if (matchShortcut(event, bindings, "zoomIn")) {
        if (previewOpen) {
          event.preventDefault();
          window.dispatchEvent(new CustomEvent("mangoo:preview-zoom", { detail: { direction: "in" } }));
        }
        return;
      }

      if (matchShortcut(event, bindings, "zoomOut")) {
        if (previewOpen) {
          event.preventDefault();
          window.dispatchEvent(new CustomEvent("mangoo:preview-zoom", { detail: { direction: "out" } }));
        }
        return;
      }

      if (matchShortcut(event, bindings, "renameSelected")) {
        if (selected) {
          event.preventDefault();
          beginFileOperationOverlay({
            fileOperationRestoreFocusRef,
            onDismissContextMenu,
            openOperation: () => fileOps.openRename({ kind: "file", file: selected }),
            overlays,
            restoreFocusTarget:
              document.activeElement instanceof HTMLElement ? document.activeElement : null,
          });
        }
        return;
      }

      if (matchShortcut(event, bindings, "permanentDelete")) {
        if (selected) {
          event.preventDefault();
          beginFileOperationOverlay({
            fileOperationRestoreFocusRef,
            onDismissContextMenu,
            openOperation: () => fileOps.openDelete({ kind: "file", file: selected }, true),
            overlays,
            restoreFocusTarget:
              document.activeElement instanceof HTMLElement ? document.activeElement : null,
          });
        }
        return;
      }

      if (matchShortcut(event, bindings, "recycleDelete")) {
        if (selected) {
          event.preventDefault();
          beginFileOperationOverlay({
            fileOperationRestoreFocusRef,
            onDismissContextMenu,
            openOperation: () => fileOps.openDelete({ kind: "file", file: selected }, false),
            overlays,
            restoreFocusTarget:
              document.activeElement instanceof HTMLElement ? document.activeElement : null,
          });
        }
        return;
      }
    }

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [
    bindings,
    fileOperationRestoreFocusRef,
    fileOps,
    library,
    onClosePreview,
    onPreviewNext,
    onPreviewPrevious,
    onDismissContextMenu,
    onToggleSidebars,
    overlays,
    previewOpen
  ]);
}

function isShortcutCaptureTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return Boolean(target.closest("[data-shortcut-capture='true']"));
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.isContentEditable) {
    return true;
  }
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
    return true;
  }
  return false;
}

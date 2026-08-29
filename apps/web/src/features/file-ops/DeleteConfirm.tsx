import { useEffect, useRef, useState, type RefObject } from "react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayField,
  OverlayInlineStatus,
} from "../../design/overlays";

export interface DeleteConfirmProps {
  open: boolean;
  /** True when permanent (skip recycle bin). */
  permanent: boolean;
  fileCount: number;
  folderCount: number;
  busy?: boolean;
  serverError?: string | null;
  onCancel: () => void;
  onConfirm: () => void;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

export function DeleteConfirm({
  open,
  permanent,
  fileCount,
  folderCount,
  busy = false,
  serverError = null,
  onCancel,
  onConfirm,
  restoreFocusRef,
}: DeleteConfirmProps) {
  const total = fileCount + folderCount;
  const [confirmText, setConfirmText] = useState("");
  const confirmInputRef = useRef<HTMLInputElement | null>(null);
  const cancelButtonRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!open) return;
    setConfirmText("");
  }, [open, permanent]);

  const expected = String(total);
  const matches = confirmText.trim() === expected;
  const canConfirm = !busy && total > 0 && (!permanent || matches);
  const description = permanent
    ? `${summarize(fileCount, folderCount)}. This cannot be undone.`
    : summarize(fileCount, folderCount);

  return (
    <ModalSurface
      ariaDescribedBy="delete-dialog-description"
      ariaLabelledBy="delete-dialog-title"
      busy={busy}
      className="file-operation-dialog"
      id="delete-dialog"
      initialFocusRef={permanent ? confirmInputRef : cancelButtonRef}
      onDismiss={onCancel}
      open={open}
      restoreFocusRef={restoreFocusRef}
      role="alertdialog"
      rootId="file-operation"
    >
      <OverlayDialogHeader
        description={description}
        descriptionId="delete-dialog-description"
        title={permanent ? "Delete permanently" : "Move to recycle bin"}
        titleId="delete-dialog-title"
      />
      <OverlayDialogBody>
        {permanent ? (
          <>
            <p className="overlay-dialog-text">
              This permanently removes the selected items from disk. To confirm, type{" "}
              <code className="overlay-dialog-code">{expected}</code> below.
            </p>
            <OverlayField
              autoComplete="off"
              disabled={busy}
              inputRef={confirmInputRef}
              label="Type item count to confirm"
              onChange={(event) => setConfirmText(event.target.value)}
              type="text"
              value={confirmText}
            />
          </>
        ) : (
          <p className="overlay-dialog-text">
            The selected items will be moved to the Windows recycle bin. You can restore them from
            there.
          </p>
        )}

        {serverError ? (
          <OverlayInlineStatus tone="error">{serverError}</OverlayInlineStatus>
        ) : null}

        <OverlayDialogActions>
          <LiquidGlassButton
            className="overlay-dialog-button"
            disabled={busy}
            onClick={onCancel}
            ref={cancelButtonRef}
            type="button"
          >
            Cancel
          </LiquidGlassButton>
          <LiquidGlassButton
            className={`overlay-dialog-button ${
              permanent
                ? "overlay-dialog-button-danger"
                : "overlay-dialog-button-primary"
            }`}
            disabled={!canConfirm}
            onClick={onConfirm}
            tone={permanent ? "danger" : "primary"}
            type="button"
          >
            {busy
              ? permanent
                ? "Deleting…"
                : "Recycling…"
              : permanent
                ? "Delete permanently"
                : "Move to recycle bin"}
          </LiquidGlassButton>
        </OverlayDialogActions>
      </OverlayDialogBody>
    </ModalSurface>
  );
}

function summarize(fileCount: number, folderCount: number): string {
  const parts: string[] = [];
  if (fileCount > 0) parts.push(`${fileCount} file${fileCount === 1 ? "" : "s"}`);
  if (folderCount > 0) parts.push(`${folderCount} folder${folderCount === 1 ? "" : "s"}`);
  return parts.length > 0 ? parts.join(" and ") : "No items";
}

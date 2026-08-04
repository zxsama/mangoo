import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayField,
  OverlayInlineStatus,
} from "../../design/overlays";

export interface RenameDialogProps {
  open: boolean;
  kind: "file" | "folder";
  currentName: string;
  busy?: boolean;
  serverError?: string | null;
  onCancel: () => void;
  onSubmit: (newName: string) => void;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

const RESERVED_WIN_NAMES = new Set([
  "CON",
  "PRN",
  "AUX",
  "NUL",
  ...Array.from({ length: 9 }, (_, i) => `COM${i + 1}`),
  ...Array.from({ length: 9 }, (_, i) => `LPT${i + 1}`),
]);

function validate(name: string): string | null {
  if (name.length === 0) return "Name cannot be empty";
  if (name.length > 255) return "Name must be 255 characters or fewer";
  if (name === "." || name === "..") return "Name cannot be . or ..";
  if (/[\\/]/.test(name)) return "Name cannot contain '/' or '\\\\'";
  const stem = name.includes(".") ? name.slice(0, name.indexOf(".")) : name;
  if (RESERVED_WIN_NAMES.has(stem.toUpperCase())) {
    return `'${stem}' is reserved on Windows`;
  }
  return null;
}

export function RenameDialog({
  open,
  kind,
  currentName,
  busy = false,
  serverError = null,
  onCancel,
  onSubmit,
  restoreFocusRef,
}: RenameDialogProps) {
  const [value, setValue] = useState(currentName);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const initialSelectionPendingRef = useRef(false);

  useEffect(() => {
    if (!open) return;
    setValue(currentName);
  }, [currentName, open]);

  useEffect(() => {
    if (!open) return;
    initialSelectionPendingRef.current = true;
  }, [currentName, kind, open]);

  const selectInitialValue = () => {
    if (!initialSelectionPendingRef.current) return;
    initialSelectionPendingRef.current = false;
    window.requestAnimationFrame(() => {
      const node = inputRef.current;
      if (!node) return;
      if (kind === "file") {
        const dot = currentName.lastIndexOf(".");
        if (dot > 0) node.setSelectionRange(0, dot);
        else node.select();
      } else {
        node.select();
      }
    });
  };

  const clientError = useMemo(() => validate(value.trim()), [value]);
  const trimmed = value.trim();
  const unchanged = trimmed === currentName;
  const canSubmit = !busy && !clientError && !unchanged;
  const displayedError = clientError ?? serverError;

  return (
    <ModalSurface
      ariaLabelledBy="rename-dialog-title"
      busy={busy}
      className="file-operation-dialog"
      id="rename-dialog"
      initialFocusRef={inputRef}
      onDismiss={onCancel}
      open={open}
      restoreFocusRef={restoreFocusRef}
      rootId="file-operation"
    >
      <OverlayDialogHeader
        title={`Rename ${kind}`}
        titleId="rename-dialog-title"
      />
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!canSubmit) return;
          onSubmit(trimmed);
        }}
      >
        <OverlayDialogBody>
          <OverlayField
            aria-describedby={displayedError ? "rename-dialog-status" : undefined}
            autoComplete="off"
            disabled={busy}
            inputRef={inputRef}
            label="New name"
            onChange={(event) => setValue(event.target.value)}
            onFocus={selectInitialValue}
            type="text"
            value={value}
          />
          {displayedError ? (
            <div id="rename-dialog-status">
              <OverlayInlineStatus tone="error">{displayedError}</OverlayInlineStatus>
            </div>
          ) : null}
          <OverlayDialogActions>
            <button
              className="overlay-dialog-button"
              disabled={busy}
              onClick={onCancel}
              type="button"
            >
              Cancel
            </button>
            <LiquidGlassButton
              className="overlay-dialog-button overlay-dialog-button-primary"
              disabled={!canSubmit}
              tone="primary"
              type="submit"
            >
              {busy ? "Renaming…" : "Rename"}
            </LiquidGlassButton>
          </OverlayDialogActions>
        </OverlayDialogBody>
      </form>
    </ModalSurface>
  );
}

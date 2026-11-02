import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { AnchoredOverlay, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayInlineStatus, type OverlayAnchor, type OverlayRootId } from "../../design/overlays";
import { DEFAULT_LIBRARY_COLOR, DEFAULT_LIBRARY_ICON, LibraryGlyph, LibraryIconPicker } from "./libraryGlyph";
import { libraryOperationError } from "./libraryOperationError";

import { DEFAULT_FOLDER_COLOR, DEFAULT_FOLDER_ICON, FolderGlyph } from "./FolderGlyph";

interface LibraryAppearanceEditorProps {
  kind?: "library" | "folder";
  anchor: OverlayAnchor | null;
  color?: string | null;
  icon?: string | null;
  id: string;
  name: string;
  onApply: (icon: string, color: string) => Promise<void> | void;
  onClose: () => void;
  onFailure?: (cause: unknown) => Promise<void> | void;
  open: boolean;
  parentId?: string;
  restoreFocusRef?: RefObject<HTMLElement | null>;
  rootId: OverlayRootId;
}

/** A draft belongs to one opening. Registry refreshes cannot overwrite unsaved choices. */
export function LibraryAppearanceEditor({ anchor, color, icon, id, name, onApply, onClose, onFailure, open, parentId, restoreFocusRef, rootId, kind = "library" }: LibraryAppearanceEditorProps) {
  const defaultIcon = kind === "folder" ? DEFAULT_FOLDER_ICON : DEFAULT_LIBRARY_ICON;
  const defaultColor = kind === "folder" ? DEFAULT_FOLDER_COLOR : DEFAULT_LIBRARY_COLOR;
  const Glyph = kind === "folder" ? FolderGlyph : LibraryGlyph;
  const [draft, setDraft] = useState({ icon: icon ?? defaultIcon, color: color ?? defaultColor });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [colorValid, setColorValid] = useState(true);
  const [pickerRevision, setPickerRevision] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  const generation = useRef(0);
  const pending = useRef(false);
  const nativeColorOpen = useRef(false);
  const initial = useRef({ icon, color });
  initial.current = { icon, color };

  useLayoutEffect(() => {
    generation.current += 1;
    pending.current = false;
    nativeColorOpen.current = false;
    setBusy(false);
    setError(null);
    setColorValid(true);
    setPickerRevision((revision) => revision + 1);
    if (open) setDraft({ icon: initial.current.icon ?? defaultIcon, color: initial.current.color ?? defaultColor });
    return () => { generation.current += 1; };
  }, [id, open, defaultIcon, defaultColor]);

  useEffect(() => {
    if (!open) return;
    const clearNativePicker = () => { nativeColorOpen.current = false; };
    window.addEventListener("focus", clearNativePicker);
    return () => window.removeEventListener("focus", clearNativePicker);
  }, [open]);

  async function apply() {
    if (!open || pending.current || !colorValid) return;
    const owner = generation.current;
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      await onApply(draft.icon, draft.color);
      if (generation.current === owner) onClose();
    } catch (cause) {
      if (generation.current === owner) setError(libraryOperationError(cause));
      // Reporting must not turn a handled save failure into an unhandled promise.
      try { await onFailure?.(cause); } catch { /* The editor retains the original actionable error. */ }
    } finally {
      if (generation.current === owner) { pending.current = false; setBusy(false); }
    }
  }

  return <AnchoredOverlay anchor={anchor} ariaLabel="图标与颜色" autoFocus busy={busy} className="library-appearance-editor" id={id} initialFocusRef={searchRef} isDismissLocked={() => pending.current || nativeColorOpen.current} onDismiss={() => { if (!pending.current) onClose(); }} open={open} parentId={parentId} placement="right-start" preferredWidth={344} minWidth={280} restoreFocusTarget={restoreFocusRef?.current} role="dialog" rootId={rootId} trapFocus>
    <OverlayDialogHeader title="图标与颜色" titleId={`${id}-title`} />
    <form onKeyDownCapture={() => { nativeColorOpen.current = false; }} onPointerDownCapture={(event) => { if (!(event.target instanceof HTMLInputElement && event.target.type === "color")) nativeColorOpen.current = false; }} onSubmit={(event) => { event.preventDefault(); event.stopPropagation(); void apply(); }}>
      <OverlayDialogBody data-overlay-scroll-container>
        <div aria-label="外观预览" className="library-appearance-preview">
          <span className="library-appearance-preview-glyph" style={{ color: draft.color }}><Glyph color={draft.color} icon={draft.icon} size={28} /></span>
          <span className="library-appearance-preview-name">{name.trim() || (kind === "folder" ? "文件夹" : "资源库")}</span>
        </div>
        <LibraryIconPicker kind={kind} color={draft.color} disabled={busy} icon={draft.icon} key={pickerRevision} onColorValidityChange={setColorValid} onNativeColorOpen={() => { nativeColorOpen.current = true; }} searchRef={searchRef} onPick={(nextIcon, nextColor) => { nativeColorOpen.current = false; setDraft({ icon: nextIcon, color: nextColor }); setError(null); }} />
        {error ? <OverlayInlineStatus tone="error">{error}</OverlayInlineStatus> : null}
      </OverlayDialogBody>
      <OverlayDialogActions>
        <LiquidGlassButton className="library-appearance-reset" disabled={busy} onClick={() => { setDraft({ icon: defaultIcon, color: defaultColor }); setColorValid(true); setPickerRevision((revision) => revision + 1); setError(null); }} type="button">恢复默认</LiquidGlassButton>
        <LiquidGlassButton disabled={busy} onClick={onClose} type="button">取消</LiquidGlassButton>
        <LiquidGlassButton className="library-appearance-apply" disabled={busy || !colorValid} type="submit">{busy ? "正在保存…" : "应用"}</LiquidGlassButton>
      </OverlayDialogActions>
    </form>
  </AnchoredOverlay>;
}

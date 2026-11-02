import { useEffect, useRef, useState, type RefObject } from "react";
import { getDesktopBridge, openExistingLibrary, pickNativeFolder } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayField } from "../../design/overlays";
import { libraryOperationError } from "./libraryOperationError";

export function OpenLibraryDialog({ open, onClose, onOpened, onFailure, restoreFocusRef }: {
  open: boolean;
  onClose: () => void;
  onOpened: () => void;
  onFailure: (cause: unknown) => void;
  restoreFocusRef: RefObject<HTMLElement | null>;
}) {
  const [path, setPath] = useState("");
  const [operation, setOperation] = useState<"picking" | "opening" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const generation = useRef(0);
  const pending = useRef(false);
  const busy = operation !== null;
  const canPick = Boolean(getDesktopBridge()?.pickFolder);

  useEffect(() => {
    generation.current += 1;
    pending.current = false;
    setOperation(null);
    setError(null);
    if (open && canPick) void run("picking", true);
    return () => { generation.current += 1; };
  }, [open]);

  useEffect(() => {
    if (!open || busy || !error) return;
    inputRef.current?.focus({ preventScroll: true });
    inputRef.current?.closest(".overlay-field")?.querySelector('[role="alert"]')?.scrollIntoView({ block: "nearest" });
  }, [open, busy, error]);

  async function run(operation: "picking" | "opening", fromLauncher = false) {
    if (!open || pending.current || (operation === "opening" && !path.trim())) return;
    pending.current = true;
    setOperation(operation);
    setError(null);
    const owner = generation.current;
    try {
      let selectedPath = path.trim();
      if (operation === "picking") {
        const picked = await pickNativeFolder();
        if (owner !== generation.current) return;
        if (!picked) { if (fromLauncher) onClose(); return; }
        selectedPath = picked;
        setPath(picked);
        setOperation("opening");
      }
      await openExistingLibrary(selectedPath);
      if (owner !== generation.current) return;
      onOpened();
      onClose();
    } catch (cause) {
      if (owner !== generation.current) return;
      setError(libraryOperationError(cause));
      onFailure(cause);
    } finally {
      if (owner === generation.current) { pending.current = false; setOperation(null); }
    }
  }

  return <ModalSurface ariaLabelledBy="library-open-title" busy={busy} className="library-operation-dialog library-open-dialog" id="library-open-dialog" initialFocusRef={inputRef} onDismiss={() => { if (!pending.current) onClose(); }} open={open} restoreFocusRef={restoreFocusRef} rootId="managed-library">
    <OverlayDialogHeader title="打开其它资源库" titleId="library-open-title" />
    <form onSubmit={(event) => { event.preventDefault(); void run("opening"); }}>
      <OverlayDialogBody data-overlay-scroll-container>
        <OverlayField autoComplete="off" disabled={busy} error={error} inputRef={inputRef} label={canPick ? "资源库数据库目录" : "服务器上的资源库数据库目录"} onChange={(event) => { setPath(event.target.value); setError(null); }} placeholder="已有 Mangoo 资源库的 Database 目录" title={path || undefined} value={path} />
        {canPick ? <LiquidGlassButton className="library-open-picker" disabled={busy} onClick={() => void run("picking")} type="button">选择文件夹…</LiquidGlassButton> : null}
      </OverlayDialogBody>
      <OverlayDialogActions>
        {busy ? <span role="status">{operation === "picking" ? "正在选择…" : "正在打开…"}</span> : null}
        <LiquidGlassButton disabled={busy} onClick={onClose} type="button">取消</LiquidGlassButton>
        <LiquidGlassButton disabled={busy || !path.trim()} tone="primary" type="submit">打开资源库</LiquidGlassButton>
      </OverlayDialogActions>
    </form>
  </ModalSurface>;
}

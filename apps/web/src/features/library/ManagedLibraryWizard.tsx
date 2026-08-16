import { useEffect, useRef, useState } from "react";
import {
  createLibrary,
  pickNewLibraryPath,
  preflightLibrary,
} from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogHeader,
} from "../../design/overlays";
import {
  DEFAULT_LIBRARY_COLOR,
  DEFAULT_LIBRARY_ICON,
  LibraryIconPicker,
} from "./libraryGlyph";

export interface ManagedLibraryWizardProps {
  firstRun?: boolean;
  onClose: () => void;
  onCreated: () => void;
  onFailure: (cause: unknown) => void;
  open: boolean;
}

export function ManagedLibraryWizard({
  firstRun = false,
  onClose,
  onCreated,
  onFailure,
  open,
}: ManagedLibraryWizardProps) {
  const [name, setName] = useState("Library");
  const [icon, setIcon] = useState(DEFAULT_LIBRARY_ICON);
  const [color, setColor] = useState(DEFAULT_LIBRARY_COLOR);
  const [managedRoot, setManagedRoot] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setError(null);
  }, [open]);

  async function chooseRoot() {
    try {
      const selected = await pickNewLibraryPath(name.trim() || "Library");
      if (selected) setManagedRoot(selected);
    } catch (cause) {
      setError(String(cause));
      onFailure(cause);
    }
  }

  async function submit() {
    if (busy) return;
    const nextName = name.trim();
    if (!nextName) {
      setError("请输入资源库名称");
      return;
    }
    if (!managedRoot) {
      setError("请选择资源库根目录");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const checked = await preflightLibrary({ name: nextName, managedRoot });
      if (!checked.ok) throw new Error(`无法使用该目录：${checked.reason}`);
      await createLibrary({
        activate: true,
        color,
        icon,
        initialMode: "local-only",
        managedRoot: checked.normalizedRoot,
        name: nextName,
        preflightToken: checked.preflightToken,
      });
      onCreated();
      onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      onFailure(cause);
    } finally {
      setBusy(false);
    }
  }

  return (
    <ModalSurface
      ariaDescribedBy="managed-library-wizard-description"
      ariaLabelledBy="managed-library-wizard-title"
      busy={busy}
      id="managed-library-wizard"
      initialFocusRef={nameRef}
      onDismiss={() => {
        if (!busy) onClose();
      }}
      open={open}
      role="dialog"
      rootId="library"
    >
      <OverlayDialogHeader
        description={firstRun ? "选择固定根目录，创建此设备上的第一个资源库。" : "选择固定根目录并创建仅本机资源库。"}
        descriptionId="managed-library-wizard-description"
        title={firstRun ? "开始使用 Megle" : "创建资源库"}
        titleId="managed-library-wizard-title"
      />
      <label className="overlay-field">
        <span>名称</span>
        <input ref={nameRef} value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <LibraryIconPicker
        color={color}
        icon={icon}
        onPick={(nextIcon, nextColor) => {
          setIcon(nextIcon);
          setColor(nextColor);
        }}
        presentation="overlay"
      />
      <div className="managed-library-wizard-root">
        <span>{managedRoot ?? "尚未选择目录"}</span>
        <button data-interactive-pointer-target="true" disabled={busy} onClick={() => void chooseRoot()} type="button">
          选择目录…
        </button>
      </div>
      {error ? <div className="settings-inline-error">{error}</div> : null}
      <OverlayDialogActions>
        {!firstRun ? <button disabled={busy} onClick={onClose} type="button">取消</button> : null}
        <LiquidGlassButton disabled={busy} onClick={() => void submit()} type="button">
          {busy ? "正在创建…" : "创建并打开"}
        </LiquidGlassButton>
      </OverlayDialogActions>
    </ModalSurface>
  );
}

import { useEffect, useRef, useState, type RefObject } from "react";
import { canPickNativeFolder, createLibrary, getDesktopBridge, LibraryCreatedActivationError, pickNativeFolder, preflightLibrary } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ArrowRight, ChevronDown, FolderOpen, Loader2, Pencil, FolderPlus, X } from "../../design/icons";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayField, OverlayInlineStatus } from "../../design/overlays";
import { DEFAULT_LIBRARY_COLOR, DEFAULT_LIBRARY_ICON, LibraryGlyph } from "./libraryGlyph";

import { applyLibraryCreationPaths, initialLibraryPathDraft, libraryContentInputs } from "./libraryCreationDraft";
import { LibraryAppearanceEditor } from "./LibraryAppearanceEditor";

const preflightMessages: Record<string, string> = {
  "content-roots-required": "请至少添加一个文件夹。",
  "content-roots-overlap": "文件夹重复或互相包含，请更改或移除这一项。",
  "database-content-overlap": "数据库不能与文件夹重叠，请选择独立的位置。",
  "invalid-name": "名称不可用，请避免特殊字符、保留名称或末尾的空格和句点。",
  "invalid-path": "无法访问此位置，请重新选择一个本机文件夹。",
  "cloud-sync-root": "请选择云同步目录之外的本机文件夹。",
  "reparse-point": "请选择实际文件夹，不要使用链接或重定向位置。",
  "nested-library-root": "此位置与已有资源库重叠，请选择独立的文件夹。",
  "not-empty": "此文件夹不是空的，请新建或选择一个空文件夹。",
  "marker-mismatch": "此文件夹包含其他资源库信息，请选择新的空文件夹。",
  "not-writable": "没有此文件夹的写入权限，请选择其他位置或调整权限。",
  "insufficient-space": "此位置的可用空间不足，请选择其他磁盘或释放空间。",
  "path-changed": "文件夹位置已变化，请重新选择后再试。"
};

type FormError = { field: "name" | "root" | "content" | "form"; rootIndex?: number | null; message: string };

export interface ManagedLibraryWizardProps {
  firstRun?: boolean;
  onClose: () => void;
  onCreated: () => void;
  onFailure: (cause: unknown) => void;
  onOpenExisting?: () => void;
  open: boolean;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

export function ManagedLibraryWizard({ firstRun = false, onClose, onCreated, onFailure, onOpenExisting, open, restoreFocusRef }: ManagedLibraryWizardProps) {
  const [name, setName] = useState("");
  const [paths, setPaths] = useState(initialLibraryPathDraft);
  const [resolving, setResolving] = useState(false);
  const [pathError, setPathError] = useState<string | null>(null);
  const resolveSequence = useRef(0);
  const rootId = useRef(1);
  const contentRef = useRef<HTMLElement>(null);
  const databaseRef = useRef<HTMLInputElement>(null);
  const nativePicker = canPickNativeFolder();
  const resolver = getDesktopBridge()?.resolveLibraryCreationPaths;
  const [databaseExpanded, setDatabaseExpanded] = useState(!resolver);
  const databasePath = paths.databasePath;
  const manualDatabase = paths.databaseSource === "manual" ? paths.databasePath : undefined;
  const [icon, setIcon] = useState(DEFAULT_LIBRARY_ICON);
  const [color, setColor] = useState(DEFAULT_LIBRARY_COLOR);
  const [operation, setOperation] = useState<"picking" | "creating" | null>(null);
  const [error, setError] = useState<FormError | null>(null);
  const [appearanceAnchor, setAppearanceAnchor] = useState<HTMLButtonElement | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const appearanceTriggerRef = useRef<HTMLButtonElement>(null);
  const rootRef = useRef<HTMLElement>(null);
  const formErrorRef = useRef<HTMLDivElement>(null);
  const pending = useRef(false);
  const generation = useRef(0);
  const busy = operation !== null;
  const ready = Boolean(name.trim() && databasePath && !resolving && !pathError && paths.roots.length && paths.roots.every((root) => root.path.trim()));
  const appearanceOpen = open && !busy && appearanceAnchor !== null;
  const status = operation === "picking" ? "正在选择存储位置…"
    : operation === "creating" ? "正在检查位置并创建资源库…"
      : resolving ? "正在更新目录…" : "";
  useEffect(() => {
    generation.current++;
    pending.current = false;
    setOperation(null);
    setError(null);
    setAppearanceAnchor(null);
    setDatabaseExpanded(!resolver);
    return () => { generation.current++; };
  }, [open]);
  useEffect(() => {
    const sequence = ++resolveSequence.current;
    const owner = generation.current;
    if (!open || !resolver || (!name.trim() && manualDatabase === undefined)) {
      setResolving(false);
      return;
    }
    setResolving(true);
    setPathError(null);
    void resolver({ name: name.trim(), ...(manualDatabase === undefined ? {} : { databasePath: manualDatabase }) }).then((result) => {
      if (sequence !== resolveSequence.current || owner !== generation.current) return;
      setPaths((current) => applyLibraryCreationPaths(current, result));
    }, (cause: unknown) => {
      if (sequence !== resolveSequence.current || owner !== generation.current) return;
      setPathError(cause instanceof Error ? cause.message : String(cause));
    }).finally(() => {
      if (sequence === resolveSequence.current && owner === generation.current) setResolving(false);
    });
    return () => { resolveSequence.current++; };
  }, [open, name, manualDatabase, resolver]);

  function changeDatabase(value: string) {
    resolveSequence.current++;
    setPaths((current) => ({ ...current, databasePath: value, databaseSource: "manual" }));
    setPathError(null);
    setError(null);
  }

  useEffect(() => {
    if (open && (error?.field === "root" || pathError)) setDatabaseExpanded(true);
  }, [error, open, pathError]);

  useEffect(() => {
    if (!open || busy || !databaseExpanded || !pathError) return;
    databaseRef.current?.focus({ preventScroll: true });
    rootRef.current?.querySelector('[role="alert"]')?.scrollIntoView({ block: "nearest" });
  }, [busy, databaseExpanded, open, pathError]);

  useEffect(() => {
    if (!open || busy || !error) return;
    if (error.field === "root" && !databaseExpanded) return;
    const field = error.field === "name" ? nameRef.current?.closest(".managed-library-form-section")
      : error.field === "root" ? rootRef.current : error.field === "content" ? contentRef.current?.querySelector(`[data-root-index="${error.rootIndex ?? 0}"]`) ?? contentRef.current : formErrorRef.current;
    // Keep the recovery control and its message reachable inside the body scroller.
    if (error.field === "name") nameRef.current?.focus({ preventScroll: true });
    if (error.field === "root") databaseRef.current?.focus({ preventScroll: true });
    if (error.field === "content") (field?.querySelector("input, button") as HTMLElement | null)?.focus({ preventScroll: true });
    field?.querySelector('[role="alert"]')?.scrollIntoView({ block: "nearest" });
  }, [busy, databaseExpanded, error, open]);

  async function chooseRoot(target: "database" | number | "add" = "database") {
    if (!open || pending.current) return;
    pending.current = true;
    setAppearanceAnchor(null);
    setOperation("picking");
    const owner = generation.current;
    try {
      const selected = await pickNativeFolder();
      if (owner === generation.current && selected) {
        if (target === "database") changeDatabase(selected);
        else {
          const id = rootId.current++;
          setPaths((current) => ({ ...current, roots: target === "add"
            ? [...current.roots, { id, path: selected, createIfMissing: false, source: "existing" }]
            : current.roots.map((root) => root.id === target ? { ...root, path: selected, createIfMissing: false, source: "existing" } : root) }));
          setError(null);
        }
      }
    } catch (cause) {
      if (owner !== generation.current) return;
      setError({ field: target === "database" ? "root" : "content", rootIndex: typeof target === "number" ? paths.roots.findIndex((root) => root.id === target) : null, message: cause instanceof Error ? cause.message : String(cause) });
      onFailure(cause);
    } finally {
      if (owner === generation.current) { pending.current = false; setOperation(null); }
    }
  }

  async function submit() {
    if (!open || pending.current) return;
    if (!name.trim()) { setError({ field: "name", message: "请输入资源库名称" }); return; }
    if (!databasePath || pathError) { setError({ field: "root", message: pathError ?? "请输入数据库目录" }); return; }
    if (resolving) return;
    if (!paths.roots.length || paths.roots.some((root) => !root.path.trim())) { setError({ field: "content", message: "请至少填写一个有效的文件夹" }); return; }
    const snapshot = { name: name.trim(), databasePath, contentRoots: libraryContentInputs(paths), icon, color };
    pending.current = true;
    setAppearanceAnchor(null);
    setOperation("creating");
    setError(null);
    const owner = generation.current;
    try {
      const checked = await preflightLibrary({ name: snapshot.name, databasePath: snapshot.databasePath, contentRoots: snapshot.contentRoots });
      if (owner !== generation.current) return;
      if (!checked.ok) {
        const message = preflightMessages[checked.reason] ?? "无法使用此位置，请重新选择文件夹后再试。";
        setError({ field: checked.reason === "invalid-name" ? "name" : checked.pathRole === "content-root" ? "content" : "root", rootIndex: checked.rootIndex, message });
        onFailure(new Error(message));
        return;
      }
      if (!checked.normalizedDatabasePath || !checked.normalizedContentRoots?.length || !checked.preflightToken) throw new Error("位置检查结果不完整，请重试。");
      await createLibrary({ activate: true, color: snapshot.color, icon: snapshot.icon, initialMode: "local-only", databasePath: checked.normalizedDatabasePath, contentRoots: checked.normalizedContentRoots, name: snapshot.name, preflightToken: checked.preflightToken });
      if (owner !== generation.current) return;
      onCreated();
      onClose();
    } catch (cause) {
      if (owner !== generation.current) return;
      if (cause instanceof LibraryCreatedActivationError) {
        onCreated();
        onClose();
        onFailure(cause);
        return;
      }
      setError({ field: "form", message: cause instanceof Error ? cause.message : String(cause) });
      onFailure(cause);
    } finally {
      if (owner === generation.current) { pending.current = false; setOperation(null); }
    }
  }

  return <ModalSurface ariaLabelledBy="managed-library-wizard-title" busy={busy} className="managed-library-wizard" id="managed-library-wizard" initialFocusRef={nameRef} onDismiss={() => { if (!busy) onClose(); }} open={open} restoreFocusRef={restoreFocusRef} role="dialog" rootId="managed-library">
    <OverlayDialogHeader title={firstRun ? "创建第一个资源库" : "创建资源库"} titleId="managed-library-wizard-title" />
    <form className="managed-library-wizard-form" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
    <OverlayDialogBody className="managed-library-wizard-body" data-overlay-scroll-container>
      <section className="managed-library-form-section">
        <div className="managed-library-identity" data-interactive-pointer-owner="true" data-pointer-glow={busy ? "off" : undefined}>
          <button aria-controls="managed-library-appearance" aria-expanded={appearanceOpen} aria-haspopup="dialog" aria-label="选择图标与颜色" className="managed-library-identity-preview" disabled={busy} onClick={(event) => setAppearanceAnchor(appearanceOpen ? null : event.currentTarget)} ref={appearanceTriggerRef} style={{ color }} title="选择图标与颜色" type="button"><LibraryGlyph color={color} icon={icon} size={26} /><Pencil aria-hidden="true" className="managed-library-identity-edit" size={11} /></button>
          <OverlayField aria-label="资源库名称" aria-invalid={error?.field === "name"} aria-describedby={error?.field === "name" ? "managed-library-name-error" : undefined} autoComplete="off" disabled={busy} inputRef={nameRef} onChange={(event) => { resolveSequence.current++; setName(event.target.value); setError((current) => current?.field === "name" ? null : current); }} placeholder="资源库名称" value={name} />
        </div>
        {error?.field === "name" ? <div className="overlay-field-error" id="managed-library-name-error" role="alert">{error.message}</div> : null}
      </section>
      <section aria-labelledby="managed-library-content-label" className="managed-library-root-field" ref={contentRef}>
        <div className="managed-library-content-heading"><span className="managed-library-field-label" id="managed-library-content-label">{resolver ? "文件夹" : "文件夹（Core 主机）"}</span>
          <button aria-label="添加内容文件夹" className="managed-library-add-folder" disabled={busy} onClick={() => {
            if (nativePicker) void chooseRoot("add");
            else { const id = rootId.current++; setPaths((current) => ({ ...current, roots: [...current.roots, { id, path: "", source: "manual", createIfMissing: false }] })); }
          }} type="button"><FolderPlus aria-hidden="true" size={16} />添加</button>
        </div>
        {paths.roots.map((root, index) => <div className="managed-library-content-entry" data-root-index={index} key={root.id}>
          <div className="managed-library-path-row managed-library-content-row" data-interactive-pointer-owner="true" data-pointer-glow={busy ? "off" : undefined}>
            <FolderOpen aria-hidden="true" className="managed-library-folder-icon" size={22} />
            <div className="managed-library-path-content">
            <span aria-hidden="true" className="managed-library-folder-name">{root.path.split(/[\\/]/).filter(Boolean).at(-1) || "文件夹"}</span>
            <OverlayField aria-invalid={error?.field === "content" && error.rootIndex === index} aria-describedby={error?.field === "content" && error.rootIndex === index ? `managed-library-content-error-${root.id}` : undefined} aria-label={`内容文件夹 ${index + 1}`} autoComplete="off" disabled={busy} onChange={(event) => {
              const value = event.target.value;
              setPaths((current) => ({ ...current, roots: current.roots.map((item) => item.id === root.id ? { ...item, path: value, source: "manual" } : item) })); setError(null);
            }} placeholder={resolver && root.source === "default" ? "填写名称后自动生成" : "输入完整文件夹路径"} title={root.path} value={root.path} />
            </div>
            {nativePicker ? <button aria-label={`更改内容文件夹 ${index + 1}`} className="managed-library-path-action" disabled={busy} onClick={() => void chooseRoot(root.id)} title="选择文件夹" type="button"><Pencil aria-hidden="true" size={16} /></button> : null}
            <button aria-label={`移除内容文件夹 ${index + 1}`} className="managed-library-path-action" disabled={busy} onClick={() => { setPaths((current) => ({ ...current, roots: current.roots.filter((item) => item.id !== root.id) })); setError(null); }} title="移除文件夹" type="button"><X aria-hidden="true" size={16} /></button>
          </div>
          {error?.field === "content" && error.rootIndex === index ? <div id={`managed-library-content-error-${root.id}`}><OverlayInlineStatus tone="error">{error.message}</OverlayInlineStatus></div> : null}
        </div>)}
        {!paths.roots.length ? <OverlayInlineStatus tone="error">请至少添加一个文件夹。</OverlayInlineStatus> : error?.field === "content" && error.rootIndex == null ? <OverlayInlineStatus tone="error">{error.message}</OverlayInlineStatus> : null}
      </section>
      <section aria-labelledby="managed-library-location-label" className="managed-library-database-section" ref={rootRef}>
        <button aria-controls="managed-library-database-editor" aria-expanded={databaseExpanded} className="managed-library-database-toggle" disabled={busy} onClick={() => setDatabaseExpanded((expanded) => !expanded)} title={databasePath || undefined} type="button">
          <span id="managed-library-location-label">{resolver ? "数据库" : "数据库（Core 主机）"}</span>
          {!databaseExpanded ? <span className="managed-library-database-summary" title={databasePath}>{databasePath}</span> : null}
          <ChevronDown aria-hidden="true" className="managed-library-database-chevron" size={14} />
        </button>
        <div className="managed-library-database-editor" hidden={!databaseExpanded} id="managed-library-database-editor">
          <div className="managed-library-path-row managed-library-database-row" data-interactive-pointer-owner="true" data-pointer-glow={busy ? "off" : undefined}>
            <OverlayField aria-invalid={error?.field === "root" || Boolean(pathError)} aria-describedby={error?.field === "root" || pathError ? "managed-library-database-error" : undefined} aria-labelledby="managed-library-location-label" autoComplete="off" disabled={busy} inputRef={databaseRef} onChange={(event) => changeDatabase(event.target.value)} placeholder={resolver ? "填写名称后自动生成" : "输入 Core 主机上的完整路径"} title={databasePath} value={databasePath} />
            {nativePicker ? <button aria-label="选择数据库目录" className="managed-library-path-action" disabled={busy} onClick={() => void chooseRoot()} title="选择数据库" type="button"><FolderOpen aria-hidden="true" size={18} /></button> : null}
          </div>
          {error?.field === "root" || pathError ? <div id="managed-library-database-error"><OverlayInlineStatus tone="error">{error?.field === "root" ? error.message : pathError}</OverlayInlineStatus></div> : null}
        </div>
      </section>
      {error?.field === "form" ? <div ref={formErrorRef}><OverlayInlineStatus tone="error">{error.message}</OverlayInlineStatus></div> : null}
    </OverlayDialogBody>
    <OverlayDialogActions className="managed-library-wizard-actions">
      <span className="managed-library-submit-status" data-quiet={!status} id="managed-library-submit-status" role="status">{status}</span>
      <div className="managed-library-action-buttons">
      {firstRun && onOpenExisting ? <LiquidGlassButton disabled={busy} onClick={onOpenExisting} type="button">打开已有资源库…</LiquidGlassButton> : null}
      {!firstRun ? <LiquidGlassButton disabled={busy} onClick={onClose} type="button">取消</LiquidGlassButton> : null}
      <LiquidGlassButton aria-describedby="managed-library-submit-status" className="managed-library-create" disabled={busy || !ready} type="submit">{operation === "creating" ? <Loader2 aria-hidden="true" className="managed-library-spinner" size={16} /> : null}{operation === "creating" ? "正在创建…" : "创建并打开"}{!busy ? <ArrowRight aria-hidden="true" size={16} /> : null}</LiquidGlassButton>
      </div>
    </OverlayDialogActions>
    </form>
    <LibraryAppearanceEditor anchor={appearanceAnchor} color={color} icon={icon} id="managed-library-appearance" name={name} parentId="managed-library-wizard"
      onApply={(nextIcon, nextColor) => { setIcon(nextIcon); setColor(nextColor); }} onClose={() => setAppearanceAnchor(null)}
      open={appearanceOpen} restoreFocusRef={appearanceTriggerRef} rootId="managed-library" />
  </ModalSurface>;
}

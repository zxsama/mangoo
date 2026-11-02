import { useEffect, useRef, useState } from "react";
import type { DirectorySortRule, DirectorySortState } from "@mangoo/core-client";
import { AnchoredOverlay, OverlayMenuItem, OverlayMenuList } from "../../design/overlays";
import { Check } from "../../design/icons";
import type { DirectorySortTarget, LibraryState } from "../../core/useLibraryData";
import { libraryOperationError } from "./libraryOperationError";

/** The menu keeps failed drafts visible. Core owns inheritance and page order. */
export function DirectorySortMenu({ anchor, target, library, onClose }: {
  anchor: HTMLElement;
  target: DirectorySortTarget;
  library: LibraryState;
  onClose: () => void;
}) {
  const [state, setState] = useState<DirectorySortState | null>(null);
  const [recursive, setRecursive] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const lastRule = useRef<DirectorySortRule | null>(null);
  const lifetime = useRef(0);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const token = ++lifetime.current;
    const controller = new AbortController();
    setLoading(true); setError(null);
    void library.getDirectorySort(target, controller.signal).then((value) => {
      if (token !== lifetime.current) return;
      setState(value); setRecursive(value.explicitRule?.recursive ?? false);
    }).catch((cause: unknown) => {
      if (token === lifetime.current && !controller.signal.aborted) setError(libraryOperationError(cause));
    }).finally(() => { if (token === lifetime.current) setLoading(false); });
    return () => { lifetime.current += 1; controller.abort(); };
  }, [attempt, library.getDirectorySort, target.kind, target.id]);
  async function save(rule: DirectorySortRule | null) {
    if (pending.current || loading) return;
    const token = lifetime.current;
    lastRule.current = rule;
    pending.current = true; setBusy(true); setError(null);
    try {
      const value = await library.updateDirectorySort(target, rule);
      if (token !== lifetime.current) return;
      setState(value); setRecursive(value.explicitRule?.recursive ?? false);
    } catch (cause) {
      if (token === lifetime.current) setError(libraryOperationError(cause));
    } finally {
      pending.current = false;
      if (token === lifetime.current) setBusy(false);
    }
  }
  return <AnchoredOverlay anchor={anchor} ariaLabel="子文件夹排序" id="directory-sort-menu" className="context-menu"
    rootId="context-menu" parentId="context-menu-root" open placement="right-start"
    preferredWidth={220} onDismiss={onClose} busy={busy} isDismissLocked={() => pending.current}
    restoreFocusTarget={anchor} role="group">
    <OverlayMenuList ariaLabel="子文件夹排序" onKeyDown={(event) => {
      if (event.key === "ArrowLeft" && !pending.current) { event.preventDefault(); event.stopPropagation(); onClose(); }
    }}>
      {loading ? <OverlayMenuItem disabled>正在读取排序…</OverlayMenuItem> : null}
      {([['name_asc', '名称升序'], ['name_desc', '名称降序']] as const).map(([order, label]) =>
        <OverlayMenuItem key={order} role="menuitemradio" checked={state?.effectiveOrder === order}
          selected={state?.effectiveOrder === order} icon={state?.effectiveOrder === order ? <Check size={14} /> : <span />}
          disabled={loading || busy || !state} onClick={() => void save({ order, recursive })}>{label}</OverlayMenuItem>)}
      <div className="overlay-menu-separator" role="separator" />
      <OverlayMenuItem role="menuitemcheckbox" checked={recursive} disabled={loading || busy || !state}
        selected={recursive} icon={recursive ? <Check size={14} /> : <span />}
        onClick={() => {
          const next = !recursive; setRecursive(next);
          if (state) void save({ order: state.effectiveOrder, recursive: next });
        }}>递归应用到子文件夹</OverlayMenuItem>
      <OverlayMenuItem disabled={loading || busy || !state?.explicitRule} onClick={() => void save(null)}>恢复继承 / 默认</OverlayMenuItem>
      {state ? <OverlayMenuItem disabled>{state.explicitRule ? '当前目录的设置' : state.source ? '继承上级目录' : '默认：名称升序'}</OverlayMenuItem> : null}
      {busy ? <OverlayMenuItem disabled>正在保存…</OverlayMenuItem> : null}
      {error ? <><div role="alert">{error}</div><OverlayMenuItem disabled={busy} onClick={() => {
        if (!state) setAttempt((value) => value + 1);
        else void save(lastRule.current);
      }}>重试</OverlayMenuItem></> : null}
    </OverlayMenuList>
  </AnchoredOverlay>;
}

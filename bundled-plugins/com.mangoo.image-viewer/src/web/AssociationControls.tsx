import { useCallback, useEffect, useState } from "react";
import { LiquidGlassButton, icons } from "@mangoo/web/plugin-host";

import type { ImageViewerAssociationStatus, ImageViewerExplorerThumbnailStatus } from "../shared/ipc.ts";
import { explorerThumbnailBridge, viewerHostBridge } from "./bridge.ts";

/**
 * "Use Mangoo to open pictures" as the user experiences it on each OS:
 * one switch for whether Mangoo is offered as an image app, plus the next
 * step the OS actually requires (on Windows the final default is a choice
 * the user confirms in Settings, which we open for them).
 */
export function AssociationControls() {
  const bridge = viewerHostBridge();
  const [status, setStatus] = useState<ImageViewerAssociationStatus | null>(null);
  const [busy, setBusy] = useState(false);
  /** The state being applied, shown immediately so the thumb moves at once. */
  const [pendingTarget, setPendingTarget] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!bridge) return;
    try {
      setStatus(await bridge.associationStatus());
    } catch {
      setStatus(null);
    }
  }, [bridge]);

  useEffect(() => {
    void load();
    // Re-read when the user comes back from the OS settings app.
    const onFocus = () => void load();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);

  if (!bridge) {
    return <p className="iv-setting-description">关联系统图片仅在桌面版中可用。</p>;
  }
  if (!status) {
    return <p className="iv-setting-description">正在读取系统设置…</p>;
  }

  async function toggle(next: boolean) {
    if (!bridge) return;
    setPendingTarget(next);
    setBusy(true);
    setError(null);
    try {
      const result = await bridge.setAssociation(next);
      setStatus(result);
      if (result.registered !== next && result.supported) setError(result.detail);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  const actionable = status.supported && status.pluginEnabled;
  const formats = status.extensions.map((ext) => ext.replace(".", "").toUpperCase());
  const unsupportedReason = !status.pluginEnabled
    ? "启用看图插件后即可设置。"
    : !status.supported
      ? status.detail
      : null;

  return (
    <div className="iv-assoc">
      <div className="iv-setting-row">
        <div className="iv-setting-text">
          <span className="iv-setting-title">用 Mangoo 打开图片</span>
          <p className="iv-setting-description">
            {unsupportedReason
              ?? (status.registered
                ? "Mangoo 已出现在图片的“打开方式”中，双击图片即可用独立窗口查看。"
                : "让 Mangoo 出现在图片的“打开方式”中，并可设为默认看图应用。")}
          </p>
        </div>
        <div className="iv-setting-control">
          <span className="iv-assoc-state" data-on={status.registered ? "true" : "false"}>
            {busy ? "正在应用…" : status.registered ? "已开启" : "未开启"}
          </span>
          <label
            aria-busy={busy || undefined}
            className={`plugin-toggle iv-switch${(busy ? pendingTarget : status.registered) ? " on" : ""}${!actionable ? " disabled" : ""}${busy ? " busy" : ""}`}
          >
            <input
              aria-label="用 Mangoo 打开图片"
              checked={busy ? pendingTarget : status.registered}
              disabled={!actionable}
              onChange={(event) => {
                // While applying, further clicks are ignored (not "disabled":
                // that flashed a not-allowed cursor under the pointer).
                if (busy) return;
                void toggle(event.target.checked);
              }}
              role="switch"
              type="checkbox"
            />
            <span aria-hidden="true" className="plugin-toggle-track">
              <span className="plugin-toggle-thumb" />
            </span>
          </label>
        </div>
      </div>

      {status.registered && status.canOpenSystemSettings ? (
        <div className="iv-assoc-next">
          <icons.Info aria-hidden="true" size={14} />
          <div>
            <strong>最后一步：设为默认</strong>
            <p>Windows 需要你亲自确认默认应用。打开“默认应用”后，在 Mangoo 看图的页面里把常用图片格式设为它即可。</p>
          </div>
          <LiquidGlassButton className="iv-assoc-button" onClick={() => void bridge.openDefaultAppsSettings()} tone="primary" type="button">
            <icons.ExternalLink aria-hidden="true" size={13} />
            <span>打开默认应用设置</span>
          </LiquidGlassButton>
        </div>
      ) : null}

      {error ? <p className="iv-assoc-error" role="alert">{error}</p> : null}

      {formats.length ? (
        <details className="iv-assoc-formats">
          <summary data-pointer-glow="off">支持 {formats.length} 种图片格式</summary>
          <p>{formats.join("、")}</p>
        </details>
      ) : null}
    </div>
  );
}

/**
 * "Show thumbnails in File Explorer": one switch. Windows already draws
 * JPEG / PNG / GIF / BMP / TIFF; the switch adds camera RAW, PSD and WebP so
 * that every picture in a folder gets a thumbnail.
 */
export function ExplorerThumbnailControls() {
  const bridge = explorerThumbnailBridge();
  const [status, setStatus] = useState<ImageViewerExplorerThumbnailStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [pendingTarget, setPendingTarget] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!bridge) return;
    try {
      setStatus(await bridge.explorerThumbnailStatus());
    } catch {
      setStatus(null);
    }
  }, [bridge]);

  useEffect(() => {
    void load();
  }, [load]);

  if (!bridge || !status) return null;
  // Not a Windows feature elsewhere: say nothing rather than show a dead switch.
  if (!status.supported && status.detail.startsWith("仅 Windows")) return null;

  async function toggle(next: boolean) {
    if (!bridge) return;
    setPendingTarget(next);
    setBusy(true);
    setError(null);
    try {
      const result = await bridge.setExplorerThumbnails(next);
      setStatus(result);
      if (result.registered !== next && result.supported) setError(result.detail);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  const actionable = status.supported && status.pluginEnabled;
  const on = busy ? pendingTarget : status.registered;
  const formats = status.extensions.map((ext) => ext.replace(".", "").toUpperCase());

  return (
    <div className="iv-assoc">
      <div className="iv-setting-row">
        <div className="iv-setting-text">
          <span className="iv-setting-title">在文件资源管理器中显示缩略图</span>
          <p className="iv-setting-description">{status.detail}</p>
        </div>
        <div className="iv-setting-control">
          <span className="iv-assoc-state" data-on={status.registered ? "true" : "false"}>
            {busy ? "正在应用…" : status.registered ? "已开启" : "未开启"}
          </span>
          <label
            aria-busy={busy || undefined}
            className={`plugin-toggle iv-switch${on ? " on" : ""}${!actionable ? " disabled" : ""}${busy ? " busy" : ""}`}
          >
            <input
              aria-label="在文件资源管理器中显示缩略图"
              checked={on}
              disabled={!actionable}
              onChange={(event) => {
                if (busy) return;
                void toggle(event.target.checked);
              }}
              role="switch"
              type="checkbox"
            />
            <span aria-hidden="true" className="plugin-toggle-track">
              <span className="plugin-toggle-thumb" />
            </span>
          </label>
        </div>
      </div>
      {error ? <p className="iv-assoc-error" role="alert">{error}</p> : null}
      <details className="iv-assoc-formats">
        <summary data-pointer-glow="off">新增 {formats.length} 种格式的缩略图</summary>
        <p>{formats.join("、")}</p>
      </details>
    </div>
  );
}

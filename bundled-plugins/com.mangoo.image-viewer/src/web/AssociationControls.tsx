import { useCallback, useEffect, useState } from "react";
import { LiquidGlassButton, icons } from "@mangoo/web/plugin-host";

import type { ImageViewerAssociationStatus } from "../shared/ipc.ts";
import { viewerHostBridge } from "./bridge.ts";

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
          <label className={`plugin-toggle iv-switch${status.registered ? " on" : ""}${!actionable || busy ? " disabled" : ""}`}>
            <input
              aria-label="用 Mangoo 打开图片"
              checked={status.registered}
              disabled={!actionable || busy}
              onChange={(event) => void toggle(event.target.checked)}
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

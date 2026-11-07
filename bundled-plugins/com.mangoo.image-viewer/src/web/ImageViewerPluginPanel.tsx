import { useCallback, useEffect, useState } from "react";
import { LiquidGlassButton, icons } from "@mangoo/web/plugin-host";

import type { ImageViewerAssociationStatus } from "../shared/ipc.ts";
import { viewerHostBridge } from "./bridge.ts";
import "./viewer.css";

/** Register / unregister Mangoo as the OS image viewer. */
export function AssociationControls({ compact = false, refreshKey }: { compact?: boolean; refreshKey?: unknown }) {
  const bridge = viewerHostBridge();
  const [status, setStatus] = useState<ImageViewerAssociationStatus | null>(null);
  const [busy, setBusy] = useState(false);

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
  }, [load, refreshKey]);

  if (!bridge) {
    return <p className="iv-assoc-detail">系统关联仅在桌面版中可用。</p>;
  }
  if (!status) {
    return <p className="iv-assoc-detail">正在读取系统关联状态…</p>;
  }

  async function toggle() {
    if (!bridge || !status) return;
    setBusy(true);
    try {
      setStatus(await bridge.setAssociation(!status.registered));
    } finally {
      setBusy(false);
    }
  }

  const actionable = status.supported && status.pluginEnabled;
  return (
    <div className="iv-assoc" data-compact={compact ? "true" : "false"}>
      <label className="iv-assoc-row" data-disabled={!actionable || busy ? "true" : "false"}>
        <span className="iv-assoc-label">
          <span>设为系统图片查看器</span>
          <small>{status.registered ? "已注册" : status.wanted && actionable ? "等待注册" : "未注册"}</small>
        </span>
        <span className={`plugin-toggle${status.registered ? " on" : ""}${!actionable || busy ? " disabled" : ""}`}>
          <input
            aria-label="设为系统图片查看器"
            checked={status.registered}
            disabled={!actionable || busy}
            onChange={() => void toggle()}
            type="checkbox"
          />
          <span aria-hidden="true" className="plugin-toggle-track">
            <span className="plugin-toggle-thumb" />
          </span>
        </span>
      </label>
      <p className="iv-assoc-detail">{status.detail}</p>
      {status.registered && status.canOpenSystemSettings ? (
        <LiquidGlassButton className="iv-assoc-button" onClick={() => void bridge.openDefaultAppsSettings()} tone="control" type="button">
          <icons.ExternalLink aria-hidden="true" size={13} />
          <span>打开系统“默认应用”设置</span>
        </LiquidGlassButton>
      ) : null}
      {!compact && status.extensions.length ? (
        <p className="iv-assoc-formats">
          支持格式：{status.extensions.map((ext) => ext.replace(".", "").toUpperCase()).join("、")}
        </p>
      ) : null}
    </div>
  );
}

const SHORTCUTS: Array<[string, string]> = [
  ["← / →、PageUp / PageDown、滚轮（可设置）", "上一张 / 下一张"],
  ["Home / End", "第一张 / 最后一张"],
  ["+ / −、Ctrl + 滚轮", "放大 / 缩小（以光标为中心）"],
  ["0 / 1、双击", "适应窗口 / 实际大小"],
  ["R / Shift+R、H / V", "旋转、水平 / 垂直翻转"],
  ["F11 / F、Esc", "全屏 / 退出"],
  ["S / F5", "幻灯片放映"],
  ["I、T", "信息面板、缩略图条"],
  ["Delete、F2", "删除到回收站、重命名"],
  ["Ctrl+C / Ctrl+Shift+C", "复制图片 / 复制路径"],
  ["Ctrl+O", "打开图片"]
];

/** Rendered in the plugin manager detail pane for com.mangoo.image-viewer. */
export function ImageViewerPluginPanel({ enabled, refreshKey }: { enabled: boolean; refreshKey?: unknown }) {
  return (
    <div className="plugin-detail-section iv-plugin-panel">
      <div className="plugin-detail-section-title">看图</div>
      <p className="iv-plugin-description">
        {enabled
          ? "在系统中双击图片即可用独立的 Mangoo 看图窗口打开，支持文件夹浏览、缩放、旋转、幻灯片、EXIF 信息与直方图。格式支持与 Mangoo 一致。"
          : "启用后，Mangoo 可作为独立的看图应用打开系统中的图片。停用插件会关闭看图窗口并移除系统关联。"}
      </p>
      <AssociationControls refreshKey={`${String(enabled)}:${String(refreshKey ?? "")}`} />
      <details className="iv-shortcuts">
        <summary data-pointer-glow="off">快捷键</summary>
        <dl>
          {SHORTCUTS.map(([keys, action]) => (
            <div className="iv-info-row" key={keys}>
              <dt>{keys}</dt>
              <dd>{action}</dd>
            </div>
          ))}
        </dl>
      </details>
    </div>
  );
}

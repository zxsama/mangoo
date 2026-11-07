import { icons } from "@mangoo/web/plugin-host";

import { AssociationControls } from "./AssociationControls.tsx";
import { useViewerPreferences } from "./preferences.ts";
import { ViewerSettingsForm } from "./ViewerSettings.tsx";
import "./viewer.css";

export { AssociationControls } from "./AssociationControls.tsx";

export const SHORTCUT_GROUPS: Array<{ title: string; items: Array<[string, string]> }> = [
  {
    title: "浏览",
    items: [
      ["← →  /  PageUp PageDown", "上一张 / 下一张"],
      ["Home / End", "第一张 / 最后一张"],
      ["鼠标侧键", "上一张 / 下一张"],
      ["S 或 F5", "开始 / 停止幻灯片"]
    ]
  },
  {
    title: "缩放与旋转",
    items: [
      ["滚轮、Ctrl + 滚轮、双指捏合", "以光标为中心缩放"],
      ["+ / −", "放大 / 缩小"],
      ["0 / 1、双击图片", "适应窗口 / 原始大小"],
      ["R / Shift + R", "向右 / 向左旋转"],
      ["H / V", "水平 / 垂直翻转"]
    ]
  },
  {
    title: "窗口与文件",
    items: [
      ["F11 或 F、Esc", "全屏 / 退出"],
      ["I、T", "显示信息、缩略图条"],
      ["Ctrl + O", "打开图片"],
      ["Ctrl + C、Ctrl + Shift + C", "复制图片、复制路径"],
      ["F2、Delete", "重命名、删除到回收站"]
    ]
  }
];

export function ShortcutList() {
  return (
    <div className="iv-shortcut-groups">
      {SHORTCUT_GROUPS.map((group) => (
        <div className="iv-shortcut-group" key={group.title}>
          <div className="iv-shortcut-group-title">{group.title}</div>
          <dl>
            {group.items.map(([keys, action]) => (
              <div className="iv-shortcut-row" key={keys}>
                <dt>{keys.split("、").map((part) => <kbd key={part}>{part}</kbd>)}</dt>
                <dd>{action}</dd>
              </div>
            ))}
          </dl>
        </div>
      ))}
    </div>
  );
}

/** Rendered in the plugin manager detail pane for com.mangoo.image-viewer. */
export function ImageViewerPluginPanel({ enabled }: { enabled: boolean; refreshKey?: unknown }) {
  const [preferences, setPreferences] = useViewerPreferences();
  if (!enabled) {
    return (
      <div className="plugin-detail-section iv-plugin-panel">
        <div className="iv-plugin-intro">
          <icons.Images aria-hidden="true" size={28} weight="light" />
          <div>
            <strong>独立的看图窗口</strong>
            <p className="iv-plugin-description">
              启用后，可以在资源管理器中双击图片直接用 Mangoo 查看：浏览整个文件夹、缩放旋转、幻灯片、EXIF 信息与直方图，支持的格式与资料库一致。
            </p>
            <p className="iv-plugin-description">停用插件会关闭看图窗口，并撤销 Mangoo 在系统中的图片关联。</p>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="plugin-detail-section iv-plugin-panel">
      <section className="iv-settings-section">
        <h3 className="iv-settings-section-title">
          <icons.Settings aria-hidden="true" size={14} />
          <span>系统</span>
        </h3>
        <div className="iv-settings-rows">
          <AssociationControls />
        </div>
      </section>
      <ViewerSettingsForm onChange={setPreferences} preferences={preferences} showSystem={false} />
      <details className="iv-shortcuts">
        <summary data-pointer-glow="off">
          <icons.Keyboard aria-hidden="true" size={14} />
          <span>键盘快捷键</span>
        </summary>
        <ShortcutList />
      </details>
    </div>
  );
}

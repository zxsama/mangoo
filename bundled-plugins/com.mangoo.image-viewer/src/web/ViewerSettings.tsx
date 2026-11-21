import { icons, SettingRow, SettingsSection, SettingsSegmented, SettingsSwitch } from "@mangoo/web/plugin-host";

import type { ImageSortKey } from "../shared/navigation.ts";
import type { ImageViewerSettingsSection } from "../shared/ipc.ts";
import type { ShortcutOverrides } from "../shared/shortcuts.ts";
import { AssociationControls, ExplorerThumbnailControls } from "./AssociationControls.tsx";
import { ShortcutEditor } from "./ShortcutEditor.tsx";
import { SLIDESHOW_INTERVALS, type ViewerBackground, type ViewerPreferences } from "./preferences.ts";

/**
 * The viewer's settings, laid out as a real settings page: titled sections,
 * one row per decision with a short explanation, and the control that fits
 * the decision (switch for on/off, segmented control for a small choice).
 * Rendered in the viewer's settings window AND in Plugin Manager; both read
 * the same localStorage preferences, so a change in one shows in the other.
 */

/** Section labels, in display order (the settings window's left column). */
export const SETTINGS_SECTION_LABELS: Record<ImageViewerSettingsSection, string> = {
  display: "显示",
  browse: "浏览",
  window: "窗口",
  actions: "操作",
  shortcuts: "快捷键",
  system: "系统"
};

const BACKGROUNDS: Array<{ value: ViewerBackground; label: string }> = [
  { value: "interface", label: "跟随界面" },
  { value: "dark", label: "深灰" },
  { value: "black", label: "黑色" },
  { value: "light", label: "浅色" },
  { value: "checker", label: "棋盘格" }
];

const SORT_KEYS: Array<{ value: ImageSortKey; label: string }> = [
  { value: "name", label: "名称" },
  { value: "modified", label: "修改时间" },
  { value: "size", label: "大小" },
  { value: "type", label: "类型" }
];

/**
 * Background picker: one preview card per choice, showing a small picture on
 * that background, so the choice is made by looking rather than by name.
 */
function BackgroundPicker({ value, onChange }: { value: ViewerBackground; onChange(next: ViewerBackground): void }) {
  return (
    <div aria-label="背景" className="iv-bg-picker" role="radiogroup">
      {BACKGROUNDS.map((option) => {
        const selected = value === option.value;
        return (
          <button
            aria-checked={selected}
            className="iv-bg-card"
            data-background={option.value}
            data-selected={selected ? "true" : "false"}
            key={option.value}
            onClick={() => onChange(option.value)}
            role="radio"
            type="button"
          >
            <span aria-hidden="true" className="iv-bg-preview">
              <span className="iv-bg-photo" />
              {selected ? (
                <span className="iv-bg-check">
                  <icons.Check data-button-content size={11} weight="fill" />
                </span>
              ) : null}
            </span>
            <span data-button-content className="iv-bg-label">{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}

export function ViewerSettingsForm({ preferences, onChange, showSystem = true, showShortcuts = false }: {
  preferences: ViewerPreferences;
  onChange(change: Partial<ViewerPreferences>): void;
  /** The plugin manager shows the system section separately. */
  showSystem?: boolean;
  /** The settings window lists the editable shortcuts in the same column. */
  showShortcuts?: boolean;
}) {
  return (
    <div className="iv-settings-form settings-layout-form">
      <SettingsSection anchorId="iv-settings-display" icon={<icons.Image aria-hidden="true" size={14} />} id="display" title="显示">
        <SettingRow description="图片后面的底色。查看带透明区域的图片时，用棋盘格更容易分辨透明部分。" stacked title="背景">
          <BackgroundPicker onChange={(background) => onChange({ background })} value={preferences.background} />
        </SettingRow>
        <SettingRow description="打开或切换图片时的初始大小。" title="默认缩放">
          <SettingsSegmented
            label="默认缩放"
            onChange={(fitMode) => onChange({ fitMode })}
            options={[
              { value: "fit", label: "适应窗口", title: "完整显示整张图片" },
              { value: "fill", label: "填满窗口", title: "铺满窗口，可能裁掉边缘" },
              { value: "actual", label: "原始大小", title: "按 100% 显示" }
            ]}
            value={preferences.fitMode}
          />
        </SettingRow>
        <SettingRow description="关闭时，比窗口小的图片保持原始大小，不会被放大到模糊。" title="放大小图片以适应窗口">
          <SettingsSwitch checked={preferences.upscaleSmall} label="放大小图片以适应窗口" onChange={(upscaleSmall) => onChange({ upscaleSmall })} />
        </SettingRow>
        <SettingRow description="图片显示得比原始像素更大时如何补出中间的像素。“像素”清晰显示每个像素，适合检查细节和像素画。" title="放大时的插值">
          <SettingsSegmented
            label="放大时的插值"
            onChange={(upscaleInterpolation) => onChange({ upscaleInterpolation })}
            options={[
              { value: "smooth", label: "平滑", title: "双线性插值，过渡柔和" },
              { value: "nearest", label: "像素", title: "最近邻，保留每个像素的硬边" }
            ]}
            value={preferences.upscaleInterpolation}
          />
        </SettingRow>
        <SettingRow description="图片显示得比原始像素更小时如何合并像素。“平滑”画质更好，“像素”保留锐利的细线但可能出现锯齿。" title="缩小时的插值">
          <SettingsSegmented
            label="缩小时的插值"
            onChange={(downscaleInterpolation) => onChange({ downscaleInterpolation })}
            options={[
              { value: "smooth", label: "平滑", title: "高质量缩小，减少锯齿和摩尔纹" },
              { value: "nearest", label: "像素", title: "最近邻，不做模糊" }
            ]}
            value={preferences.downscaleInterpolation}
          />
        </SettingRow>
        <SettingRow description={<>按 Alt+1/2/3/4 只看红、绿、蓝或透明通道，Alt+0 恢复。开启后红绿蓝通道先乘以透明度，半透明区域会按透明度变暗。完全透明的像素不保存颜色，两种方式下都显示为黑色。</>} title="单通道显示预乘 Alpha">
          <SettingsSwitch checked={preferences.channelPremultiply} label="单通道显示预乘 Alpha" onChange={(channelPremultiply) => onChange({ channelPremultiply })} />
        </SettingRow>
      </SettingsSection>

      <SettingsSection anchorId="iv-settings-browse" icon={<icons.Images aria-hidden="true" size={14} />} id="browse" title="浏览">
        <SettingRow description="按住 Ctrl 滚动总是缩放。图片放大后，滚轮会先平移图片。" title="鼠标滚轮">
          <SettingsSegmented
            label="鼠标滚轮"
            onChange={(wheelAction) => onChange({ wheelAction })}
            options={[
              { value: "navigate", label: "切换图片" },
              { value: "zoom", label: "缩放图片" }
            ]}
            value={preferences.wheelAction}
          />
        </SettingRow>
        <SettingRow description="文件夹里图片的排列顺序，缩略图条和上一张/下一张都按这个顺序。" stacked title="排序方式">
          <span className="iv-setting-inline">
            <SettingsSegmented label="排序方式" onChange={(sortKey) => onChange({ sortKey })} options={SORT_KEYS} value={preferences.sortKey} />
            <button
              aria-label={preferences.sortDirection === "asc" ? "升序" : "降序"}
              className="iv-direction-button"
              data-pointer-glow="off"
              onClick={() => onChange({ sortDirection: preferences.sortDirection === "asc" ? "desc" : "asc" })}
              title={preferences.sortDirection === "asc" ? "升序（点击切换为降序）" : "降序（点击切换为升序）"}
              type="button"
            >
              <icons.ArrowDownUp aria-hidden="true" size={13} />
              <span>{preferences.sortDirection === "asc" ? "升序" : "降序"}</span>
            </button>
          </span>
        </SettingRow>
        <SettingRow description="在最后一张按下一张时回到第一张。" title="循环浏览">
          <SettingsSwitch checked={preferences.loop} label="循环浏览" onChange={(loop) => onChange({ loop })} />
        </SettingRow>
        <SettingRow description="相机同时保存 RAW 和 JPEG 时，同名的两个文件只显示为一张图片（显示 RAW，可按 J 切换为内嵌 JPEG）。关闭后两个文件分别显示。" title="合并同名 RAW 与 JPEG">
          <SettingsSwitch checked={preferences.pairRawJpeg} label="合并同名 RAW 与 JPEG" onChange={(pairRawJpeg) => onChange({ pairRawJpeg })} />
        </SettingRow>
        <SettingRow description="把文件夹里的图片纵向连成一条长图，像在浏览器里看条漫一样滚动阅读。按 M 或工具栏按钮开关，Ctrl+滚轮或 +/- 调整宽度，0 恢复铺满宽度。" title="漫画（条漫）模式">
          <SettingsSwitch checked={preferences.comicMode} label="漫画（条漫）模式" onChange={(comicMode) => onChange({ comicMode })} />
        </SettingRow>
        <SettingRow description="漫画模式下相邻两张图片之间的空隙。条漫一般用“无”，单页漫画可以留一点间距。" title="漫画页间距">
          <SettingsSegmented
            label="漫画页间距"
            onChange={(comicGap) => onChange({ comicGap })}
            options={[
              { value: 0, label: "无" },
              { value: 8, label: "窄" },
              { value: 24, label: "宽" }
            ]}
            value={preferences.comicGap}
          />
        </SettingRow>
        <SettingRow description="缩略图条上方的细长拖动条：拖动或悬停即可预览文件夹里任意位置的图片，松开后打开。图片很多时只加载拖动条像素能覆盖到的图片。" title="快速浏览拖动条">
          <SettingsSwitch checked={preferences.showScrubBar} label="快速浏览拖动条" onChange={(showScrubBar) => onChange({ showScrubBar })} />
        </SettingRow>
        <SettingRow description="比较同一构图的多张照片时很有用。" title="切换图片时保持缩放与旋转">
          <SettingsSwitch checked={preferences.keepTransform} label="切换图片时保持缩放与旋转" onChange={(keepTransform) => onChange({ keepTransform })} />
        </SettingRow>
        <SettingRow description="幻灯片放映时每张图片停留的时间。" stacked title="幻灯片间隔">
          <SettingsSegmented
            label="幻灯片间隔"
            onChange={(slideshowSeconds) => onChange({ slideshowSeconds })}
            options={SLIDESHOW_INTERVALS.map((seconds) => ({ value: seconds, label: `${seconds} 秒` }))}
            value={preferences.slideshowSeconds}
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection anchorId="iv-settings-window" icon={<icons.LayoutPanelTop aria-hidden="true" size={14} />} id="window" title="窗口">
        <SettingRow description="关闭后标题栏会自动隐藏，把鼠标移到窗口顶部即可显示。也可以用标题栏上的图钉按钮切换。" title="固定标题栏">
          <SettingsSwitch checked={preferences.titlebarPinned} label="固定标题栏" onChange={(titlebarPinned) => onChange({ titlebarPinned })} />
        </SettingRow>
        <SettingRow description="打开 GIF、APNG、WebP 动图时自动显示帧控制面板，可暂停、逐帧查看和调整速度。" title="动图自动显示帧面板">
          <SettingsSwitch checked={preferences.framesAutoOpen} label="动图自动显示帧面板" onChange={(framesAutoOpen) => onChange({ framesAutoOpen })} />
        </SettingRow>
      </SettingsSection>

      <SettingsSection anchorId="iv-settings-actions" icon={<icons.SlidersHorizontal aria-hidden="true" size={14} />} id="actions" title="操作">
        <SettingRow description="删除的图片会移到回收站，随时可以恢复。" title="删除前确认">
          <SettingsSwitch checked={preferences.confirmDelete} label="删除前确认" onChange={(confirmDelete) => onChange({ confirmDelete })} />
        </SettingRow>
        <SettingRow description="全屏或幻灯片时，Esc 总是先退出全屏或幻灯片。" title="按 Esc 关闭窗口">
          <SettingsSwitch checked={preferences.escapeCloses} label="按 Esc 关闭窗口" onChange={(escapeCloses) => onChange({ escapeCloses })} />
        </SettingRow>
      </SettingsSection>

      {showShortcuts ? (
        <SettingsSection anchorId="iv-settings-shortcuts" icon={<icons.Keyboard aria-hidden="true" size={14} />} id="shortcuts" title="快捷键">
          <ShortcutEditor onChange={(shortcuts: ShortcutOverrides) => onChange({ shortcuts })} overrides={preferences.shortcuts} />
        </SettingsSection>
      ) : null}

      {showSystem ? (
        <SettingsSection anchorId="iv-settings-system" icon={<icons.Settings aria-hidden="true" size={14} />} id="system" title="系统">
          <AssociationControls />
          <ExplorerThumbnailControls />
        </SettingsSection>
      ) : null}
    </div>
  );
}

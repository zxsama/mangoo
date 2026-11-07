import type { ReactNode } from "react";
import { icons } from "@mangoo/web/plugin-host";

import type { ImageSortKey } from "../shared/navigation.ts";
import { AssociationControls } from "./AssociationControls.tsx";
import { SLIDESHOW_INTERVALS, type ViewerBackground, type ViewerPreferences } from "./preferences.ts";

/**
 * The viewer's settings, laid out as a real settings page: titled sections,
 * one row per decision with a short explanation, and the control that fits
 * the decision (switch for on/off, segmented control for a small choice).
 * Rendered in the viewer's settings dialog AND in Plugin Manager; both read
 * the same localStorage preferences, so a change in one shows in the other.
 */

export function Switch({ checked, label, onChange, disabled = false }: {
  checked: boolean;
  label: string;
  onChange(next: boolean): void;
  disabled?: boolean;
}) {
  return (
    <span className={`plugin-toggle iv-switch${checked ? " on" : ""}${disabled ? " disabled" : ""}`}>
      <input aria-label={label} checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} role="switch" type="checkbox" />
      <span aria-hidden="true" className="plugin-toggle-track">
        <span className="plugin-toggle-thumb" />
      </span>
    </span>
  );
}

export function Segmented<T extends string | number>({ value, options, label, onChange }: {
  value: T;
  options: ReadonlyArray<{ value: T; label: ReactNode; title?: string }>;
  label: string;
  onChange(next: T): void;
}) {
  return (
    <div aria-label={label} className="iv-segmented" role="radiogroup">
      {options.map((option) => {
        const selected = option.value === value;
        return (
          <button
            aria-checked={selected}
            className="iv-segment"
            data-pointer-glow="off"
            data-selected={selected ? "true" : "false"}
            key={String(option.value)}
            onClick={() => onChange(option.value)}
            role="radio"
            title={option.title}
            type="button"
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

export function SettingRow({ title, description, children, htmlFor, stacked = false }: {
  title: string;
  stacked?: boolean;
  description?: ReactNode;
  children: ReactNode;
  htmlFor?: string;
}) {
  return (
    <div className="iv-setting-row" data-layout={stacked ? "stacked" : "inline"}>
      <div className="iv-setting-text">
        <label className="iv-setting-title" htmlFor={htmlFor}>{title}</label>
        {description ? <p className="iv-setting-description">{description}</p> : null}
      </div>
      <div className="iv-setting-control">{children}</div>
    </div>
  );
}

function Section({ title, icon, children }: { title: string; icon: ReactNode; children: ReactNode }) {
  return (
    <section className="iv-settings-section">
      <h3 className="iv-settings-section-title">
        {icon}
        <span>{title}</span>
      </h3>
      <div className="iv-settings-rows">{children}</div>
    </section>
  );
}

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

export function ViewerSettingsForm({ preferences, onChange, showSystem = true }: {
  preferences: ViewerPreferences;
  onChange(change: Partial<ViewerPreferences>): void;
  /** The plugin manager shows the system section separately. */
  showSystem?: boolean;
}) {
  return (
    <div className="iv-settings-form">
      <Section icon={<icons.Image aria-hidden="true" size={14} />} title="显示">
        <SettingRow description="图片后面的底色。查看带透明区域的图片时，用棋盘格更容易分辨透明部分。" stacked title="背景">
          <span className="iv-background-swatches" role="radiogroup" aria-label="背景">
            {BACKGROUNDS.map((option) => (
              <button
                aria-checked={preferences.background === option.value}
                className="iv-swatch"
                data-background={option.value}
                data-pointer-glow="off"
                data-selected={preferences.background === option.value ? "true" : "false"}
                key={option.value}
                onClick={() => onChange({ background: option.value })}
                role="radio"
                title={option.label}
                type="button"
              >
                <span className="iv-swatch-chip" aria-hidden="true" />
                <span className="iv-swatch-label">{option.label}</span>
              </button>
            ))}
          </span>
        </SettingRow>
        <SettingRow description="打开或切换图片时的初始大小。" title="默认缩放">
          <Segmented
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
          <Switch checked={preferences.upscaleSmall} label="放大小图片以适应窗口" onChange={(upscaleSmall) => onChange({ upscaleSmall })} />
        </SettingRow>
        <SettingRow description="放大到 200% 以上时清晰显示每个像素，适合检查细节和像素画。" title="高倍放大时显示像素">
          <Switch checked={preferences.pixelatedZoom} label="高倍放大时显示像素" onChange={(pixelatedZoom) => onChange({ pixelatedZoom })} />
        </SettingRow>
      </Section>

      <Section icon={<icons.Images aria-hidden="true" size={14} />} title="浏览">
        <SettingRow description="按住 Ctrl 滚动总是缩放；拖动图片可以平移。" title="鼠标滚轮">
          <Segmented
            label="鼠标滚轮"
            onChange={(wheelAction) => onChange({ wheelAction })}
            options={[
              { value: "zoom", label: "缩放图片" },
              { value: "navigate", label: "切换图片" }
            ]}
            value={preferences.wheelAction}
          />
        </SettingRow>
        <SettingRow description="文件夹里图片的排列顺序，缩略图条和上一张/下一张都按这个顺序。" stacked title="排序方式">
          <span className="iv-setting-inline">
            <Segmented label="排序方式" onChange={(sortKey) => onChange({ sortKey })} options={SORT_KEYS} value={preferences.sortKey} />
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
          <Switch checked={preferences.loop} label="循环浏览" onChange={(loop) => onChange({ loop })} />
        </SettingRow>
        <SettingRow description="比较同一构图的多张照片时很有用。" title="切换图片时保持缩放与旋转">
          <Switch checked={preferences.keepTransform} label="切换图片时保持缩放与旋转" onChange={(keepTransform) => onChange({ keepTransform })} />
        </SettingRow>
        <SettingRow description="幻灯片放映时每张图片停留的时间。" stacked title="幻灯片间隔">
          <Segmented
            label="幻灯片间隔"
            onChange={(slideshowSeconds) => onChange({ slideshowSeconds })}
            options={SLIDESHOW_INTERVALS.map((seconds) => ({ value: seconds, label: `${seconds} 秒` }))}
            value={preferences.slideshowSeconds}
          />
        </SettingRow>
      </Section>

      <Section icon={<icons.SlidersHorizontal aria-hidden="true" size={14} />} title="操作">
        <SettingRow description="删除的图片会移到回收站，随时可以恢复。" title="删除前确认">
          <Switch checked={preferences.confirmDelete} label="删除前确认" onChange={(confirmDelete) => onChange({ confirmDelete })} />
        </SettingRow>
        <SettingRow description="全屏或幻灯片时，Esc 总是先退出全屏或幻灯片。" title="按 Esc 关闭窗口">
          <Switch checked={preferences.escapeCloses} label="按 Esc 关闭窗口" onChange={(escapeCloses) => onChange({ escapeCloses })} />
        </SettingRow>
      </Section>

      {showSystem ? (
        <Section icon={<icons.Settings aria-hidden="true" size={14} />} title="系统">
          <AssociationControls />
        </Section>
      ) : null}
    </div>
  );
}

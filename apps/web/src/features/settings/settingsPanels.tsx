import {
  Check,
  Gauge,
  Images,
  Keyboard,
  Library,
  Notepad,
  Palette,
  Pencil,
  RefreshCw,
  Settings2,
  Trash2
} from "../../design/icons";
import { lazy, Suspense, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import {
  deleteLibraryFile,
  listLibraries,
  openNotificationsWindow,
  renameLibraryEntry,
  setLibraryIcon,
  subscribeLibraryRegistry,
  switchLibrary
} from "../../core/desktop";
import type { LibrarySummary } from "../../core/libraryTypes";
import type { LibraryState } from "../../core/useLibraryData";
import { DEFAULT_INTERFACE_STYLE, LiquidGlassButton, type InterfaceStyleController } from "../../design/liquid-glass";
import { ColorPicker } from "../../design/ColorPicker";
import { ParameterSlider } from "../../design/ParameterSlider";
import { DisclosureSummary } from "../../design/SectionDisclosure";
import { SettingRow, SettingsSegmented, useSettingsNavigation } from "../../design/SettingsControls";
import { useOverlayStack } from "../../design/overlays";
import { INTERFACE_COLOR_ROLES } from "./interfaceStyle";
import { thumbnailFailureLogIntentForLibrary } from "../notifications/notificationModel";
import { LibraryGlyph } from "../library/libraryGlyph";
import { LibraryAppearanceEditor } from "../library/LibraryAppearanceEditor";
import { ManagedLibraryWizard } from "../library/ManagedLibraryWizard";
import { useHostCapability } from "../plugins/hostPlugins";
import { PluginContributionsSlot } from "../plugins/PluginContributionsSlot";
import {
  DEFAULT_LIBRARY_GRID_PREFERENCES,
  LIBRARY_GRID_PREFERENCE_LIMITS,
  type LibraryGridPreferences
} from "../media-grid/gridPreferences";
import {
  DEFAULT_PREVIEW_PREFERENCES,
  PREVIEW_PREFERENCE_LIMITS,
  type PreviewPreferences
} from "../preview/previewPreferences";
import {
  normalizeShortcutEvent,
  useShortcutBindings,
  type ShortcutActionId
} from "../shortcuts/shortcutBindings";

import "./hostSettings.css";

// Localized labels for the shortcut actions. The binding ids are stable; only
// the display copy is translated so the settings UI is fully in Chinese.
const SHORTCUT_GROUPS = ["导航", "文件操作", "预览"] as const;
const KEY_LABELS: Readonly<Record<string, string>> = {
  ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓", Escape: "Esc"
};
const SHORTCUT_ACTIONS: Record<ShortcutActionId, { label: string; group: typeof SHORTCUT_GROUPS[number] }> = {
  focusSearch: { label: "聚焦搜索", group: "导航" },
  renameSelected: { label: "重命名所选文件", group: "文件操作" },
  recycleDelete: { label: "移到回收站", group: "文件操作" },
  permanentDelete: { label: "永久删除", group: "文件操作" },
  closeOrReturn: { label: "关闭 / 返回", group: "导航" },
  previewNext: { label: "下一张预览", group: "预览" },
  previewPrevious: { label: "上一张预览", group: "预览" },
  zoomIn: { label: "放大", group: "预览" },
  zoomOut: { label: "缩小", group: "预览" },
  toggleSidebars: { label: "切换侧栏", group: "导航" }
};

interface SettingsPanelsProps {
  gridPreferences: LibraryGridPreferences;
  interfaceStyle: InterfaceStyleController;
  library: LibraryState;
  onGridPreferencesChange: (patch: Partial<LibraryGridPreferences>) => void;
  onPreviewPreferencesChange: (patch: Partial<PreviewPreferences>) => void;
  previewPreferences: PreviewPreferences;
}

const TABS = [
  { id: "general", label: "常用", icon: Settings2 },
  { id: "libraries", label: "资源库", icon: Library },
  { id: "thumbnails", label: "缩略图", icon: Images },
  { id: "appearance", label: "外观", icon: Palette },
  { id: "shortcuts", label: "快捷键", icon: Keyboard },
  { id: "diagnostics", label: "诊断", icon: Gauge }
] as const;

const TABS_WITH_NOTEBOOK = [...TABS, { id: "notebook", label: "笔记", icon: Notepad }] as const;
type SectionId = (typeof TABS_WITH_NOTEBOOK)[number]["id"];
const NotebookSettings = lazy(() => import("@mangoo/plugin-notebook/settings").then(
  ({ NotebookSettings }) => ({ default: NotebookSettings })
));

export function SettingsPanels({
  gridPreferences,
  interfaceStyle,
  library,
  onGridPreferencesChange,
  onPreviewPreferencesChange,
  previewPreferences
}: SettingsPanelsProps) {
  const notebookEnabled = useHostCapability("notebook.v1");
  const tabs = notebookEnabled ? TABS_WITH_NOTEBOOK : TABS;
  const sectionIds = useMemo(() => tabs.map(({ id }) => id), [tabs]);
  const { active, jumpTo, scrollRef } = useSettingsNavigation(sectionIds);
  const content: Record<SectionId, ReactNode> = {
    general: <GeneralTab onNavigate={jumpTo} onPreviewPreferencesChange={onPreviewPreferencesChange} previewPreferences={previewPreferences} />,
    libraries: <LibrariesTab />,
    thumbnails: (
      <ThumbnailsTab
        current={active === "thumbnails"}
        library={library}
        onPreviewPreferencesChange={onPreviewPreferencesChange}
        previewPreferences={previewPreferences}
      />
    ),
    appearance: (
      <AppearanceTab
        gridPreferences={gridPreferences}
        interfaceStyle={interfaceStyle}
        onGridPreferencesChange={onGridPreferencesChange}
      />
    ),
    shortcuts: <ShortcutsTab />,
    diagnostics: <DiagnosticsTab library={library} />,
    notebook: (
      <Suspense fallback={<p role="status">正在载入笔记设置…</p>}>
        <NotebookSettings />
      </Suspense>
    )
  };

  return (
    <div aria-label="偏好设置" className="settings-layout-scroll host-settings" ref={scrollRef} role="region" tabIndex={0}>
      <div className="settings-layout-body host-settings-columns">
        <nav className="settings-layout-nav settings-dialog-nav" aria-label="设置分类">
          {tabs.map(({ id, label, icon: Icon }) => (
            <button
              aria-controls={`settings-${id}`}
              aria-current={active === id ? "location" : undefined}
              className="settings-layout-nav-item settings-dialog-nav-item"
              data-active={active === id}
              key={id}
              onClick={() => jumpTo(id)}
              type="button"
            >
              <Icon aria-hidden="true" size={15} />
              <span>{label}</span>
            </button>
          ))}
        </nav>
        <main className="settings-dialog-content settings-dialog-main">
          <div className="settings-layout-form host-settings-page">
            {tabs.map(({ id, label, icon: Icon }) => (
              <section aria-labelledby={`settings-heading-${id}`} className="host-settings-category" data-section={id} id={`settings-${id}`} key={id}>
                <header className="host-settings-heading">
                  <h2 className="settings-control-heading" id={`settings-heading-${id}`}>
                    <Icon aria-hidden="true" size={14} /><span>{label}</span>
                  </h2>
                  {id === "appearance" && (
                    <LiquidGlassButton onClick={interfaceStyle.resetInterfaceStyle} type="button">恢复默认外观</LiquidGlassButton>
                  )}
                </header>
                <div className="settings-control-rows host-settings-sheet">{content[id]}</div>
              </section>
            ))}
          </div>
        </main>
      </div>
    </div>
  );
}

function SettingsGroup({ title, children, className = "", collapsible = false, groupRef }: {
  title?: string;
  children: ReactNode;
  className?: string;
  collapsible?: boolean;
  groupRef?: (element: HTMLElement | null) => void;
}) {
  const classes = `settings-control-section host-settings-group ${className}`.trimEnd();
  const body = <div className="settings-control-rows settings-group-body">{children}</div>;
  if (collapsible && title) {
    return (
      <details className={classes} ref={groupRef}>
        <DisclosureSummary label={title} />
        {body}
      </details>
    );
  }
  if (!title) return <div className={classes} ref={groupRef}>{body}</div>;
  return (
    <section className={classes} ref={groupRef}>
      <h3 className="settings-control-heading">{title}</h3>
      {body}
    </section>
  );
}

// ---- General tab: common tasks and memory buffers -------------------------

function GeneralTab({
  onNavigate,
  onPreviewPreferencesChange,
  previewPreferences
}: {
  onNavigate: (id: SectionId) => void;
  onPreviewPreferencesChange: (patch: Partial<PreviewPreferences>) => void;
  previewPreferences: PreviewPreferences;
}) {
  return (
    <>
      <SettingsGroup className="host-settings-parameters" title="浏览缓存（内存）">
        <StyleSlider
          id="preview-buffer-limit"
          label="预览缓冲区"
          description="浏览时可在内存中保留的原图数据量。"
          max={PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.max}
          min={PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.min}
          onChange={(previewBufferLimitMb) => onPreviewPreferencesChange({ previewBufferLimitMb })}
          step={PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.step}
          unit="MB"
          resetValue={DEFAULT_PREVIEW_PREFERENCES.previewBufferLimitMb}
          value={previewPreferences.previewBufferLimitMb}
        />
        <StyleSlider
          id="thumbnail-cache-limit"
          label="缩略图缓存"
          description="本次会话内可在内存中保留的缩略图数据量。"
          max={PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.max}
          min={PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.min}
          onChange={(thumbnailCacheLimitMb) => onPreviewPreferencesChange({ thumbnailCacheLimitMb })}
          step={PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.step}
          unit="MB"
          resetValue={DEFAULT_PREVIEW_PREFERENCES.thumbnailCacheLimitMb}
          value={previewPreferences.thumbnailCacheLimitMb}
        />
      </SettingsGroup>
      <div className="host-settings-jumps" role="group" aria-label="常用操作">
        <span>快速前往</span>
        <div className="settings-action-row">
          <button onClick={() => onNavigate("libraries")} type="button"><Library aria-hidden="true" size={15} /><span data-button-content>管理资源库</span></button>
          <button onClick={() => onNavigate("thumbnails")} type="button"><Images aria-hidden="true" size={15} /><span data-button-content>查看生成状况</span></button>
          <button onClick={() => onNavigate("appearance")} type="button"><Palette aria-hidden="true" size={15} /><span data-button-content>调整外观</span></button>
        </div>
      </div>
    </>
  );
}

// ---- Thumbnails tab: live generation status + generation mode --------------

const thumbnailModes = [
  { value: "renderedOnly", label: "浏览时入库", title: "默认仅已刷新缩略图入库，跳过尚未浏览的媒体。" },
  { value: "allPersist", label: "全库入库", title: "打开资源库后自动后台生成全部缩略图并入库，优先处理正在浏览的内容。" },
  { value: "cacheOnly", label: "仅本次缓存", title: "只缓存不入库，关闭应用后释放本次浏览的缩略图。" }
] as const;

function ThumbnailsTab({
  current,
  library,
  onPreviewPreferencesChange,
  previewPreferences
}: {
  current: boolean;
  library: LibraryState;
  onPreviewPreferencesChange: (patch: Partial<PreviewPreferences>) => void;
  previewPreferences: PreviewPreferences;
}) {
  const stats = library.thumbnailCacheStats;
  const busy = library.thumbnailCacheActionBusy;
  const activeBulk = stats?.activeBulkTaskCount ?? 0;
  const refreshThumbnailCacheStats = library.refreshThumbnailCacheStats;
  // Poll faster only while a bulk generation is active; otherwise slowly.
  const statsPollActive = activeBulk > 0 || busy;
  const [statsGroup, setStatsGroup] = useState<HTMLElement | null>(null);
  const [statsVisible, setStatsVisible] = useState(false);
  useEffect(() => {
    if (!statsGroup) return;
    const observer = new IntersectionObserver(([entry]) => setStatsVisible(entry.isIntersecting));
    observer.observe(statsGroup);
    return () => observer.disconnect();
  }, [statsGroup]);

  // A reader at the end of Libraries can see these numbers before the active
  // heading changes to Thumbnails. Keep one query interval across that boundary.
  const pollStats = current || statsVisible;
  useEffect(() => {
    if (!pollStats) return;
    void refreshThumbnailCacheStats();
    const interval = window.setInterval(
      () => {
        void refreshThumbnailCacheStats();
      },
      statsPollActive ? 8000 : 20000
    );
    return () => window.clearInterval(interval);
  }, [pollStats, refreshThumbnailCacheStats, statsPollActive]);

  // Progress estimate: cached / (cached + missing + stale). failed excluded from
  // the "done" fraction but shown separately.
  const cached = stats?.cachedCount ?? 0;
  const remaining = stats ? stats.missingCount + stats.staleCount : 0;
  const failed = stats?.failedCount ?? 0;
  const total = cached + remaining;
  const percent = total > 0 ? Math.min(100, Math.round((cached / total) * 100)) : 0;
  let progressCaption = "读取中…";
  if (activeBulk > 0) progressCaption = `正在后台生成 · ${activeBulk.toLocaleString()} 个任务进行中`;
  else if (remaining > 0) progressCaption = `待生成 ${remaining.toLocaleString()} 项`;
  else if (failed > 0) progressCaption = `有 ${failed.toLocaleString()} 项生成失败`;
  else if (stats) progressCaption = total > 0 ? "全部缩略图已就绪" : "当前资源库没有可生成缩略图的媒体";

  const generationMode = previewPreferences.thumbnailGenerationMode;
  const currentFolderDisabled = library.selectedFolderId === null || busy;

  return (
    <>
      <SettingsGroup>
        <SettingRow title="生成模式" description={thumbnailModes.find(mode => mode.value === generationMode)?.title}>
          <SettingsSegmented
            label="缩略图生成模式"
            onChange={thumbnailGenerationMode => onPreviewPreferencesChange({ thumbnailGenerationMode })}
            options={thumbnailModes}
            value={generationMode}
          />
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup className="host-settings-cache" title="生成状况" groupRef={setStatsGroup}>
        <div className="host-settings-cache-status">
          <div className="settings-progress">
            <div className="settings-progress-head">
              <span className="settings-progress-label">
                已入库 {cached.toLocaleString()} / {total.toLocaleString()}
              </span>
              <span className="settings-progress-percent">{percent}%</span>
            </div>
            <div
              aria-label="缩略图生成进度"
              aria-valuemax={100}
              aria-valuemin={0}
              aria-valuenow={percent}
              className="settings-progress-track"
              role="progressbar"
            >
              <div className="settings-progress-fill" style={{ width: `${percent}%` }} />
            </div>
            <div className="settings-progress-caption">{progressCaption}</div>
          </div>
          <LiquidGlassButton
            disabled={library.thumbnailCacheStatsLoading}
            onClick={() => void library.refreshThumbnailCacheStats()}
            type="button"
          >
            <RefreshCw aria-hidden="true" size={13} />
            <span>{library.thumbnailCacheStatsLoading ? "刷新中…" : "刷新统计"}</span>
          </LiquidGlassButton>
        </div>
        <dl className="settings-stat-grid">
          <StatCell label="已缓存" value={stats ? cached.toLocaleString() : "-"} />
          <StatCell label="待生成 / 过期" value={stats ? remaining.toLocaleString() : "-"} />
          <StatCell
            label="失败"
            onActivate={failed > 0 ? () => {
              void library.refreshThumbnailCacheStats({
                requireNotificationPersistence: true
              }).then((freshStats) => {
                if (!freshStats) return;
                const intent = thumbnailFailureLogIntentForLibrary(
                  library.thumbnailCacheStatsLibraryId,
                  freshStats.failedCount,
                  freshStats.failureDetails.length,
                  freshStats.failureDetailsTruncated
                );
                if (intent) void openNotificationsWindow(intent);
              });
            } : undefined}
            value={stats ? failed.toLocaleString() : "-"}
          />
          <StatCell label="活动任务" value={stats ? activeBulk.toLocaleString() : "-"} />
          <StatCell
            label="数据库占用"
            value={stats ? formatCacheBytes(stats.totalBlobBytes) : "-"}
          />
        </dl>
      </SettingsGroup>

      <SettingsGroup className="host-settings-actions" title="缓存操作">
        <SettingRow title="手动生成" description={library.selectedFolderId === null ? "选择文件夹后可按目录生成。" : undefined}>
          <LiquidGlassButton
            disabled={currentFolderDisabled}
            onClick={() => void library.generateCurrentFolderThumbnailCache()}
            type="button"
          >
            生成当前文件夹
          </LiquidGlassButton>
          <LiquidGlassButton
            disabled={currentFolderDisabled}
            onClick={() => void library.generateCurrentTreeThumbnailCache()}
            type="button"
          >
            生成当前目录树
          </LiquidGlassButton>
          <LiquidGlassButton
            disabled={busy}
            onClick={() => void library.generateAllThumbnailCache()}
            type="button"
          >
            {busy ? "生成中…" : "生成全部缩略图"}
          </LiquidGlassButton>
        </SettingRow>
        <SettingRow title="维护缓存">
          <LiquidGlassButton
            disabled={busy}
            onClick={() => void library.retryThumbnailCacheFailures()}
            type="button"
          >
            重试失败 / 过期
          </LiquidGlassButton>
          <LiquidGlassButton
            disabled={busy}
            onClick={() => void library.clearThumbnailCache()}
            tone="danger"
            type="button"
          >
            <Trash2 aria-hidden="true" size={13} />
            <span>清空持久缓存</span>
          </LiquidGlassButton>
        </SettingRow>
      </SettingsGroup>
    </>
  );
}

function StatCell({
  label,
  onActivate,
  value
}: {
  label: string;
  onActivate?: () => void;
  value: string;
}) {
  return (
    <div className="settings-stat-cell">
      <dt>{label}</dt>
      <dd>
        {onActivate ? (
          <LiquidGlassButton aria-label={`查看 ${value} 项失败记录`} onClick={onActivate} type="button">
            {value}
          </LiquidGlassButton>
        ) : value}
      </dd>
    </div>
  );
}

// ---- Appearance tab: liquid-glass interface style --------------------------

function AppearanceTab({
  interfaceStyle,
  gridPreferences,
  onGridPreferencesChange
}: {
  interfaceStyle: InterfaceStyleController;
  gridPreferences: LibraryGridPreferences;
  onGridPreferencesChange: (patch: Partial<LibraryGridPreferences>) => void;
}) {
  const { limits, value, setInterfaceStyle, previewInterfaceColor } = interfaceStyle;
  return (
    <>
      <SettingsGroup>
        <ColorField
          id="theme-accent-color"
          label="主题色"
          description="用于主界面和插件的选中状态、焦点与交互高光。"
          onChange={(accentColor) => setInterfaceStyle({ accentColor })}
          onPreview={(color) => previewInterfaceColor("accentColor", color)}
          value={value.accentColor}
        />
        <details className="settings-control-group host-settings-role-colors">
          <DisclosureSummary label="状态与标记颜色" />
          <div className="host-settings-palette">
            {INTERFACE_COLOR_ROLES.map(({ role, label }) => (
              <ColorField
                key={role}
                id={`theme-role-${role}`}
                label={label}
                onChange={(color) => setInterfaceStyle({ roleColors: { [role]: color } })}
                onPreview={(color) => previewInterfaceColor(role, color)}
                value={value.roleColors[role]}
              />
            ))}
          </div>
        </details>
      </SettingsGroup>
      <SettingsGroup className="host-settings-parameters" title="网格">
        <StyleSlider
          id="library-grid-gap"
          label="缩略图间距"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.tileGap.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.tileGap.min}
          onChange={(tileGap) => onGridPreferencesChange({ tileGap })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.tileGap.step}
          unit="px"
          resetValue={DEFAULT_LIBRARY_GRID_PREFERENCES.tileGap}
          value={gridPreferences.tileGap}
        />
        <StyleSlider
          id="library-grid-label-line-height"
          label="名称行距"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.labelLineHeight.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.labelLineHeight.min}
          onChange={(labelLineHeight) => onGridPreferencesChange({ labelLineHeight })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.labelLineHeight.step}
          unit="px"
          resetValue={DEFAULT_LIBRARY_GRID_PREFERENCES.labelLineHeight}
          value={gridPreferences.labelLineHeight}
        />
        <StyleSlider
          id="library-grid-label-gap"
          label="名称距离"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.labelGap.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.labelGap.min}
          onChange={(labelGap) => onGridPreferencesChange({ labelGap })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.labelGap.step}
          unit="px"
          resetValue={DEFAULT_LIBRARY_GRID_PREFERENCES.labelGap}
          value={gridPreferences.labelGap}
        />
        <StyleSlider
          id="library-folder-edge-shadow-alpha"
          label="文件夹边缘阴影"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.folderEdgeShadowAlpha.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.folderEdgeShadowAlpha.min}
          onChange={(folderEdgeShadowAlpha) => onGridPreferencesChange({ folderEdgeShadowAlpha })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.folderEdgeShadowAlpha.step}
          unit="%"
          resetValue={DEFAULT_LIBRARY_GRID_PREFERENCES.folderEdgeShadowAlpha}
          value={gridPreferences.folderEdgeShadowAlpha}
        />
      </SettingsGroup>
      <SettingsGroup className="host-settings-parameters" title="圆角">
        <StyleSlider
          id="window-corner-radius"
          label="窗口圆角"
          max={limits.windowCornerRadius.max}
          min={limits.windowCornerRadius.min}
          onChange={(windowCornerRadius) => setInterfaceStyle({ windowCornerRadius })}
          step={limits.windowCornerRadius.step}
          unit="px"
          resetValue={DEFAULT_INTERFACE_STYLE.windowCornerRadius}
          value={value.windowCornerRadius}
        />
        <StyleSlider
          id="panel-control-corner-radius"
          label="面板与控件圆角"
          max={limits.panelControlCornerRadius.max}
          min={limits.panelControlCornerRadius.min}
          onChange={(panelControlCornerRadius) => setInterfaceStyle({ panelControlCornerRadius })}
          step={limits.panelControlCornerRadius.step}
          unit="px"
          resetValue={DEFAULT_INTERFACE_STYLE.panelControlCornerRadius}
          value={value.panelControlCornerRadius}
        />
        <StyleSlider
          id="content-corner-radius"
          label="内容圆角"
          max={limits.contentCornerRadius.max}
          min={limits.contentCornerRadius.min}
          onChange={(contentCornerRadius) => setInterfaceStyle({ contentCornerRadius })}
          step={limits.contentCornerRadius.step}
          unit="px"
          resetValue={DEFAULT_INTERFACE_STYLE.contentCornerRadius}
          value={value.contentCornerRadius}
        />
      </SettingsGroup>
      <div className="host-settings-materials">
        <h3>材质与交互</h3>
        <SettingsGroup className="host-settings-parameters" title="侧栏材质" collapsible>
          <StyleSlider id="side-blur" label="模糊" max={limits.sideBlur.max} min={limits.sideBlur.min} onChange={(sideBlur) => setInterfaceStyle({ sideBlur })} step={limits.sideBlur.step} resetValue={DEFAULT_INTERFACE_STYLE.sideBlur} value={value.sideBlur} />
          <StyleSlider id="side-opacity" label="不透明度" max={limits.sideOpacity.max} min={limits.sideOpacity.min} onChange={(sideOpacity) => setInterfaceStyle({ sideOpacity })} step={limits.sideOpacity.step} resetValue={DEFAULT_INTERFACE_STYLE.sideOpacity} value={value.sideOpacity} />
          <StyleSlider id="side-overlay-strength" label="叠加强度" max={limits.sideOverlayStrength.max} min={limits.sideOverlayStrength.min} onChange={(sideOverlayStrength) => setInterfaceStyle({ sideOverlayStrength })} step={limits.sideOverlayStrength.step} resetValue={DEFAULT_INTERFACE_STYLE.sideOverlayStrength} value={value.sideOverlayStrength} />
          <ColorField id="side-overlay-color" label="叠加颜色" onChange={(sideOverlayColor) => setInterfaceStyle({ sideOverlayColor })} onPreview={(color) => previewInterfaceColor("sideOverlayColor", color)} value={value.sideOverlayColor} />
          <StyleSlider id="side-saturation" label="饱和度" max={limits.sideSaturation.max} min={limits.sideSaturation.min} onChange={(sideSaturation) => setInterfaceStyle({ sideSaturation })} step={limits.sideSaturation.step} resetValue={DEFAULT_INTERFACE_STYLE.sideSaturation} value={value.sideSaturation} />
          <StyleSlider id="side-stroke-opacity" label="描边不透明度" max={limits.sideStrokeOpacity.max} min={limits.sideStrokeOpacity.min} onChange={(sideStrokeOpacity) => setInterfaceStyle({ sideStrokeOpacity })} step={limits.sideStrokeOpacity.step} resetValue={DEFAULT_INTERFACE_STYLE.sideStrokeOpacity} value={value.sideStrokeOpacity} />
        </SettingsGroup>
        <SettingsGroup className="host-settings-parameters" title="中央工作区材质" collapsible>
          <StyleSlider id="center-blur" label="模糊" max={limits.centerBlur.max} min={limits.centerBlur.min} onChange={(centerBlur) => setInterfaceStyle({ centerBlur })} step={limits.centerBlur.step} resetValue={DEFAULT_INTERFACE_STYLE.centerBlur} value={value.centerBlur} />
          <StyleSlider id="center-opacity" label="不透明度" max={limits.centerOpacity.max} min={limits.centerOpacity.min} onChange={(centerOpacity) => setInterfaceStyle({ centerOpacity })} step={limits.centerOpacity.step} resetValue={DEFAULT_INTERFACE_STYLE.centerOpacity} value={value.centerOpacity} />
          <StyleSlider id="center-overlay-strength" label="叠加强度" max={limits.centerOverlayStrength.max} min={limits.centerOverlayStrength.min} onChange={(centerOverlayStrength) => setInterfaceStyle({ centerOverlayStrength })} step={limits.centerOverlayStrength.step} resetValue={DEFAULT_INTERFACE_STYLE.centerOverlayStrength} value={value.centerOverlayStrength} />
          <ColorField id="center-overlay-color" label="叠加颜色" onChange={(centerOverlayColor) => setInterfaceStyle({ centerOverlayColor })} onPreview={(color) => previewInterfaceColor("centerOverlayColor", color)} value={value.centerOverlayColor} />
          <StyleSlider id="center-saturation" label="饱和度" max={limits.centerSaturation.max} min={limits.centerSaturation.min} onChange={(centerSaturation) => setInterfaceStyle({ centerSaturation })} step={limits.centerSaturation.step} resetValue={DEFAULT_INTERFACE_STYLE.centerSaturation} value={value.centerSaturation} />
          <StyleSlider id="center-stroke-opacity" label="描边不透明度" max={limits.centerStrokeOpacity.max} min={limits.centerStrokeOpacity.min} onChange={(centerStrokeOpacity) => setInterfaceStyle({ centerStrokeOpacity })} step={limits.centerStrokeOpacity.step} resetValue={DEFAULT_INTERFACE_STYLE.centerStrokeOpacity} value={value.centerStrokeOpacity} />
        </SettingsGroup>
        <SettingsGroup className="host-settings-parameters" title="玻璃平滑度" collapsible>
          <StyleSlider id="dither-opacity" label="抖动强度" max={limits.ditherOpacity.max} min={limits.ditherOpacity.min} onChange={(ditherOpacity) => setInterfaceStyle({ ditherOpacity })} step={limits.ditherOpacity.step} resetValue={DEFAULT_INTERFACE_STYLE.ditherOpacity} value={value.ditherOpacity} />
          <StyleSlider id="backdrop-gradient-strength" label="背景渐变强度" max={limits.backdropGradientStrength.max} min={limits.backdropGradientStrength.min} onChange={(backdropGradientStrength) => setInterfaceStyle({ backdropGradientStrength })} step={limits.backdropGradientStrength.step} resetValue={DEFAULT_INTERFACE_STYLE.backdropGradientStrength} value={value.backdropGradientStrength} />
        </SettingsGroup>
        <SettingsGroup className="host-settings-parameters" title="交互高光" collapsible>
          <StyleSlider id="edge-highlight-brightness" label="边缘高光亮度" max={limits.edgeHighlightBrightness.max} min={limits.edgeHighlightBrightness.min} onChange={(edgeHighlightBrightness) => setInterfaceStyle({ edgeHighlightBrightness })} step={limits.edgeHighlightBrightness.step} resetValue={DEFAULT_INTERFACE_STYLE.edgeHighlightBrightness} value={value.edgeHighlightBrightness} />
          <StyleSlider id="halo-brightness" label="光晕亮度" max={limits.haloBrightness.max} min={limits.haloBrightness.min} onChange={(haloBrightness) => setInterfaceStyle({ haloBrightness })} step={limits.haloBrightness.step} resetValue={DEFAULT_INTERFACE_STYLE.haloBrightness} value={value.haloBrightness} />
          <StyleSlider id="pointer-response-radius" label="指针响应半径" max={limits.pointerResponseRadius.max} min={limits.pointerResponseRadius.min} onChange={(pointerResponseRadius) => setInterfaceStyle({ pointerResponseRadius })} step={limits.pointerResponseRadius.step} resetValue={DEFAULT_INTERFACE_STYLE.pointerResponseRadius} value={value.pointerResponseRadius} />
          <StyleSlider id="pointer-sensing-distance" label="感应距离" max={limits.pointerSensingDistance.max} min={limits.pointerSensingDistance.min} onChange={(pointerSensingDistance) => setInterfaceStyle({ pointerSensingDistance })} step={limits.pointerSensingDistance.step} resetValue={DEFAULT_INTERFACE_STYLE.pointerSensingDistance} value={value.pointerSensingDistance} />
          <StyleSlider id="pointer-falloff" label="衰减曲线" max={limits.pointerFalloff.max} min={limits.pointerFalloff.min} onChange={(pointerFalloff) => setInterfaceStyle({ pointerFalloff })} step={limits.pointerFalloff.step} resetValue={DEFAULT_INTERFACE_STYLE.pointerFalloff} value={value.pointerFalloff} />
        </SettingsGroup>
        <SettingsGroup className="host-settings-parameters" title="对话框材质" collapsible>
          <StyleSlider id="dialog-blur" label="对话框模糊" max={limits.dialogBlur.max} min={limits.dialogBlur.min} onChange={(dialogBlur) => setInterfaceStyle({ dialogBlur })} step={limits.dialogBlur.step} resetValue={DEFAULT_INTERFACE_STYLE.dialogBlur} value={value.dialogBlur} />
          <StyleSlider id="dialog-opacity" label="对话框不透明度" max={limits.dialogOpacity.max} min={limits.dialogOpacity.min} onChange={(dialogOpacity) => setInterfaceStyle({ dialogOpacity })} step={limits.dialogOpacity.step} resetValue={DEFAULT_INTERFACE_STYLE.dialogOpacity} value={value.dialogOpacity} />
          <StyleSlider id="dialog-overlay-strength" label="对话框叠加强度" max={limits.dialogOverlayStrength.max} min={limits.dialogOverlayStrength.min} onChange={(dialogOverlayStrength) => setInterfaceStyle({ dialogOverlayStrength })} step={limits.dialogOverlayStrength.step} resetValue={DEFAULT_INTERFACE_STYLE.dialogOverlayStrength} value={value.dialogOverlayStrength} />
          <StyleSlider id="dialog-backdrop-dim" label="对话框背景变暗" max={limits.dialogBackdropDim.max} min={limits.dialogBackdropDim.min} onChange={(dialogBackdropDim) => setInterfaceStyle({ dialogBackdropDim })} step={limits.dialogBackdropDim.step} resetValue={DEFAULT_INTERFACE_STYLE.dialogBackdropDim} value={value.dialogBackdropDim} />
        </SettingsGroup>
      </div>
    </>
  );
}

// ---- Shortcuts tab ---------------------------------------------------------

function ShortcutsTab() {
  const { actions, bindings, conflicts, onReset, setBinding } = useShortcutBindings();
  const [capturing, setCapturing] = useState<ShortcutActionId | null>(null);
  const captureButton = useRef<HTMLButtonElement>(null);
  const labelFor = (id: ShortcutActionId) => SHORTCUT_ACTIONS[id].label;

  useEffect(() => {
    if (!capturing || !captureButton.current) return;
    // A wheel scroll can take the focused capture button out of view without a blur.
    const observer = new IntersectionObserver(([entry]) => {
      if (!entry.isIntersecting) setCapturing(null);
    });
    observer.observe(captureButton.current);
    return () => observer.disconnect();
  }, [capturing]);

  function handleCaptureKeyDown(
    event: KeyboardEvent<HTMLButtonElement>,
    actionId: ShortcutActionId
  ) {
    if (capturing !== actionId) return;
    event.preventDefault();
    event.stopPropagation();
    const nextBinding = normalizeShortcutEvent(event);
    if (!nextBinding) return;
    setBinding(actionId, nextBinding);
    setCapturing(null);
  }

  function renderBinding(action: typeof actions[number]) {
    const actionConflicts = conflicts[action.id] ?? [];
    return (
      <SettingRow
        key={action.id}
        title={labelFor(action.id)}
        description={actionConflicts.length ? <span className="shortcut-binding-conflict">
          与 {actionConflicts.map(labelFor).join("、")} 冲突
        </span> : undefined}
      >
        <LiquidGlassButton
          active={capturing === action.id}
          className="shortcut-binding-capture"
          aria-label={`${labelFor(action.id)}：${capturing === action.id ? "请按下快捷键" : bindings[action.id]}`}
          data-shortcut-capture="true"
          onBlur={() => {
            if (capturing === action.id) setCapturing(null);
          }}
          onClick={() => setCapturing(action.id)}
          onKeyDown={(event) => handleCaptureKeyDown(event, action.id)}
          ref={capturing === action.id ? captureButton : undefined}
          type="button"
        >
          {capturing === action.id ? "请按下快捷键" : (
            <span className="host-settings-keys" aria-hidden="true">
              {bindings[action.id].split("+").map((key, index) => (
                <span key={index}>{index > 0 && <small>+</small>}<kbd>{KEY_LABELS[key] ?? key}</kbd></span>
              ))}
            </span>
          )}
        </LiquidGlassButton>
      </SettingRow>
    );
  }

  return (
    <>
      <div className="host-settings-toolbar">
        <p>点击按键，再按下新的组合键。</p>
        <LiquidGlassButton onClick={onReset} type="button">
          恢复默认快捷键
        </LiquidGlassButton>
      </div>
      {SHORTCUT_GROUPS.map((group) => (
        <SettingsGroup key={group} title={group}>
          {actions.filter((action) => SHORTCUT_ACTIONS[action.id].group === group).map(renderBinding)}
        </SettingsGroup>
      ))}
    </>
  );
}

// ---- Libraries tab: full library management (superset of the switcher) -----

function LibrariesTab() {
  const overlays = useOverlayStack();
  const [libs, setLibs] = useState<LibrarySummary[]>([]);
  const [appearanceTarget, setAppearanceTarget] = useState<LibrarySummary | null>(null);
  const appearanceRestoreFocusRef = useRef<HTMLElement | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleteConfirmText, setDeleteConfirmText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [wizardOpen, setWizardOpen] = useState(false);
  const wizardRestoreFocusRef = useRef<HTMLElement | null>(null);
  const registryMountedRef = useRef(false);
  const registryReloadGenerationRef = useRef(0);

  const openManagedLibraryWizard = (
    source: HTMLButtonElement,
  ) => {
    wizardRestoreFocusRef.current = source;
    overlays.openRoot("managed-library");
    setWizardOpen(true);
  };

  const closeManagedLibraryWizard = () => {
    setWizardOpen(false);
    overlays.closeRoot("managed-library");
  };

  const reload = () => {
    if (!registryMountedRef.current) return;
    const generation = ++registryReloadGenerationRef.current;
    void listLibraries().then((nextLibraries) => {
      if (!registryMountedRef.current || generation !== registryReloadGenerationRef.current) return;
      setLibs(nextLibraries);
      const listed = new Set(nextLibraries.map((entry) => entry.libraryUuid));
      setAppearanceTarget((current) => current && listed.has(current.libraryUuid) ? current : null);
      setRenamingId((current) => current && listed.has(current) ? current : null);
      setConfirmDeleteId((current) => current && listed.has(current) ? current : null);
    }).catch((cause: unknown) => {
      if (!registryMountedRef.current || generation !== registryReloadGenerationRef.current) return;
      setError(cause instanceof Error ? cause.message : "资源库列表加载失败");
    });
  };
  useEffect(() => {
    registryMountedRef.current = true;
    reload();
    const unsubscribe = subscribeLibraryRegistry(reload);
    return () => {
      registryMountedRef.current = false;
      registryReloadGenerationRef.current += 1;
      unsubscribe();
    };
  }, []);

  async function onSwitch(l: LibrarySummary) {
    if (l.active || l.availability !== "available") return;
    setAppearanceTarget(null);
    setConfirmDeleteId(null);
    const res = await switchLibrary(l.libraryUuid);
    if (!res.ok) setError(res.error ?? "切换失败");
    reload();
  }

  async function commitRename(l: LibrarySummary) {
    const name = renameValue.trim();
    if (name && name !== l.name) {
      await renameLibraryEntry(l.libraryUuid, name);
    }
    setRenamingId(null);
    reload();
  }

  async function onDeleteFile(l: LibrarySummary) {
    if (l.active) return;
    const res = await deleteLibraryFile(l.libraryUuid);
    if (!res.ok) setError(res.error ?? "删除失败");
    setConfirmDeleteId(null);
    setDeleteConfirmText("");
    reload();
  }

  return (
    <>
      {appearanceTarget ? <LibraryAppearanceEditor
        anchor={appearanceRestoreFocusRef.current} color={appearanceTarget.color} icon={appearanceTarget.icon} id="settings-library-appearance" name={appearanceTarget.name}
        onApply={async (icon, color) => { await setLibraryIcon(appearanceTarget.libraryUuid, icon, color); }}
        onClose={() => setAppearanceTarget(null)} open restoreFocusRef={appearanceRestoreFocusRef} rootId="library"
      /> : null}
      {error ? <div className="settings-inline-error">{error}</div> : null}
      <SettingsGroup>
        <ul className="settings-library-list">
          {libs.map((l) => (
            <li
              key={l.libraryUuid}
              className={`settings-library-row${l.active ? " active" : ""}${
                l.availability === "available" ? "" : " unavailable"
              }`}
            >
              <div className="settings-library-main">
                <div className="settings-library-icon-anchor">
                  <button
                    type="button"
                    className="settings-library-icon"
                    aria-label="更改图标"
                    aria-haspopup="dialog"
                    onClick={(event) => { appearanceRestoreFocusRef.current = event.currentTarget; setAppearanceTarget(l); }}
                  >
                    <LibraryGlyph icon={l.icon ?? undefined} color={l.color ?? undefined} size={18} />
                  </button>

                </div>
                <div className="settings-library-info">
                  {renamingId === l.libraryUuid ? (
                    <input
                      aria-label="资源库名称"
                      className="settings-library-rename"
                      autoFocus
                      value={renameValue}
                      onChange={(event) => setRenameValue(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") void commitRename(l);
                        if (event.key === "Escape") setRenamingId(null);
                      }}
                      onBlur={() => void commitRename(l)}
                    />
                  ) : (
                    <div className="settings-library-name">
                      <span title={l.name}>{l.name}</span>
                      {l.active ? (
                        <span className="settings-library-badge active">
                          <Check size={12} aria-hidden="true" /> 当前
                        </span>
                      ) : null}
                      {l.availability !== "available" ? (
                        <span className="settings-library-badge missing">文件缺失</span>
                      ) : null}
                    </div>
                  )}
                  <div className="settings-library-path" title={l.databasePath}>
                    {l.databasePath}
                  </div>
                </div>
                <div className="settings-library-actions">
                  <LiquidGlassButton
                    type="button"
                    disabled={l.active || l.availability !== "available"}
                    onClick={() => void onSwitch(l)}
                  >
                    切换
                  </LiquidGlassButton>
                  <LiquidGlassButton
                    type="button"
                    aria-label="重命名"
                    title="重命名"
                    onClick={() => {
                      setRenamingId(l.libraryUuid);
                      setRenameValue(l.name);
                    }}
                  >
                    <Pencil size={14} aria-hidden="true" />
                  </LiquidGlassButton>
                  <LiquidGlassButton
                    type="button"
                    disabled={l.active}
                    aria-label="删除库文件"
                    title={l.active ? "无法删除当前资源库" : "删除库文件（不可撤销）"}
                    onClick={() => {
                      setConfirmDeleteId((cur) => (cur === l.libraryUuid ? null : l.libraryUuid));
                      setDeleteConfirmText("");
                    }}
                    tone="danger"
                  >
                    <Trash2 size={14} aria-hidden="true" />
                  </LiquidGlassButton>
                </div>
              </div>
              {confirmDeleteId === l.libraryUuid ? (
                <div className="settings-library-delete-confirm">
                  <p>
                    永久删除库文件并从磁盘移除，此操作不可撤销。输入库名{" "}
                    <code>{l.name}</code> 以确认。
                  </p>
                  <div className="settings-library-delete-confirm-row">
                    <input
                      autoFocus
                      aria-label="输入库名以确认删除"
                      value={deleteConfirmText}
                      placeholder={l.name}
                      onChange={(event) => setDeleteConfirmText(event.target.value)}
                    />
                    <LiquidGlassButton
                      type="button"
                      onClick={() => {
                        setConfirmDeleteId(null);
                        setDeleteConfirmText("");
                      }}
                    >
                      取消
                    </LiquidGlassButton>
                    <LiquidGlassButton
                      type="button"
                      disabled={deleteConfirmText.trim() !== l.name}
                      onClick={() => void onDeleteFile(l)}
                      tone="danger"
                    >
                      删除
                    </LiquidGlassButton>
                  </div>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
        <div className="host-settings-library-footer">
          <span>{libs.length ? `已登记 ${libs.length} 个资源库` : "本机资源库"}</span>
          <LiquidGlassButton type="button" onClick={(event) => openManagedLibraryWizard(event.currentTarget)}>
            创建资源库…
          </LiquidGlassButton>
        </div>
      </SettingsGroup>
      <div className="host-settings-contributions"><PluginContributionsSlot slot="library-settings" /></div>
      <ManagedLibraryWizard
        onClose={closeManagedLibraryWizard}
        onCreated={reload}
        onFailure={(cause) => setError(cause instanceof Error ? cause.message : String(cause))}
        open={wizardOpen}
        restoreFocusRef={wizardRestoreFocusRef}
      />
    </>
  );
}

// ---- Diagnostics tab -------------------------------------------------------

function DiagnosticsTab({ library }: { library: LibraryState }) {
  const diagnostics = library.diagnostics;
  const probed = library.diagnosticsProbed;
  const ffmpegAvailable = diagnostics?.ffmpegAvailable;
  const dbPath = diagnostics?.dbPath ?? null;
  const pluginsDir = diagnostics?.pluginsDir ?? null;

  return (
    <>
      <SettingsGroup>
        <SettingRow title="FFmpeg" description="视频缩略图与媒体处理工具。">
          {ffmpegAvailable == null ? (
            <span className="settings-badge settings-badge-pending">{probed ? "未取得检测结果" : "检测中…"}</span>
          ) : ffmpegAvailable ? (
            <span className="settings-badge settings-badge-ok">可用</span>
          ) : (
            <span className="settings-badge settings-badge-missing" title="ffmpeg 不在 PATH 中">缺失</span>
          )}
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup className="host-settings-paths" title="本机路径">
        <SettingRow title="数据库"><PathDisplay probed={probed} value={dbPath} /></SettingRow>
        <SettingRow title="插件目录"><PathDisplay probed={probed} value={pluginsDir} /></SettingRow>
      </SettingsGroup>
    </>
  );
}

// ---- Shared controls -------------------------------------------------------

function ColorField({
  description,
  id,
  label,
  onChange,
  onPreview,
  value
}: {
  description?: string;
  id: string;
  label: string;
  onChange: (value: string) => void;
  onPreview: (value: string | null) => void;
  value: string;
}) {
  return (
    <SettingRow htmlFor={id} title={label} description={description}>
      <div className="host-settings-field no-drag">
        <ColorPicker id={id} label={label} value={value} onChange={onChange} onPreview={onPreview} showValue zh />
      </div>
    </SettingRow>
  );
}

function StyleSlider({
  description,
  disabled = false,
  id,
  label,
  max,
  min,
  onChange,
  resetValue,
  step,
  unit = "x",
  value
}: {
  description?: string;
  disabled?: boolean;
  id: string;
  label: string;
  max: number;
  min: number;
  onChange: (value: number) => void;
  resetValue: number;
  step: number;
  unit?: "%" | "MB" | "px" | "x";
  value: number;
}) {
  const numberInput = useRef<HTMLInputElement>(null);

  // Let the native editor keep partial decimals until the preference changes or editing ends.
  useLayoutEffect(() => {
    if (numberInput.current) numberInput.current.value = String(value);
  }, [value, disabled]);

  function commitNumber(input: HTMLInputElement) {
    const entered = input.valueAsNumber;
    if (disabled || !Number.isFinite(entered) || entered === value) {
      input.value = String(value);
      return;
    }

    // Use the slider's native rounding, including fractional steps and an off-step maximum.
    const range = document.createElement("input");
    range.type = "range";
    range.min = input.min;
    range.max = input.max;
    range.step = input.step;
    range.valueAsNumber = entered;
    const next = range.valueAsNumber;
    input.value = String(next);
    if (next !== value) onChange(next);
  }

  return (
    <div className="settings-control-row host-settings-parameter no-drag">
      <label className="settings-control-label" htmlFor={id}>{label}</label>
      <span className="settings-style-slider-control" data-settings-slider-control={id}>
        <ParameterSlider
          aria-describedby={description ? `${id}-description` : undefined}
          aria-valuetext={formatStyleValue(value, unit)}
          disabled={disabled}
          id={id}
          max={max}
          min={min}
          onChange={onChange}
          resetLabel={formatStyleValue(resetValue, unit)}
          resetValue={resetValue}
          step={step}
          value={value}
        />
      </span>
      <label className="host-settings-parameter-value" data-unit={unit} htmlFor={`${id}-value`}>
        <input
          aria-describedby={description ? `${id}-description` : undefined}
          aria-label={`${label}数值（${unit === "x" ? "倍" : unit}）`}
          defaultValue={value}
          disabled={disabled}
          id={`${id}-value`}
          max={max}
          min={min}
          onBlur={(event) => commitNumber(event.currentTarget)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing || (event.key !== "Enter" && event.key !== "Escape")) return;
            event.preventDefault();
            event.stopPropagation();
            if (event.key === "Enter") commitNumber(event.currentTarget);
            else event.currentTarget.value = String(value);
          }}
          ref={numberInput}
          step={step}
          type="number"
        />
        <span aria-hidden="true">{unit}</span>
      </label>
      {description && <p className="settings-control-description" id={`${id}-description`}>{description}</p>}
    </div>
  );
}

function PathDisplay({ value, probed }: { value: string | null; probed: boolean }) {
  if (!probed) {
    return <span className="settings-path-pending">检测中…</span>;
  }
  if (!value) {
    return <span className="settings-path-pending">未提供路径</span>;
  }
  return (
    <code className="settings-path" title={value}>
      {value}
    </code>
  );
}

function formatStyleValue(value: number, unit: "%" | "MB" | "px" | "x") {
  const rounded = Number.isInteger(value) ? String(value) : value.toFixed(2);
  if (unit === "%") return `${rounded}%`;
  if (unit === "MB") return `${rounded} MB`;
  return unit === "px" ? `${rounded}px` : `${rounded}x`;
}

function formatCacheBytes(value: number) {
  if (value <= 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  const rounded = amount >= 10 || unitIndex === 0 ? amount.toFixed(0) : amount.toFixed(1);
  return `${rounded} ${units[unitIndex]}`;
}

export type { SettingsPanelsProps };

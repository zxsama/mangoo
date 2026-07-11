import {
  CheckCircle2,
  Database,
  FolderCog,
  Gauge,
  Images,
  Keyboard,
  Palette,
  RefreshCw,
  Settings2,
  Trash2,
  XCircle
} from "lucide-react";
import { useEffect, useState, type KeyboardEvent, type ReactNode } from "react";
import type { LibraryState } from "../../core/useLibraryData";
import type { InterfaceStyleController } from "../../design/liquid-glass";
import {
  LIBRARY_GRID_PREFERENCE_LIMITS,
  type LibraryGridPreferences
} from "../media-grid/gridPreferences";
import {
  PREVIEW_PREFERENCE_LIMITS,
  type PreviewPreferences
} from "../preview/previewPreferences";
import {
  normalizeShortcutEvent,
  useShortcutBindings,
  type ShortcutActionId
} from "../shortcuts/shortcutBindings";

// Localized labels for the shortcut actions. The binding ids are stable; only
// the display copy is translated so the settings UI is fully in Chinese.
const SHORTCUT_ACTION_LABELS: Record<string, string> = {
  focusSearch: "聚焦搜索",
  renameSelected: "重命名所选文件",
  recycleDelete: "移到回收站",
  permanentDelete: "永久删除",
  closeOrReturn: "关闭 / 返回",
  previewNext: "下一张预览",
  previewPrevious: "上一张预览",
  zoomIn: "放大",
  zoomOut: "缩小",
  toggleSidebars: "切换侧栏"
};

interface SettingsPanelsProps {
  gridPreferences: LibraryGridPreferences;
  interfaceStyle: InterfaceStyleController;
  library: LibraryState;
  onGridPreferencesChange: (patch: Partial<LibraryGridPreferences>) => void;
  onPreviewPreferencesChange: (patch: Partial<PreviewPreferences>) => void;
  previewPreferences: PreviewPreferences;
}

type SettingsTabId = "general" | "thumbnails" | "appearance" | "shortcuts" | "diagnostics";

const TABS: Array<{ id: SettingsTabId; label: string; icon: typeof Settings2 }> = [
  { id: "general", label: "常用", icon: Settings2 },
  { id: "thumbnails", label: "缩略图", icon: Images },
  { id: "appearance", label: "外观", icon: Palette },
  { id: "shortcuts", label: "快捷键", icon: Keyboard },
  { id: "diagnostics", label: "诊断", icon: Gauge }
];

export function SettingsPanels({
  gridPreferences,
  interfaceStyle,
  library,
  onGridPreferencesChange,
  onPreviewPreferencesChange,
  previewPreferences
}: SettingsPanelsProps) {
  const [activeTab, setActiveTab] = useState<SettingsTabId>("general");

  return (
    <div className="settings-dialog-layout">
      <aside className="settings-dialog-nav" aria-label="设置分类">
        <div className="settings-dialog-nav-title" id="settings-dialog-title">
          偏好设置
        </div>
        <nav className="settings-dialog-nav-list" role="tablist" aria-orientation="vertical">
          {TABS.map((tab) => {
            const Icon = tab.icon;
            const active = tab.id === activeTab;
            return (
              <button
                key={tab.id}
                aria-selected={active}
                className={active ? "settings-dialog-nav-item active" : "settings-dialog-nav-item"}
                onClick={() => setActiveTab(tab.id)}
                role="tab"
                type="button"
              >
                <Icon aria-hidden="true" size={16} />
                <span>{tab.label}</span>
              </button>
            );
          })}
        </nav>
      </aside>
      <div className="settings-dialog-main">
        <div className="settings-dialog-content" role="tabpanel">
          {activeTab === "general" ? (
            <GeneralTab
              gridPreferences={gridPreferences}
              onGridPreferencesChange={onGridPreferencesChange}
              onPreviewPreferencesChange={onPreviewPreferencesChange}
              previewPreferences={previewPreferences}
            />
          ) : null}
          {activeTab === "thumbnails" ? (
            <ThumbnailsTab
              library={library}
              onPreviewPreferencesChange={onPreviewPreferencesChange}
              previewPreferences={previewPreferences}
            />
          ) : null}
          {activeTab === "appearance" ? <AppearanceTab interfaceStyle={interfaceStyle} /> : null}
          {activeTab === "shortcuts" ? <ShortcutsTab /> : null}
          {activeTab === "diagnostics" ? <DiagnosticsTab library={library} /> : null}
        </div>
      </div>
    </div>
  );
}

function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-group">
      <h3 className="settings-group-title">{title}</h3>
      <div className="settings-group-body">{children}</div>
    </section>
  );
}

// ---- General tab: browsing + grid spacing + memory buffers -----------------

function GeneralTab({
  gridPreferences,
  onGridPreferencesChange,
  onPreviewPreferencesChange,
  previewPreferences
}: {
  gridPreferences: LibraryGridPreferences;
  onGridPreferencesChange: (patch: Partial<LibraryGridPreferences>) => void;
  onPreviewPreferencesChange: (patch: Partial<PreviewPreferences>) => void;
  previewPreferences: PreviewPreferences;
}) {
  return (
    <>
      <SettingsGroup title="媒体网格">
        <StyleSlider
          id="library-grid-gap"
          label="缩略图间距"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.tileGap.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.tileGap.min}
          onChange={(tileGap) => onGridPreferencesChange({ tileGap })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.tileGap.step}
          unit="px"
          value={gridPreferences.tileGap}
        />
        <StyleSlider
          id="library-grid-label-height"
          label="名称行高"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.tileLabelHeight.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.tileLabelHeight.min}
          onChange={(tileLabelHeight) => onGridPreferencesChange({ tileLabelHeight })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.tileLabelHeight.step}
          unit="px"
          value={gridPreferences.tileLabelHeight}
        />
      </SettingsGroup>
      <SettingsGroup title="文件夹网格">
        <StyleSlider
          id="library-folder-grid-gap"
          label="文件夹缩略图间距"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.folderTileGap.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.folderTileGap.min}
          onChange={(folderTileGap) => onGridPreferencesChange({ folderTileGap })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.folderTileGap.step}
          unit="px"
          value={gridPreferences.folderTileGap}
        />
        <StyleSlider
          id="library-folder-grid-label-height"
          label="文件夹名称行高"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.folderTileLabelHeight.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.folderTileLabelHeight.min}
          onChange={(folderTileLabelHeight) => onGridPreferencesChange({ folderTileLabelHeight })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.folderTileLabelHeight.step}
          unit="px"
          value={gridPreferences.folderTileLabelHeight}
        />
        <StyleSlider
          id="library-folder-edge-shadow-alpha"
          label="文件夹边缘阴影"
          max={LIBRARY_GRID_PREFERENCE_LIMITS.folderEdgeShadowAlpha.max}
          min={LIBRARY_GRID_PREFERENCE_LIMITS.folderEdgeShadowAlpha.min}
          onChange={(folderEdgeShadowAlpha) => onGridPreferencesChange({ folderEdgeShadowAlpha })}
          step={LIBRARY_GRID_PREFERENCE_LIMITS.folderEdgeShadowAlpha.step}
          unit="%"
          value={gridPreferences.folderEdgeShadowAlpha}
        />
      </SettingsGroup>
      <SettingsGroup title="浏览缓存 (内存)">
        <p className="settings-note">
          控制浏览时 Megle 在内存中保留的原图与缩略图数据量。持久缩略图缓存存储在 SQLite
          中，可在重启后复用（见“缩略图”标签页）。
        </p>
        <StyleSlider
          id="preview-buffer-limit"
          label="预览缓冲区"
          max={PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.max}
          min={PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.min}
          onChange={(previewBufferLimitMb) => onPreviewPreferencesChange({ previewBufferLimitMb })}
          step={PREVIEW_PREFERENCE_LIMITS.previewBufferLimitMb.step}
          unit="MB"
          value={previewPreferences.previewBufferLimitMb}
        />
        <StyleSlider
          id="thumbnail-cache-limit"
          label="缩略图缓存 (内存)"
          max={PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.max}
          min={PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.min}
          onChange={(thumbnailCacheLimitMb) => onPreviewPreferencesChange({ thumbnailCacheLimitMb })}
          step={PREVIEW_PREFERENCE_LIMITS.thumbnailCacheLimitMb.step}
          unit="MB"
          value={previewPreferences.thumbnailCacheLimitMb}
        />
      </SettingsGroup>
    </>
  );
}

// ---- Thumbnails tab: live generation status + generation mode --------------

function ThumbnailsTab({
  library,
  onPreviewPreferencesChange,
  previewPreferences
}: {
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

  // The stats aggregate scans the whole library, so it must NOT be polled
  // aggressively — the numbers are a reference, not live. Poll on a fixed timer.
  //
  // CRITICAL: depend ONLY on the stable `refreshThumbnailCacheStats` callback and
  // the boolean `statsPollActive` — NOT on `library` (a fresh object every
  // render) or `stats` (which this effect's own polling updates). Depending on
  // those re-ran the effect on every render, and since each run kicks off an
  // immediate whole-library stats query, opening the thumbnail tab spun a
  // positive-feedback loop: poll -> setStats -> re-render -> effect re-runs ->
  // poll again. On a 650k-file library that piled up dozens of concurrent
  // multi-second whole-library scans, exhausting the blocking-thread pool
  // (thumbnails stopped refreshing) and ballooning Core memory.
  useEffect(() => {
    void refreshThumbnailCacheStats();
    const interval = window.setInterval(
      () => {
        void refreshThumbnailCacheStats();
      },
      statsPollActive ? 8000 : 20000
    );
    return () => window.clearInterval(interval);
  }, [refreshThumbnailCacheStats, statsPollActive]);

  // Progress estimate: cached / (cached + missing + stale). failed excluded from
  // the "done" fraction but shown separately.
  const cached = stats?.cachedCount ?? 0;
  const remaining = stats ? stats.missingCount + stats.staleCount : 0;
  const failed = stats?.failedCount ?? 0;
  const total = cached + remaining;
  const percent = total > 0 ? Math.min(100, Math.round((cached / total) * 100)) : stats ? 100 : 0;

  const generationMode = previewPreferences.thumbnailGenerationMode;
  const currentFolderDisabled = library.selectedFolderId === null || busy;

  return (
    <>
      <SettingsGroup title="生成模式">
        <p className="settings-note">
          默认仅将你实际看到并已生成的缩略图入库，后台写入压力最小；也可以让后台自动生成整个库的全部缩略图，或只在本次会话缓存而不写入数据库。
        </p>
        <RadioOption
          checked={generationMode === "renderedOnly"}
          description="只有已经在界面上生成并显示出来的缩略图才会入库（推荐，默认）。"
          label="仅已刷新缩略图入库"
          name="thumbnail-generate-mode"
          onSelect={() => onPreviewPreferencesChange({ thumbnailGenerationMode: "renderedOnly" })}
        />
        <RadioOption
          checked={generationMode === "allPersist"}
          description="打开库后在后台自动生成全部缩略图并入库（低优先级，浏览时让路）。"
          label="自动后台生成全部缩略图"
          name="thumbnail-generate-mode"
          onSelect={() => onPreviewPreferencesChange({ thumbnailGenerationMode: "allPersist" })}
        />
        <RadioOption
          checked={generationMode === "cacheOnly"}
          description="缩略图仅在本次会话内存缓存中显示，不写入数据库，库文件不会变大。"
          label="只缓存不入库"
          name="thumbnail-generate-mode"
          onSelect={() => onPreviewPreferencesChange({ thumbnailGenerationMode: "cacheOnly" })}
        />
      </SettingsGroup>

      <SettingsGroup title="生成状况">
        <div className="settings-progress">
          <div className="settings-progress-head">
            <span className="settings-progress-label">
              已入库 {cached.toLocaleString()} / {total.toLocaleString()}
            </span>
            <span className="settings-progress-percent">{percent}%</span>
          </div>
          <div
            aria-valuemax={100}
            aria-valuemin={0}
            aria-valuenow={percent}
            className="settings-progress-track"
            role="progressbar"
          >
            <div className="settings-progress-fill" style={{ width: `${percent}%` }} />
          </div>
          <div className="settings-progress-caption">
            {activeBulk > 0
              ? `正在后台生成 · ${activeBulk.toLocaleString()} 个任务进行中`
              : remaining > 0
                ? `待生成 ${remaining.toLocaleString()} 项`
                : stats
                  ? "全部缩略图已就绪"
                  : "读取中…"}
          </div>
        </div>
        <dl className="settings-stat-grid">
          <StatCell label="已缓存" value={stats ? cached.toLocaleString() : "-"} />
          <StatCell label="待生成 / 过期" value={stats ? remaining.toLocaleString() : "-"} />
          <StatCell label="失败" value={stats ? failed.toLocaleString() : "-"} />
          <StatCell label="活动任务" value={stats ? activeBulk.toLocaleString() : "-"} />
          <StatCell
            label="数据库占用"
            value={stats ? formatCacheBytes(stats.totalBlobBytes) : "-"}
          />
        </dl>
        <button
          className="settings-inline-action"
          disabled={library.thumbnailCacheStatsLoading}
          onClick={() => void library.refreshThumbnailCacheStats()}
          type="button"
        >
          <RefreshCw aria-hidden="true" size={13} />
          <span>{library.thumbnailCacheStatsLoading ? "刷新中…" : "刷新统计"}</span>
        </button>
      </SettingsGroup>

      <SettingsGroup title="手动生成">
        <div className="settings-action-row">
          <button
            className="settings-inline-action"
            disabled={currentFolderDisabled}
            onClick={() => void library.generateCurrentFolderThumbnailCache()}
            type="button"
          >
            生成当前文件夹
          </button>
          <button
            className="settings-inline-action"
            disabled={currentFolderDisabled}
            onClick={() => void library.generateCurrentTreeThumbnailCache()}
            type="button"
          >
            生成当前目录树
          </button>
          <button
            className="settings-inline-action"
            disabled={busy}
            onClick={() => void library.generateAllThumbnailCache()}
            type="button"
          >
            {busy ? "生成中…" : "生成全部缩略图"}
          </button>
          <button
            className="settings-inline-action"
            disabled={busy}
            onClick={() => void library.retryThumbnailCacheFailures()}
            type="button"
          >
            重试失败 / 过期
          </button>
          <button
            className="settings-inline-action settings-inline-action-danger"
            disabled={busy}
            onClick={() => void library.clearThumbnailCache()}
            type="button"
          >
            <Trash2 aria-hidden="true" size={13} />
            <span>清空持久缓存</span>
          </button>
        </div>
      </SettingsGroup>
    </>
  );
}

function RadioOption({
  checked,
  description,
  label,
  name,
  onSelect
}: {
  checked: boolean;
  description: string;
  label: string;
  name: string;
  onSelect: () => void;
}) {
  return (
    <label className={checked ? "settings-radio checked" : "settings-radio"}>
      <input checked={checked} name={name} onChange={onSelect} type="radio" />
      <span className="settings-radio-body">
        <span className="settings-radio-label">{label}</span>
        <span className="settings-radio-description">{description}</span>
      </span>
    </label>
  );
}

function StatCell({ label, value }: { label: string; value: string }) {
  return (
    <div className="settings-stat-cell">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

// ---- Appearance tab: liquid-glass interface style --------------------------

function AppearanceTab({ interfaceStyle }: { interfaceStyle: InterfaceStyleController }) {
  const { limits, value, resetInterfaceStyle, setInterfaceStyle } = interfaceStyle;
  return (
    <>
      <div className="settings-group-heading-row">
        <p className="settings-note">调整液态玻璃外观：圆角、材质模糊 / 透明度、颜色与交互高光。</p>
        <button className="settings-inline-action" onClick={resetInterfaceStyle} type="button">
          恢复默认外观
        </button>
      </div>
      <SettingsGroup title="圆角">
        <StyleSlider
          id="window-corner-radius"
          disabled
          label="窗口圆角"
          max={limits.windowCornerRadius.max}
          min={limits.windowCornerRadius.min}
          onChange={(windowCornerRadius) => setInterfaceStyle({ windowCornerRadius })}
          step={limits.windowCornerRadius.step}
          unit="px"
          value={value.windowCornerRadius}
        />
        <StyleSlider
          id="surface-corner-radius"
          label="面板圆角"
          max={limits.surfaceCornerRadius.max}
          min={limits.surfaceCornerRadius.min}
          onChange={(surfaceCornerRadius) => setInterfaceStyle({ surfaceCornerRadius })}
          step={limits.surfaceCornerRadius.step}
          unit="px"
          value={value.surfaceCornerRadius}
        />
        <StyleSlider
          id="control-corner-radius"
          label="控件圆角"
          max={limits.controlCornerRadius.max}
          min={limits.controlCornerRadius.min}
          onChange={(controlCornerRadius) => setInterfaceStyle({ controlCornerRadius })}
          step={limits.controlCornerRadius.step}
          unit="px"
          value={value.controlCornerRadius}
        />
        <StyleSlider
          id="content-corner-radius"
          label="内容圆角"
          max={limits.contentCornerRadius.max}
          min={limits.contentCornerRadius.min}
          onChange={(contentCornerRadius) => setInterfaceStyle({ contentCornerRadius })}
          step={limits.contentCornerRadius.step}
          unit="px"
          value={value.contentCornerRadius}
        />
      </SettingsGroup>
      <SettingsGroup title="侧栏材质">
        <StyleSlider id="side-blur" label="模糊" max={limits.sideBlur.max} min={limits.sideBlur.min} onChange={(sideBlur) => setInterfaceStyle({ sideBlur })} step={limits.sideBlur.step} value={value.sideBlur} />
        <StyleSlider id="side-opacity" label="不透明度" max={limits.sideOpacity.max} min={limits.sideOpacity.min} onChange={(sideOpacity) => setInterfaceStyle({ sideOpacity })} step={limits.sideOpacity.step} value={value.sideOpacity} />
        <StyleSlider id="side-overlay-strength" label="叠加强度" max={limits.sideOverlayStrength.max} min={limits.sideOverlayStrength.min} onChange={(sideOverlayStrength) => setInterfaceStyle({ sideOverlayStrength })} step={limits.sideOverlayStrength.step} value={value.sideOverlayStrength} />
        <ColorField id="side-overlay-color" label="叠加颜色" onChange={(sideOverlayColor) => setInterfaceStyle({ sideOverlayColor })} value={value.sideOverlayColor} />
        <StyleSlider id="side-saturation" label="饱和度" max={limits.sideSaturation.max} min={limits.sideSaturation.min} onChange={(sideSaturation) => setInterfaceStyle({ sideSaturation })} step={limits.sideSaturation.step} value={value.sideSaturation} />
        <StyleSlider id="side-stroke-opacity" label="描边不透明度" max={limits.sideStrokeOpacity.max} min={limits.sideStrokeOpacity.min} onChange={(sideStrokeOpacity) => setInterfaceStyle({ sideStrokeOpacity })} step={limits.sideStrokeOpacity.step} value={value.sideStrokeOpacity} />
      </SettingsGroup>
      <SettingsGroup title="中央工作区材质">
        <StyleSlider id="center-blur" label="模糊" max={limits.centerBlur.max} min={limits.centerBlur.min} onChange={(centerBlur) => setInterfaceStyle({ centerBlur })} step={limits.centerBlur.step} value={value.centerBlur} />
        <StyleSlider id="center-opacity" label="不透明度" max={limits.centerOpacity.max} min={limits.centerOpacity.min} onChange={(centerOpacity) => setInterfaceStyle({ centerOpacity })} step={limits.centerOpacity.step} value={value.centerOpacity} />
        <StyleSlider id="center-overlay-strength" label="叠加强度" max={limits.centerOverlayStrength.max} min={limits.centerOverlayStrength.min} onChange={(centerOverlayStrength) => setInterfaceStyle({ centerOverlayStrength })} step={limits.centerOverlayStrength.step} value={value.centerOverlayStrength} />
        <ColorField id="center-overlay-color" label="叠加颜色" onChange={(centerOverlayColor) => setInterfaceStyle({ centerOverlayColor })} value={value.centerOverlayColor} />
        <StyleSlider id="center-saturation" label="饱和度" max={limits.centerSaturation.max} min={limits.centerSaturation.min} onChange={(centerSaturation) => setInterfaceStyle({ centerSaturation })} step={limits.centerSaturation.step} value={value.centerSaturation} />
        <StyleSlider id="center-stroke-opacity" label="描边不透明度" max={limits.centerStrokeOpacity.max} min={limits.centerStrokeOpacity.min} onChange={(centerStrokeOpacity) => setInterfaceStyle({ centerStrokeOpacity })} step={limits.centerStrokeOpacity.step} value={value.centerStrokeOpacity} />
      </SettingsGroup>
      <SettingsGroup title="玻璃平滑度">
        <StyleSlider id="dither-opacity" label="抖动强度" max={limits.ditherOpacity.max} min={limits.ditherOpacity.min} onChange={(ditherOpacity) => setInterfaceStyle({ ditherOpacity })} step={limits.ditherOpacity.step} value={value.ditherOpacity} />
        <StyleSlider id="backdrop-gradient-strength" label="背景渐变强度" max={limits.backdropGradientStrength.max} min={limits.backdropGradientStrength.min} onChange={(backdropGradientStrength) => setInterfaceStyle({ backdropGradientStrength })} step={limits.backdropGradientStrength.step} value={value.backdropGradientStrength} />
      </SettingsGroup>
      <SettingsGroup title="交互高光">
        <StyleSlider id="edge-highlight-brightness" label="边缘高光亮度" max={limits.edgeHighlightBrightness.max} min={limits.edgeHighlightBrightness.min} onChange={(edgeHighlightBrightness) => setInterfaceStyle({ edgeHighlightBrightness })} step={limits.edgeHighlightBrightness.step} value={value.edgeHighlightBrightness} />
        <StyleSlider id="halo-brightness" label="光晕亮度" max={limits.haloBrightness.max} min={limits.haloBrightness.min} onChange={(haloBrightness) => setInterfaceStyle({ haloBrightness })} step={limits.haloBrightness.step} value={value.haloBrightness} />
        <StyleSlider id="pointer-response-radius" label="指针响应半径" max={limits.pointerResponseRadius.max} min={limits.pointerResponseRadius.min} onChange={(pointerResponseRadius) => setInterfaceStyle({ pointerResponseRadius })} step={limits.pointerResponseRadius.step} value={value.pointerResponseRadius} />
        <StyleSlider id="pointer-sensing-distance" label="感应距离" max={limits.pointerSensingDistance.max} min={limits.pointerSensingDistance.min} onChange={(pointerSensingDistance) => setInterfaceStyle({ pointerSensingDistance })} step={limits.pointerSensingDistance.step} value={value.pointerSensingDistance} />
        <StyleSlider id="pointer-falloff" label="衰减曲线" max={limits.pointerFalloff.max} min={limits.pointerFalloff.min} onChange={(pointerFalloff) => setInterfaceStyle({ pointerFalloff })} step={limits.pointerFalloff.step} value={value.pointerFalloff} />
      </SettingsGroup>
      <SettingsGroup title="对话框材质">
        <StyleSlider id="dialog-blur" label="对话框模糊" max={limits.dialogBlur.max} min={limits.dialogBlur.min} onChange={(dialogBlur) => setInterfaceStyle({ dialogBlur })} step={limits.dialogBlur.step} value={value.dialogBlur} />
        <StyleSlider id="dialog-opacity" label="对话框不透明度" max={limits.dialogOpacity.max} min={limits.dialogOpacity.min} onChange={(dialogOpacity) => setInterfaceStyle({ dialogOpacity })} step={limits.dialogOpacity.step} value={value.dialogOpacity} />
        <StyleSlider id="dialog-overlay-strength" label="对话框叠加强度" max={limits.dialogOverlayStrength.max} min={limits.dialogOverlayStrength.min} onChange={(dialogOverlayStrength) => setInterfaceStyle({ dialogOverlayStrength })} step={limits.dialogOverlayStrength.step} value={value.dialogOverlayStrength} />
        <StyleSlider id="dialog-backdrop-dim" label="对话框背景变暗" max={limits.dialogBackdropDim.max} min={limits.dialogBackdropDim.min} onChange={(dialogBackdropDim) => setInterfaceStyle({ dialogBackdropDim })} step={limits.dialogBackdropDim.step} value={value.dialogBackdropDim} />
      </SettingsGroup>
    </>
  );
}

// ---- Shortcuts tab ---------------------------------------------------------

function ShortcutsTab() {
  const { actions, bindings, conflicts, onReset, setBinding } = useShortcutBindings();
  const [capturing, setCapturing] = useState<ShortcutActionId | null>(null);
  const labelFor = (id: string) => SHORTCUT_ACTION_LABELS[id] ?? id;

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

  return (
    <>
      <div className="settings-group-heading-row">
        <p className="settings-note">点击右侧按钮后按下新的组合键即可重新绑定。</p>
        <button className="settings-inline-action" onClick={onReset} type="button">
          恢复默认快捷键
        </button>
      </div>
      <div className="shortcut-bindings-list">
        {actions.map((action) => {
          const actionConflicts = conflicts[action.id] ?? [];
          return (
            <div className="shortcut-binding-row" key={action.id}>
              <div className="shortcut-binding-label">{labelFor(action.id)}</div>
              <button
                className={
                  capturing === action.id
                    ? "shortcut-binding-capture active"
                    : "shortcut-binding-capture"
                }
                data-shortcut-capture="true"
                onBlur={() => {
                  if (capturing === action.id) setCapturing(null);
                }}
                onClick={() => setCapturing(action.id)}
                onKeyDown={(event) => handleCaptureKeyDown(event, action.id)}
                type="button"
              >
                {capturing === action.id ? "请按下快捷键" : bindings[action.id]}
              </button>
              {actionConflicts.length > 0 ? (
                <div className="shortcut-binding-conflict">
                  与 {actionConflicts.map((id) => labelFor(id)).join("、")} 冲突
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
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
    <SettingsGroup title="诊断信息">
      <dl className="settings-diagnostics-grid">
        <div className="settings-diagnostics-row">
          <dt>
            <span className="settings-diagnostics-icon" aria-hidden="true">
              {ffmpegAvailable ? <CheckCircle2 size={16} /> : <XCircle size={16} />}
            </span>
            FFmpeg
          </dt>
          <dd>
            {!probed ? (
              <span className="settings-badge settings-badge-pending">检测中…</span>
            ) : ffmpegAvailable ? (
              <span className="settings-badge settings-badge-ok">可用</span>
            ) : (
              <span className="settings-badge settings-badge-missing" title="ffmpeg 不在 PATH 中">
                缺失
              </span>
            )}
          </dd>
        </div>
        <div className="settings-diagnostics-row">
          <dt>
            <span className="settings-diagnostics-icon" aria-hidden="true">
              <Database size={16} />
            </span>
            数据库
          </dt>
          <dd>
            <PathDisplay probed={probed} value={dbPath} />
          </dd>
        </div>
        <div className="settings-diagnostics-row">
          <dt>
            <span className="settings-diagnostics-icon" aria-hidden="true">
              <FolderCog size={16} />
            </span>
            插件目录
          </dt>
          <dd>
            <PathDisplay probed={probed} value={pluginsDir} />
          </dd>
        </div>
      </dl>
    </SettingsGroup>
  );
}

// ---- Shared controls -------------------------------------------------------

function ColorField({
  id,
  label,
  onChange,
  value
}: {
  id: string;
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <label className="settings-style-slider settings-style-color no-drag" htmlFor={id}>
      <span>{label}</span>
      <input
        id={id}
        onChange={(event) => onChange(event.currentTarget.value)}
        type="color"
        value={value}
      />
      <output htmlFor={id}>{value.toUpperCase()}</output>
    </label>
  );
}

function StyleSlider({
  disabled = false,
  id,
  label,
  max,
  min,
  onChange,
  step,
  unit = "x",
  value
}: {
  disabled?: boolean;
  id: string;
  label: string;
  max: number;
  min: number;
  onChange: (value: number) => void;
  step: number;
  unit?: "%" | "MB" | "px" | "x";
  value: number;
}) {
  return (
    <label className="settings-style-slider no-drag" htmlFor={id}>
      <span>{label}</span>
      <span className="settings-style-slider-control" data-settings-slider-control={id}>
        <input
          id={id}
          disabled={disabled}
          max={max}
          min={min}
          onChange={(event) => onChange(Number(event.currentTarget.value))}
          step={step}
          type="range"
          value={value}
        />
      </span>
      <output htmlFor={id}>{formatStyleValue(value, unit)}</output>
    </label>
  );
}

function PathDisplay({ value, probed }: { value: string | null; probed: boolean }) {
  if (!probed) {
    return <span className="settings-path-pending">检测中…</span>;
  }
  if (!value) {
    return <span className="settings-path-pending">在桌面应用中运行以查看</span>;
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

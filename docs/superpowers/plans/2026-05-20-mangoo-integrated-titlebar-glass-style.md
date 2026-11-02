# Mangoo Integrated Titlebar Glass Style Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Mangoo 桌面 UI 从旧的全宽 `ShellTopBar` 改为三列融合标题栏，并补齐本机玻璃样式偏好、预览交互和验证契约。

**Architecture:** `AppShell` 只拥有三列标题栏 slot、三栏布局和 overlay host 挂载点；业务状态仍由 `App.tsx` 与 feature view 拥有，通过显式 props 把 Library/Preview 工具放入中间标题栏。Liquid Glass 偏好以 renderer `localStorage` 存储并挂载到 `document.documentElement` CSS variables，不写入 Core 或数据库。

**Tech Stack:** React + TypeScript + Vite, Electron frameless window chrome, localStorage, CSS custom properties, existing Liquid Glass primitives, Lucide icons, Node-based static/visual checks.

---

## File Structure / 文件边界

- Create: `apps/web/src/features/settings/interfaceStyle.ts`
  - 新增本机 UI 样式偏好模块：类型、默认值、存储键 `mangoo.interfaceStyle`、读写/归一化、CSS variable 映射、React hook。
  - 该文件不调用 Core API，不导入 Electron bridge。
- Modify: `apps/web/src/app/App.tsx`
  - 挂载 `useInterfaceStyle()`，把 CSS variables 应用到 root。
  - 继续拥有 `activeView`、`previewOpen`、Tasks/Recent overlay 状态和 Library 数据。
  - 组装 `AppShell` 的 `titlebarLeft`、`titlebarCenter`、`titlebarRight` slots。
- Modify: `apps/web/src/app-shell/AppShell.tsx`
  - 从 `topbar` prop 改成三列 titlebar slots。
  - 只渲染 shell 布局、titlebar surfaces、sidebar/workspace/overlay slots。
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
  - 替换旧全宽 `ShellTopBar` 组件；保留 `ShellWorkspaceView` 类型。
  - 导出左侧主导航、中间 Library toolbar、中间 Preview toolbar、右侧 Tasks/Recent/WindowChrome 组件。
  - 不再渲染品牌标题块，不再渲染单条 `className="topbar"` 全宽 header。
- Modify: `apps/web/src/features/library/LibraryView.tsx`
  - 保留 Library 数据流、空状态、grid、中央预览、右侧 inspector。
  - 移除内容区 `library-content-toolbar` 的搜索/筛选/排序/刷新/返回控件。
  - 接收 `onPreviewPrevious` / `onPreviewNext` props，供中央预览使用；导航算法由 `App.tsx` 统一拥有。
- Modify: `apps/web/src/features/preview/CentralPreviewStage.tsx`
  - 收紧长边填充、双击 100%/长边填充、`Ctrl + wheel` 缩放、普通滚轮翻页、拖拽平移、透明无边框契约。
  - 通过 `onViewStateChange` 把 view mode / scale 状态回传给中间标题栏。
- Modify: `apps/web/src/features/preview/PreviewPanel.tsx`
  - 保证右侧横图/竖图/方图均 `contain`、透明、无黑底和描边。
- Modify: `apps/web/src/features/settings/SettingsView.tsx`
  - 新增 `Interface style` section：Glass blur、Pointer glow brightness、Edge highlight brightness、Reset interface style。
  - 接收 `interfaceStyle` controller props，不自行读写 localStorage。
- Modify: `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
  - 继续提供 Liquid Glass material layers 和全局 pointer tracking。
  - 给 buttons 增加 `data-no-drag="true"` 和默认 `no-drag` class；surface 不默认 no-drag，方便 titlebar 空白区拖动。
- Modify: `apps/web/src/design/liquid-glass/index.ts`
  - 导出 `useInterfaceStyle`、默认值和类型。
- Modify: `apps/web/src/styles.css`
  - 新增三列 titlebar 布局、drag/no-drag、settings sliders、glass brightness variables、preview contain/transparent 规则。
  - 移除旧 `.topbar` 全宽布局依赖。
- Modify: `tools/checks/validate-ui-design.mjs`
  - 更新静态 UI 契约到三列融合标题栏、localStorage 偏好、blur 层级、preview/contain、drag/no-drag。
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`
  - 如果文件存在，更新 visual harness 到三列标题栏、Interface style、drag/no-drag、预览和局部边缘高亮断言。
  - 如果文件不存在，最终验证跳过该命令并在执行报告里说明。

执行纪律：当前工作树很脏，且可能有 Claude 并行改动。每个任务开始前运行 `git status --short` 只用于观察；只改本任务 `Files` 列出的文件，不回滚、不格式化、不覆盖无关改动。同一时间只运行一个实现子代理，按下面任务顺序串行推进。

## Task 1: Interface Style Preference 与 Settings 区域

**Files:**
- Create: `apps/web/src/features/settings/interfaceStyle.ts`
- Modify: `apps/web/src/design/liquid-glass/index.ts`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/features/settings/SettingsView.tsx`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: 创建偏好模块**

在 `apps/web/src/features/settings/interfaceStyle.ts` 写入以下接口和函数。范围值采用明确产品决策：blur `0..2`，pointer glow `0..2`，edge highlight `0..8`，默认 edge 为 `5`。

```ts
import { useCallback, useEffect, useMemo, useState } from "react";

export const INTERFACE_STYLE_STORAGE_KEY = "mangoo.interfaceStyle";

export interface InterfaceStylePreference {
  glassBlur: number;
  pointerGlowBrightness: number;
  edgeHighlightBrightness: number;
}

export const DEFAULT_INTERFACE_STYLE: InterfaceStylePreference = {
  glassBlur: 1,
  pointerGlowBrightness: 1,
  edgeHighlightBrightness: 5
};

export const INTERFACE_STYLE_LIMITS = {
  glassBlur: { min: 0, max: 2, step: 0.05 },
  pointerGlowBrightness: { min: 0, max: 2, step: 0.05 },
  edgeHighlightBrightness: { min: 0, max: 8, step: 0.25 }
} as const;

export type InterfaceStylePatch = Partial<InterfaceStylePreference>;

export interface InterfaceStyleController {
  value: InterfaceStylePreference;
  limits: typeof INTERFACE_STYLE_LIMITS;
  setInterfaceStyle: (patch: InterfaceStylePatch) => void;
  resetInterfaceStyle: () => void;
}

export function normalizeInterfaceStyle(input: unknown): InterfaceStylePreference {
  const source = isRecord(input) ? input : {};
  return {
    glassBlur: clampNumber(source.glassBlur, INTERFACE_STYLE_LIMITS.glassBlur, DEFAULT_INTERFACE_STYLE.glassBlur),
    pointerGlowBrightness: clampNumber(source.pointerGlowBrightness, INTERFACE_STYLE_LIMITS.pointerGlowBrightness, DEFAULT_INTERFACE_STYLE.pointerGlowBrightness),
    edgeHighlightBrightness: clampNumber(source.edgeHighlightBrightness, INTERFACE_STYLE_LIMITS.edgeHighlightBrightness, DEFAULT_INTERFACE_STYLE.edgeHighlightBrightness)
  };
}

export function readInterfaceStyle(storage: Storage | undefined = window.localStorage) {
  if (!storage) return DEFAULT_INTERFACE_STYLE;
  try {
    const raw = storage.getItem(INTERFACE_STYLE_STORAGE_KEY);
    return raw ? normalizeInterfaceStyle(JSON.parse(raw)) : DEFAULT_INTERFACE_STYLE;
  } catch {
    return DEFAULT_INTERFACE_STYLE;
  }
}

export function writeInterfaceStyle(value: InterfaceStylePreference, storage: Storage | undefined = window.localStorage) {
  if (!storage) return;
  storage.setItem(INTERFACE_STYLE_STORAGE_KEY, JSON.stringify(normalizeInterfaceStyle(value)));
}

export function interfaceStyleToCssVariables(value: InterfaceStylePreference): Record<string, string> {
  const normalized = normalizeInterfaceStyle(value);
  return {
    "--glass-blur": `${roundCssNumber(26 * normalized.glassBlur)}px`,
    "--glass-elevated-blur": `${roundCssNumber(34 * normalized.glassBlur)}px`,
    "--glass-control-blur": `${roundCssNumber(18 * normalized.glassBlur)}px`,
    "--glass-pointer-glow-brightness": String(roundCssNumber(normalized.pointerGlowBrightness)),
    "--glass-edge-highlight-brightness": String(roundCssNumber(normalized.edgeHighlightBrightness))
  };
}

export function applyInterfaceStyleVariables(value: InterfaceStylePreference, target: HTMLElement = document.documentElement) {
  const variables = interfaceStyleToCssVariables(value);
  for (const [name, cssValue] of Object.entries(variables)) {
    target.style.setProperty(name, cssValue);
  }
}

export function useInterfaceStyle(): InterfaceStyleController {
  const [value, setValue] = useState<InterfaceStylePreference>(() => readInterfaceStyle());

  useEffect(() => {
    applyInterfaceStyleVariables(value);
    writeInterfaceStyle(value);
  }, [value]);

  const setInterfaceStyle = useCallback((patch: InterfaceStylePatch) => {
    setValue((current) => normalizeInterfaceStyle({ ...current, ...patch }));
  }, []);

  const resetInterfaceStyle = useCallback(() => {
    setValue(DEFAULT_INTERFACE_STYLE);
  }, []);

  return useMemo(
    () => ({ value, limits: INTERFACE_STYLE_LIMITS, setInterfaceStyle, resetInterfaceStyle }),
    [resetInterfaceStyle, setInterfaceStyle, value]
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function clampNumber(value: unknown, limits: { min: number; max: number }, fallback: number) {
  const number = typeof value === "number" && Number.isFinite(value) ? value : fallback;
  return Math.min(limits.max, Math.max(limits.min, number));
}

function roundCssNumber(value: number) {
  return Math.round(value * 1000) / 1000;
}
```

- [ ] **Step 2: 从 Liquid Glass index 导出偏好 API**

在 `apps/web/src/design/liquid-glass/index.ts` 添加：

```ts
export {
  DEFAULT_INTERFACE_STYLE,
  INTERFACE_STYLE_LIMITS,
  INTERFACE_STYLE_STORAGE_KEY,
  applyInterfaceStyleVariables,
  interfaceStyleToCssVariables,
  normalizeInterfaceStyle,
  readInterfaceStyle,
  useInterfaceStyle,
  writeInterfaceStyle,
  type InterfaceStyleController,
  type InterfaceStylePreference
} from "../../features/settings/interfaceStyle";
```

- [ ] **Step 3: 在 App 挂载偏好并传给 Settings**

在 `apps/web/src/app/App.tsx` 中从 `../design/liquid-glass` 导入 `useInterfaceStyle`，在 `App()` 顶部创建 controller：

```tsx
const interfaceStyle = useInterfaceStyle();
```

渲染 Settings 时改为：

```tsx
<SettingsView interfaceStyle={interfaceStyle} library={library} />
```

- [ ] **Step 4: 在 SettingsView 增加 Interface style section**

更新 `SettingsViewProps`：

```ts
import type { InterfaceStyleController } from "../../design/liquid-glass";

interface SettingsViewProps {
  interfaceStyle: InterfaceStyleController;
  library: LibraryState;
}
```

在 Diagnostics 与 Thumbnail cache 之间插入：

```tsx
<InterfaceStyleSection interfaceStyle={interfaceStyle} />
```

新增组件：

```tsx
function InterfaceStyleSection({ interfaceStyle }: { interfaceStyle: InterfaceStyleController }) {
  const { limits, value, resetInterfaceStyle, setInterfaceStyle } = interfaceStyle;
  return (
    <LiquidGlassSurface
      as="section"
      className="settings-section settings-interface-style"
      aria-labelledby="settings-interface-style-title"
      interactive
      scrollable
      tone="panel"
    >
      <div className="settings-section-heading">
        <h2 className="settings-section-title" id="settings-interface-style-title">
          Interface style
        </h2>
        <button className="settings-action no-drag" onClick={resetInterfaceStyle} type="button">
          Reset interface style
        </button>
      </div>
      <StyleSlider
        id="glass-blur"
        label="Glass blur"
        max={limits.glassBlur.max}
        min={limits.glassBlur.min}
        onChange={(glassBlur) => setInterfaceStyle({ glassBlur })}
        step={limits.glassBlur.step}
        value={value.glassBlur}
      />
      <StyleSlider
        id="pointer-glow-brightness"
        label="Pointer glow brightness"
        max={limits.pointerGlowBrightness.max}
        min={limits.pointerGlowBrightness.min}
        onChange={(pointerGlowBrightness) => setInterfaceStyle({ pointerGlowBrightness })}
        step={limits.pointerGlowBrightness.step}
        value={value.pointerGlowBrightness}
      />
      <StyleSlider
        id="edge-highlight-brightness"
        label="Edge highlight brightness"
        max={limits.edgeHighlightBrightness.max}
        min={limits.edgeHighlightBrightness.min}
        onChange={(edgeHighlightBrightness) => setInterfaceStyle({ edgeHighlightBrightness })}
        step={limits.edgeHighlightBrightness.step}
        value={value.edgeHighlightBrightness}
      />
    </LiquidGlassSurface>
  );
}

function StyleSlider({
  id,
  label,
  max,
  min,
  onChange,
  step,
  value
}: {
  id: string;
  label: string;
  max: number;
  min: number;
  onChange: (value: number) => void;
  step: number;
  value: number;
}) {
  return (
    <label className="settings-style-slider no-drag" htmlFor={id}>
      <span>{label}</span>
      <input
        id={id}
        max={max}
        min={min}
        onChange={(event) => onChange(Number(event.currentTarget.value))}
        step={step}
        type="range"
        value={value}
      />
      <output htmlFor={id}>{formatStyleValue(value)}</output>
    </label>
  );
}

function formatStyleValue(value: number) {
  return Number.isInteger(value) ? String(value) : value.toFixed(2);
}
```

- [ ] **Step 5: 增加 Settings slider CSS 和默认变量**

在 `:root` 确保存在：

```css
--glass-pointer-glow-brightness: 1;
--glass-edge-highlight-brightness: 5;
```

在 settings CSS 区块添加：

```css
.settings-interface-style {
  gap: 12px;
}

.settings-style-slider {
  display: grid;
  grid-template-columns: minmax(150px, 1fr) minmax(180px, 260px) 48px;
  gap: 12px;
  align-items: center;
  color: var(--text-soft);
  font-size: 12.5px;
}

.settings-style-slider input[type="range"] {
  width: 100%;
  accent-color: var(--accent);
}

.settings-style-slider output {
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
  text-align: right;
}
```

- [ ] **Step 6: 任务级命令**

Run: `rg -n "mangoo.interfaceStyle|Interface style|--glass-pointer-glow-brightness|--glass-edge-highlight-brightness|settings-style-slider" apps/web/src`

Expected: 命中 `interfaceStyle.ts`、`index.ts`、`App.tsx`、`SettingsView.tsx`、`styles.css`；没有 Core、desktop、database 文件命中。

## Task 2: 三列融合标题栏 Shell

**Files:**
- Modify: `apps/web/src/app-shell/AppShell.tsx`
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: 改 AppShell props 与布局**

将 `AppShellProps` 改为：

```tsx
interface AppShellProps {
  titlebarLeft: ReactNode;
  titlebarCenter: ReactNode;
  titlebarRight: ReactNode;
  sidebar: ReactNode;
  workspace: ReactNode;
  overlays: ReactNode;
}
```

渲染结构改为：

```tsx
import { LiquidGlassSurface } from "../design/liquid-glass";

export function AppShell({
  titlebarLeft,
  titlebarCenter,
  titlebarRight,
  sidebar,
  workspace,
  overlays
}: AppShellProps) {
  return (
    <main className="app-shell">
      <LiquidGlassSurface
        as="section"
        className="shell-titlebar shell-titlebar-left shell-drag"
        aria-label="Primary navigation"
        interactive
        tone="chrome"
      >
        {titlebarLeft}
      </LiquidGlassSurface>
      <LiquidGlassSurface
        as="section"
        className="shell-titlebar shell-titlebar-center shell-drag"
        aria-label="Workspace toolbar"
        interactive
        tone="chrome"
      >
        {titlebarCenter}
      </LiquidGlassSurface>
      <LiquidGlassSurface
        as="section"
        className="shell-titlebar shell-titlebar-right shell-drag"
        aria-label="Window actions"
        interactive
        tone="chrome"
      >
        {titlebarRight}
      </LiquidGlassSurface>
      {sidebar}
      <section className="app-workspace-slot">{workspace}</section>
      {overlays}
    </main>
  );
}
```

说明：`AppShell` 不导入 Library、Tasks、Recent 或 media preview 业务组件。

- [ ] **Step 2: 替换 ShellTopBar 的职责**

在 `ShellTopBar.tsx` 删除旧 `ShellTopBar` 全宽组件实现，保留 `ShellWorkspaceView` 类型并导出这些组件：

```tsx
export type ShellWorkspaceView = "library" | "plugins" | "settings";

export function ShellPrimaryNav({
  activeView,
  onSelectView
}: {
  activeView: ShellWorkspaceView;
  onSelectView: (view: ShellWorkspaceView) => void;
}) {
  return (
    <nav className="shell-primary-nav no-drag" aria-label="Workbench sections" role="tablist">
      {tabs.map((tab) => {
        const Icon = tab.icon;
        return (
          <LiquidGlassButton
            active={activeView === tab.id}
            aria-current={activeView === tab.id ? "page" : undefined}
            aria-label={tab.label}
            aria-selected={activeView === tab.id}
            className={activeView === tab.id ? "shell-nav-button active no-drag" : "shell-nav-button no-drag"}
            key={tab.id}
            onClick={() => onSelectView(tab.id)}
            role="tab"
            title={tab.label}
            tone="control"
            type="button"
          >
            <Icon size={17} />
            <span className="shell-nav-caption">{tab.caption}</span>
          </LiquidGlassButton>
        );
      })}
    </nav>
  );
}
```

右侧组件：

```tsx
export function ShellRightActions({
  recentOpsOpen,
  scanActive,
  taskPaletteOpen,
  onCloseTasks,
  onOpenTasks,
  onToggleRecent
}: ShellRightActionsProps) {
  return (
    <div className="shell-right-actions no-drag">
      <LiquidGlassButton
        active={taskPaletteOpen || scanActive}
        aria-label={taskPaletteOpen ? "Close tasks palette" : "Open tasks palette"}
        aria-pressed={taskPaletteOpen}
        className={taskPaletteOpen || scanActive ? "top-action task-drawer-toggle active no-drag" : "top-action task-drawer-toggle no-drag"}
        onClick={taskPaletteOpen ? onCloseTasks : onOpenTasks}
        title={taskPaletteOpen ? "Close tasks palette" : "Open tasks palette"}
        tone="control"
        type="button"
      >
        <ListChecks size={16} />
        <span className="top-action-label">Tasks</span>
        {scanActive ? <span className="task-drawer-status" aria-hidden="true" /> : null}
      </LiquidGlassButton>
      <LiquidGlassButton
        active={recentOpsOpen}
        aria-label="Toggle recent file operations"
        aria-pressed={recentOpsOpen}
        className={`top-action recent-ops-toggle no-drag${recentOpsOpen ? " active" : ""}`}
        onClick={onToggleRecent}
        title="Recent file operations"
        tone="control"
        type="button"
      >
        <History size={16} />
        <span className="top-action-label">Recent</span>
      </LiquidGlassButton>
      <WindowChrome />
    </div>
  );
}
```

不要保留 `chrome-title-block`、`chrome-title`、`chrome-subtitle`。

- [ ] **Step 3: 在 App 中组装三个 titlebar slot**

更新 imports：

```tsx
import {
  LibraryTitlebarToolbar,
  PreviewTitlebarToolbar,
  ShellPrimaryNav,
  ShellRightActions,
  type ShellWorkspaceView
} from "../app-shell/ShellTopBar";
```

传入 `AppShell`：

```tsx
<AppShell
  titlebarLeft={
    <ShellPrimaryNav activeView={activeView} onSelectView={setActiveView} />
  }
  titlebarCenter={renderCenterTitlebar()}
  titlebarRight={
    <ShellRightActions
      recentOpsOpen={recentOpsOpen}
      scanActive={library.scanActive}
      taskPaletteOpen={taskDrawerOpen}
      onCloseTasks={closeTaskPalette}
      onOpenTasks={openTaskPalette}
      onToggleRecent={onToggleRecent}
    />
  }
  sidebar={renderSidebar()}
  workspace={renderWorkspace()}
  overlays={renderOverlays()}
/>
```

`renderSidebar()`、`renderWorkspace()`、`renderOverlays()` 使用当前 `App.tsx` 已有 JSX 和 props 原样拆成 helper，不改变 `LibrarySidebar`、`LibraryView`、`ShellOverlayHost` 的数据来源。

`renderCenterTitlebar()` 先返回空拖拽区域，Task 3 再接入 Library/Preview 工具：

```tsx
function renderCenterTitlebar() {
  if (activeView === "library") {
    return <div className="shell-titlebar-placeholder" aria-hidden="true" />;
  }
  if (activeView === "plugins") {
    return <div className="shell-titlebar-summary">Plugins</div>;
  }
  return <div className="shell-titlebar-summary">Settings</div>;
}
```

- [ ] **Step 4: 写三列基础 CSS**

替换 `.app-shell` grid：

```css
.app-shell {
  --shell-left-width: minmax(260px, 292px);
  --shell-right-width: 270px;
  --shell-titlebar-height: 58px;
  display: grid;
  position: relative;
  grid-template-areas:
    "titlebar-left titlebar-center titlebar-right"
    "sidebar workspace workspace";
  grid-template-columns: var(--shell-left-width) minmax(0, 1fr) var(--shell-right-width);
  grid-template-rows: var(--shell-titlebar-height) minmax(0, 1fr);
  width: 100vw;
  height: 100vh;
  margin: 0;
  min-width: 0;
  border: 0;
  border-radius: var(--radius-window);
  background: transparent;
  box-shadow: none;
  color: #f4f7f9;
  overflow: hidden;
}
```

新增：

```css
.shell-titlebar {
  display: flex;
  align-items: center;
  min-width: 0;
  min-height: 0;
  padding: 0 12px;
  background: transparent;
  border-bottom: 1px solid var(--line);
  box-shadow: var(--glass-shadow-tight);
}

.shell-titlebar-left {
  grid-area: titlebar-left;
  border-right: 1px solid var(--line);
  border-radius: var(--radius-window) 0 0 0;
}

.shell-titlebar-center {
  grid-area: titlebar-center;
  justify-content: space-between;
}

.shell-titlebar-right {
  grid-area: titlebar-right;
  justify-content: flex-end;
  border-left: 1px solid var(--line);
  border-radius: 0 var(--radius-window) 0 0;
}

.app-workspace-slot {
  display: contents;
}

.library-sidebar {
  grid-area: sidebar;
  border-radius: 0 0 0 var(--radius-window);
}

.workspace {
  grid-area: workspace;
  grid-template-columns: minmax(0, 1fr) var(--shell-right-width);
}
```

删除或停止使用旧 `.topbar`、`.top-tabs`、`.top-tab` 布局类；如果保留用于迁移，不能再有 JSX 引用。

- [ ] **Step 5: 窄屏布局保持可用**

在现有 `@media (max-width: 720px)` 中更新为三列标题栏压缩版：

```css
@media (max-width: 720px) {
  .app-shell {
    grid-template-areas:
      "titlebar-left titlebar-center titlebar-right"
      "workspace workspace workspace";
    grid-template-columns: auto minmax(0, 1fr) auto;
    grid-template-rows: var(--shell-titlebar-height) minmax(0, 1fr);
  }

  .shell-titlebar {
    min-width: 0;
    padding: 0 8px;
  }

  .shell-titlebar-center {
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: none;
  }

  .shell-titlebar-center::-webkit-scrollbar {
    display: none;
  }

  .shell-nav-caption,
  .top-action-label {
    display: none;
  }

  .library-sidebar,
  .inspector-panel {
    display: none;
  }

  .workspace {
    grid-area: workspace;
    grid-template-columns: minmax(0, 1fr);
  }
}
```

- [ ] **Step 6: 任务级命令**

Run: `rg -n "ShellTopBar|className=\"topbar|\\.topbar\\b|shell-titlebar|ShellPrimaryNav|ShellRightActions" apps/web/src`

Expected: `ShellTopBar` 只作为文件名或导入源出现，不再作为 JSX component 使用；没有 `className="topbar"`；`shell-titlebar`、`ShellPrimaryNav`、`ShellRightActions` 命中。

## Task 3: Library 工具上移与预览标题栏工具

**Files:**
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/features/library/LibraryView.tsx`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: 在 ShellTopBar.tsx 增加 LibraryTitlebarToolbar**

从 `../features/library/FilterMenu`、`SearchBar`、`SortMenu` 复用现有组件，不复制搜索/筛选/排序逻辑。组件接口：

```tsx
export function LibraryTitlebarToolbar({
  favorite,
  kind,
  mediaCount,
  minRating,
  onClearFilters,
  onRefresh,
  onSetKind,
  onSetMinRating,
  onSetQ,
  onSetSort,
  onToggleFavorite,
  onToggleTag,
  q,
  scanActive,
  searchActive,
  sort,
  tagIds,
  tags,
  title
}: LibraryTitlebarToolbarProps) {
  return (
    <div className="titlebar-workspace-toolbar titlebar-library-toolbar no-drag">
      <FilterMenu
        favorite={favorite}
        kind={kind}
        minRating={minRating}
        onClear={onClearFilters}
        onSetKind={onSetKind}
        onSetMinRating={onSetMinRating}
        onToggleFavorite={onToggleFavorite}
        onToggleTag={onToggleTag}
        tagIds={tagIds}
        tags={tags}
      />
      <SearchBar value={q} onChange={onSetQ} />
      <SortMenu value={sort} onChange={onSetSort} />
      <LiquidGlassButton
        aria-label="Refresh library"
        className="titlebar-icon-button no-drag"
        onClick={onRefresh}
        title="Refresh"
        tone="control"
        type="button"
      >
        <RefreshCw aria-hidden="true" size={16} />
      </LiquidGlassButton>
      <div className="titlebar-library-summary" title={title}>
        {mediaCount} media{searchActive ? " / filtered" : ""}{scanActive ? " / scanning" : ""}
      </div>
    </div>
  );
}
```

`LibraryTitlebarToolbarProps` 的 `kind`、`minRating`、`sort` 类型直接从现有组件 props 或 local union 复用，避免 Shell 读取 Core。

- [ ] **Step 2: 增加 PreviewTitlebarToolbar**

接口：

```tsx
export function PreviewTitlebarToolbar({
  canGoNext,
  canGoPrevious,
  mode,
  scale,
  selectedName,
  onBack,
  onGoNext,
  onGoPrevious,
  onResetView,
  onToggleActualSize
}: PreviewTitlebarToolbarProps) {
  return (
    <div className="titlebar-workspace-toolbar titlebar-preview-toolbar no-drag">
      <LiquidGlassButton className="library-toolbar-back no-drag" onClick={onBack} tone="control" type="button">
        <ArrowLeft aria-hidden="true" size={16} />
        <span>Back</span>
      </LiquidGlassButton>
      <div className="titlebar-preview-divider" aria-hidden="true" />
      <LiquidGlassButton aria-label="Previous media" className="titlebar-icon-button no-drag" disabled={!canGoPrevious} onClick={onGoPrevious} tone="control" type="button">
        <ChevronLeft size={16} />
      </LiquidGlassButton>
      <LiquidGlassButton aria-label="Next media" className="titlebar-icon-button no-drag" disabled={!canGoNext} onClick={onGoNext} tone="control" type="button">
        <ChevronRight size={16} />
      </LiquidGlassButton>
      <LiquidGlassButton className="titlebar-preview-mode no-drag" onClick={onToggleActualSize} tone="control" type="button">
        {mode === "actual" ? "100%" : "Fit long edge"}
      </LiquidGlassButton>
      <LiquidGlassButton aria-label="Reset preview view" className="titlebar-icon-button no-drag" onClick={onResetView} tone="control" type="button">
        <RotateCcw size={15} />
      </LiquidGlassButton>
      <div className="titlebar-preview-summary" title={selectedName}>
        {Math.round(scale * 100)}% · {selectedName}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Lift preview navigation to App**

在 `App.tsx` 中新增：

```tsx
const selectedMediaIndex = library.media.findIndex((item) => item.id === library.selectedMediaId);
const canPreviewPrevious = selectedMediaIndex > 0;
const canPreviewNext = selectedMediaIndex >= 0 && selectedMediaIndex < library.media.length - 1;

const handlePreviewPrevious = useCallback(() => {
  if (selectedMediaIndex <= 0) return;
  const previous = library.media[selectedMediaIndex - 1];
  if (previous) library.setSelectedMediaId(previous.id);
}, [library, selectedMediaIndex]);

const handlePreviewNext = useCallback(() => {
  if (selectedMediaIndex < 0 || selectedMediaIndex >= library.media.length - 1) return;
  const next = library.media[selectedMediaIndex + 1];
  if (next) library.setSelectedMediaId(next.id);
}, [library, selectedMediaIndex]);
```

为预览标题栏状态添加：

```tsx
const [previewViewState, setPreviewViewState] = useState({
  mode: "fit-long-edge" as const,
  scale: 1
});
const [previewViewCommands, setPreviewViewCommands] = useState<{
  reset: () => void;
  toggleActualSize: () => void;
} | null>(null);
```

- [ ] **Step 4: renderCenterTitlebar 接入 Library 与 Preview**

在 `App.tsx` 的 `renderCenterTitlebar()` 中：

```tsx
if (activeView === "library" && previewOpen && library.selectedMedia) {
  return (
    <PreviewTitlebarToolbar
      canGoNext={canPreviewNext}
      canGoPrevious={canPreviewPrevious}
      mode={previewViewState.mode}
      scale={previewViewState.scale}
      selectedName={library.selectedMedia.name}
      onBack={() => setPreviewOpen(false)}
      onGoNext={handlePreviewNext}
      onGoPrevious={handlePreviewPrevious}
      onResetView={() => previewViewCommands?.reset()}
      onToggleActualSize={() => previewViewCommands?.toggleActualSize()}
    />
  );
}

if (activeView === "library") {
  const selectedRoot = library.roots.find((root) => root.id === library.selectedRootId) ?? null;
  const selectedFolder = library.folders.find((folder) => folder.id === library.selectedFolderId);
  return (
    <LibraryTitlebarToolbar
      favorite={library.searchState.favorite}
      kind={library.searchState.kind}
      mediaCount={library.media.length}
      minRating={library.searchState.minRating}
      onClearFilters={library.clearFilters}
      onRefresh={() => void library.refresh()}
      onSetKind={library.setKind}
      onSetMinRating={library.setMinRating}
      onSetQ={library.setQ}
      onSetSort={library.setSort}
      onToggleFavorite={library.toggleFavoriteFilter}
      onToggleTag={library.toggleTagFilter}
      q={library.searchState.q}
      scanActive={library.scanActive}
      searchActive={library.searchActive}
      sort={library.searchState.sort}
      tagIds={library.searchState.tagIds}
      tags={library.tags}
      title={selectedFolder?.name ?? selectedRoot?.displayName ?? "Library"}
    />
  );
}
```

- [ ] **Step 5: LibraryView 移除内容区 toolbar**

删除 `LibraryView.tsx` 中的 `library-content-toolbar` 块、`FilterMenu` / `SearchBar` / `SortMenu` / `RefreshCw` / `ArrowLeft` imports 和内部 `handlePreviewPrevious` / `handlePreviewNext` 函数。

更新 props：

```ts
interface LibraryViewProps {
  library: LibraryState;
  previewOpen: boolean;
  onOpenPreview: (mediaId: number) => void;
  onClosePreview: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  onPreviewViewStateChange: (state: { mode: "fit-long-edge" | "actual"; scale: number }) => void;
  onPreviewCommandChange: (commands: { reset: () => void; toggleActualSize: () => void } | null) => void;
  onMediaContextMenu?: (event: {
    item: MediaRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
}
```

`CentralPreviewStage` 调用改为：

```tsx
<CentralPreviewStage
  selectedMedia={previewMedia}
  onClosePreview={onClosePreview}
  onPreviewNext={onPreviewNext}
  onPreviewPrevious={onPreviewPrevious}
  onCommandChange={onPreviewCommandChange}
  onViewStateChange={onPreviewViewStateChange}
/>
```

`grid-surface` 内只保留 error strip 与 `library-grid-content`。

- [ ] **Step 6: 工具栏 CSS**

新增：

```css
.titlebar-workspace-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  min-width: 0;
}

.titlebar-library-toolbar .search-bar {
  flex: 1 1 280px;
  max-width: 520px;
}

.titlebar-library-summary,
.titlebar-preview-summary {
  min-width: 0;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.titlebar-icon-button {
  width: 34px;
  height: 34px;
  padding: 0;
}

.titlebar-preview-divider {
  width: 1px;
  height: 24px;
  background: var(--line);
}

.titlebar-preview-mode {
  height: 34px;
  padding: 0 10px;
  font-size: 12px;
  white-space: nowrap;
}
```

删除 `.library-content-toolbar` 布局依赖；如果样式残留，保证没有 JSX 引用。

- [ ] **Step 7: 任务级命令**

Run: `rg -n "library-content-toolbar|LibraryTitlebarToolbar|PreviewTitlebarToolbar|onPreviewViewStateChange|onPreviewCommandChange" apps/web/src`

Expected: `library-content-toolbar` 没有 JSX 命中；titlebar toolbar 和 preview state/commands 命中 `App.tsx`、`ShellTopBar.tsx`、`LibraryView.tsx`、`CentralPreviewStage.tsx`。

## Task 4: drag/no-drag 与双击最大化/还原契约

**Files:**
- Modify: `apps/web/src/styles.css`
- Modify: `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
- Modify: `apps/web/src/features/window-chrome/WindowChrome.tsx`
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`

- [ ] **Step 1: 统一 no-drag 语义**

在 `LiquidGlassButton` 默认 className 追加 `no-drag`：

```tsx
className={liquidGlassClassName(
  { active, className, scrollable: false, tone, variant },
  "liquid-glass-button no-drag"
)}
```

在按钮 JSX 添加：

```tsx
data-no-drag="true"
```

`LiquidGlassSurface` 不默认 no-drag，因为 titlebar surface 的空白区域必须可拖动；交互 surface 内的控件由 CSS 子选择器处理。

- [ ] **Step 2: WindowChrome 标记 no-drag**

`WindowChrome.tsx` 根节点改为：

```tsx
<div className="window-chrome no-drag" data-no-drag="true" role="group" aria-label="Window controls">
```

每个 `LiquidGlassButton` 保留 `window-chrome-button` class。

- [ ] **Step 3: CSS drag/no-drag**

新增或替换：

```css
.shell-drag {
  -webkit-app-region: drag;
}

.no-drag,
[data-no-drag="true"],
.shell-titlebar :where(button, input, select, textarea, a, [role="button"], [role="tab"], [role="tablist"], [role="group"]) {
  -webkit-app-region: no-drag;
}

.shell-titlebar-center .shell-titlebar-placeholder,
.shell-titlebar-summary {
  flex: 1 1 auto;
  min-width: 0;
  height: 100%;
}
```

删除旧 `.topbar-drag` 选择器，或保留但没有 JSX 使用；静态检查会确保新路径存在。

- [ ] **Step 4: 双击最大化/还原实现决策**

不要给空白标题栏写 React `onDoubleClick` handler。Electron 的 `-webkit-app-region: drag` 区域由桌面窗口系统处理拖动和双击最大化/还原；按钮/输入/菜单因为 `no-drag` 不触发窗口拖动。后续 visual harness 用实际 Electron window controls 验证。

- [ ] **Step 5: 任务级命令**

Run: `rg -n "shell-drag|no-drag|data-no-drag|topbar-drag|-webkit-app-region" apps/web/src`

Expected: `shell-drag` 和 `no-drag` 命中；`topbar-drag` 没有 JSX 命中；`-webkit-app-region: drag` 与 `-webkit-app-region: no-drag` 都在 `styles.css` 中。

## Task 5: LiquidGlass blur 与 brightness tokens

**Files:**
- Modify: `apps/web/src/styles.css`
- Modify: `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`

- [ ] **Step 1: 保留真实 backdrop blur 在材质层**

`.liquid-glass-backdrop` 必须继续包含：

```css
-webkit-backdrop-filter: blur(var(--glass-blur-current)) saturate(var(--glass-saturation));
backdrop-filter:
  url("#mangoo-liquid-glass-refraction")
  blur(var(--glass-blur-current))
  saturate(var(--glass-saturation));
```

禁止在 `.app-shell`、`.workspace`、`.grid-surface`、`.virtual-grid`、`.central-preview-stage` 添加 `backdrop-filter` 或非透明 background。

- [ ] **Step 2: pointer glow 与 edge highlight 分变量**

在 `:root` 将当前强度拆成基准乘数：

```css
--glass-pointer-glow-brightness: 1;
--glass-edge-highlight-brightness: 5;
--glass-pointer-fill-opacity: calc(0.01 * var(--glass-pointer-glow-brightness));
--glass-pointer-press-opacity: calc(0.014 * var(--glass-pointer-glow-brightness));
--glass-pointer-illumination-opacity: calc(0.048 * var(--glass-pointer-glow-brightness));
--glass-border-highlight-opacity: calc(0.032 * var(--glass-edge-highlight-brightness));
```

这样默认 edge `5` 得到 `0.16`，等价于现有局部边缘高亮强度；pointer glow 默认保持现有强度。

- [ ] **Step 3: 不让 pointer hover 污染全窗口**

确认 `.liquid-glass[data-glass-pointer="active"]` 不设置 `--glass-border-current`，局部亮边只由 `.liquid-glass-edge` 的径向 mask 表达。

保留 `LiquidGlassSurface.tsx` 中的 `GLASS_POINTER_EDGE_PROXIMITY_PX = 56`、`nearestPointOnGlassEdge`、`distanceToGlassEdge`，不要改回整块 surface hover。

全局 pointer tracking 仍只更新 `[data-liquid-glass]` surface 的局部 CSS variables。

- [ ] **Step 4: 任务级命令**

Run: `rg -n "backdrop-filter|--glass-pointer-glow-brightness|--glass-edge-highlight-brightness|--glass-border-highlight-opacity|nearestPointOnGlassEdge|distanceToGlassEdge" apps/web/src/styles.css apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`

Expected: blur 只在 `.liquid-glass-backdrop` 和 elevated overlays/fallbacks 中；brightness tokens 命中 `styles.css`；edge proximity helpers 命中 `LiquidGlassSurface.tsx`。

## Task 6: 中央预览交互与透明无边框画布

**Files:**
- Modify: `apps/web/src/features/preview/CentralPreviewStage.tsx`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: 扩展 CentralPreviewStage props**

```ts
type PreviewViewMode = "fit-long-edge" | "actual";

interface CentralPreviewStageProps {
  selectedMedia: MediaRecord;
  onClosePreview: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  onViewStateChange: (state: { mode: PreviewViewMode; scale: number }) => void;
  onCommandChange: (commands: { reset: () => void; toggleActualSize: () => void } | null) => void;
}
```

- [ ] **Step 2: 回传 view state**

新增 effect：

```ts
useEffect(() => {
  onViewStateChange({ mode: viewMode, scale });
}, [onViewStateChange, scale, viewMode]);
```

- [ ] **Step 3: 暴露 titlebar 命令**

将现有 `resetTransform()` 与双击切换逻辑拆成可复用函数：

```ts
const toggleActualSizeAt = useCallback((clientX: number, clientY: number) => {
  if (viewMode === "actual") {
    zoomAtPoint(clientX, clientY, fitLongEdgeScale() ?? 1, "fit-long-edge");
    return;
  }
  zoomAtPoint(clientX, clientY, actualSizeScale(), "actual");
}, [viewMode, zoomAtPoint]);

const toggleActualSizeAtCenter = useCallback(() => {
  const point = stageCenterPoint();
  if (!point) return;
  toggleActualSizeAt(point.x, point.y);
}, [toggleActualSizeAt]);

useEffect(() => {
  onCommandChange({ reset: resetTransform, toggleActualSize: toggleActualSizeAtCenter });
  return () => onCommandChange(null);
}, [onCommandChange, toggleActualSizeAtCenter]);
```

`handleDoubleClick` 调用 `toggleActualSizeAt(event.clientX, event.clientY)`。

- [ ] **Step 4: 保持滚轮分流**

`onPreviewWheel` 必须保持：

```ts
event.preventDefault();
if (event.ctrlKey) {
  zoomAtPoint(event.clientX, event.clientY, scale * (dominantDelta < 0 ? 1.12 : 0.89), "actual");
  return;
}
if (dominantDelta > 0) onPreviewNext();
else if (dominantDelta < 0) onPreviewPrevious();
```

普通滚轮只翻页；`Ctrl + wheel` 只缩放，并阻止浏览器页面缩放。

- [ ] **Step 5: 修正长边填充算法**

当前函数名 `fitLongEdgeScale()` 需要与规格保持一致。实现选择：长边填充 = `Math.min(stageWidth / naturalWidth, stageHeight / naturalHeight)`，保证长边贴合可用舞台且媒体完整可见。不要用 `Math.max`，因为那会裁切短边。

- [ ] **Step 6: 透明无边框 CSS**

确保：

```css
.central-preview,
.central-preview-stage {
  background: transparent;
}

.central-preview-stage {
  border: 0;
  border-radius: 0;
  outline: none;
  box-shadow: none;
  overscroll-behavior: contain;
  touch-action: none;
}

.central-preview-stage:focus-visible {
  outline: none;
  box-shadow: none;
}

.central-preview-stage .preview-placeholder.ready {
  width: max-content;
  height: max-content;
  background: transparent;
  overflow: visible;
}

.central-preview-stage .preview-image {
  width: auto;
  height: auto;
  max-width: none;
  max-height: none;
  object-fit: contain;
}

.central-preview-transform {
  position: absolute;
  top: 50%;
  left: 50%;
  width: max-content;
  height: max-content;
  transform-origin: top left;
}
```

- [ ] **Step 7: 任务级命令**

Run: `rg -n "onViewStateChange|onCommandChange|event.ctrlKey|onDoubleClick|setPointerCapture|fit-long-edge|central-preview-stage|central-preview-transform" apps/web/src/features/preview/CentralPreviewStage.tsx apps/web/src/styles.css`

Expected: 状态回传、命令回传、滚轮分流、双击、拖拽、长边填充和透明样式均命中。

## Task 7: 右侧 PreviewPanel 横竖图 contain

**Files:**
- Modify: `apps/web/src/features/preview/PreviewPanel.tsx`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: 保持 PreviewPanel 只做摘要预览**

`PreviewPanel` 继续使用：

```tsx
<div className="preview-stage" style={previewStageStyle(selectedMedia)}>
  <MediaPreview
    media={selectedMedia}
    onMediaReady={undefined}
    source="original"
    thumbnail={thumbnail}
  />
</div>
```

成功态不添加额外 wrapper、不添加背景图层、不使用 thumbnail crop。

- [ ] **Step 2: aspect-ratio 决策**

保留 `previewStageStyle(media)`，横图、竖图、方图均用原始 `width / height` 设置 stage aspect ratio：

```ts
return { aspectRatio: `${media.width} / ${media.height}` };
```

如果尺寸缺失，fallback 使用 CSS 的 `aspect-ratio: 1 / 1`，仍然 `contain`。

- [ ] **Step 3: CSS contain 与透明**

确保：

```css
.preview-stage {
  display: grid;
  aspect-ratio: 1 / 1;
  min-width: 0;
  max-height: 260px;
  place-items: center;
  overflow: hidden;
  border: 0;
  background: transparent;
}

.preview-stage .preview-placeholder.ready {
  background: transparent;
}

.preview-panel .preview-image {
  width: auto;
  height: auto;
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
}
```

不要让 `.preview-stage` 与 `.tile-thumb` 共享 opaque background。

- [ ] **Step 4: 任务级命令**

Run: `rg -n "previewStageStyle|source=\"original\"|preview-panel .preview-image|\\.preview-stage|object-fit: contain" apps/web/src/features/preview/PreviewPanel.tsx apps/web/src/styles.css`

Expected: `source="original"` 命中 PreviewPanel；`.preview-panel .preview-image` 使用 `width: auto`、`height: auto`、`max-width: 100%`、`max-height: 100%`、`object-fit: contain`；`.preview-stage` 透明无边框。

## Task 8: validate-ui-design.mjs 静态契约更新

**Files:**
- Modify: `tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: 更新读取目标**

保留读取这些文件：

```js
const appShell = read("apps/web/src/app-shell/AppShell.tsx");
const shellTitlebar = read("apps/web/src/app-shell/ShellTopBar.tsx");
const interfaceStyle = read("apps/web/src/features/settings/interfaceStyle.ts");
```

后续断言用 `shellTitlebar` 表示替换后的 titlebar slot 组件，不再按旧全宽 topbar 语义命名。

- [ ] **Step 2: 删除旧 topbar overlay 断言**

移除这些旧要求：

```js
shellTopBar.includes("WindowChrome")
topbarBlocks.some((block) => /position:\s*absolute/.test(block))
styles.includes(".topbar,")
styles.includes(".topbar-drag")
```

替换为三列断言：

```js
for (const value of [
  "titlebarLeft",
  "titlebarCenter",
  "titlebarRight",
  "shell-titlebar-left",
  "shell-titlebar-center",
  "shell-titlebar-right"
]) {
  if (!appShell.includes(value) && !styles.includes(value)) {
    fail(`integrated titlebar shell contract missing ${value}`);
  }
}

if (app.includes("<ShellTopBar")) {
  fail("old full-width ShellTopBar JSX must not be rendered");
}

if (/className=["'][^"']*\btopbar\b/.test(app) || /className=["'][^"']*\btopbar\b/.test(shellTitlebar)) {
  fail("old full-width topbar class must not be used");
}
```

- [ ] **Step 3: 校验三列 grid 与到顶**

```js
const appShellBlocks = cssBlocksForSelector(styles, ".app-shell");
if (!appShellBlocks.some((block) =>
  /grid-template-areas:\s*"titlebar-left titlebar-center titlebar-right"\s*"sidebar workspace workspace"/.test(block) &&
  /grid-template-rows:\s*var\(--shell-titlebar-height\)\s+minmax\(0,\s*1fr\)/.test(block)
)) {
  fail("app shell must define integrated three-column titlebar grid");
}

for (const selector of [".shell-titlebar-left", ".shell-titlebar-center", ".shell-titlebar-right"]) {
  if (cssBlocksForSelector(styles, selector).length === 0) {
    fail(`integrated titlebar selector missing ${selector}`);
  }
}
```

- [ ] **Step 4: 校验 localStorage 偏好与 CSS variables**

```js
for (const value of [
  "mangoo.interfaceStyle",
  "DEFAULT_INTERFACE_STYLE",
  "glassBlur",
  "pointerGlowBrightness",
  "edgeHighlightBrightness",
  "applyInterfaceStyleVariables",
  "useInterfaceStyle"
]) {
  if (!interfaceStyle.includes(value)) {
    fail(`interface style preference contract missing ${value}`);
  }
}

for (const value of [
  "--glass-pointer-glow-brightness",
  "--glass-edge-highlight-brightness",
  "--glass-blur",
  "--glass-elevated-blur",
  "--glass-control-blur"
]) {
  if (!styles.includes(value) && !interfaceStyle.includes(value)) {
    fail(`interface style CSS variable contract missing ${value}`);
  }
}

if (!settingsView.includes("Interface style") || !settingsView.includes("Reset interface style")) {
  fail("settings must expose Interface style controls and reset");
}
```

- [ ] **Step 5: 校验 drag/no-drag**

```js
if (!styles.includes(".shell-drag") || !styles.includes("-webkit-app-region: drag")) {
  fail("integrated titlebar must define draggable blank regions");
}

if (!styles.includes(".no-drag") || !styles.includes("-webkit-app-region: no-drag")) {
  fail("interactive titlebar controls must define no-drag regions");
}

for (const value of ["data-no-drag", "WindowChrome", "ShellRightActions", "ShellPrimaryNav"]) {
  if (!shellTitlebar.includes(value) && !windowChrome.includes(value) && !liquidGlassSurface.includes(value)) {
    fail(`integrated titlebar no-drag/control contract missing ${value}`);
  }
}
```

- [ ] **Step 6: 校验 blur 层级、preview 与右侧 contain**

沿用现有 preview 断言，并新增：

```js
for (const selector of [".app-shell", ".workspace", ".grid-surface", ".virtual-grid", ".central-preview-stage"]) {
  for (const block of cssBlocksForSelector(styles, selector)) {
    if (block.includes("backdrop-filter")) {
      fail(`${selector} must not use persistent backdrop-filter`);
    }
    if (hasNonTransparentBackground(block)) {
      fail(`${selector} must not paint a global gray backing plate`);
    }
  }
}

if (!styles.includes("calc(0.032 * var(--glass-edge-highlight-brightness))")) {
  fail("edge highlight default must be controlled by brightness variable with 5x default");
}

if (!centralPreviewStage.includes("onViewStateChange") || !centralPreviewStage.includes("onCommandChange")) {
  fail("central preview must expose state and commands for the integrated center titlebar");
}
```

- [ ] **Step 7: 任务级命令**

Run: `npm run check:ui-design`

Expected: 如果实现代码和契约同步，输出 `PASS: UI liquid glass design boundaries` 或更新后的 pass 文案，进程退出码为 `0`。

## Task 9: visual harness 截图与断言更新

**Files:**
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs` if it exists

- [ ] **Step 1: 如果 harness 文件存在，更新 layoutEvidenceExpression**

将 `.topbar` selectors 改为三列：

```js
const titlebarLeft = box(".shell-titlebar-left");
const titlebarCenter = box(".shell-titlebar-center");
const titlebarRight = box(".shell-titlebar-right");
```

返回：

```js
return {
  htmlBackground: css("html")?.backgroundColor ?? null,
  bodyBackground: css("body")?.backgroundColor ?? null,
  rootBackground: css("#root")?.backgroundColor ?? null,
  shellBackground: css(".app-shell")?.backgroundColor ?? null,
  shellBackgroundImage: css(".app-shell")?.backgroundImage ?? null,
  titlebarLeft,
  titlebarCenter,
  titlebarRight,
  gridSurface: box(".grid-surface"),
  inspector: box(".inspector-panel"),
  sidebar: box(".library-sidebar")
};
```

- [ ] **Step 2: 更新初始截图与布局断言**

截图名改为：

```js
const integratedTitlebarMain = await screenshot(client, "ui-integrated-titlebar-main.png");
```

断言：

```js
if (!near(layout.titlebarLeft?.top, 0, 1) || !near(layout.titlebarCenter?.top, 0, 1) || !near(layout.titlebarRight?.top, 0, 1)) {
  hardFailures.push("integrated titlebar columns do not reach the top of the window");
}
if (!near(layout.sidebar?.top, layout.titlebarLeft?.bottom, 1)) {
  hardFailures.push("library sidebar is not visually connected below the left titlebar");
}
if (!near(layout.inspector?.left, layout.titlebarRight?.left, 2)) {
  hardFailures.push("right titlebar is not aligned with the inspector column");
}
```

- [ ] **Step 3: 增加 Settings Interface style 断言**

进入 Settings：

```js
await clickSelector(client, '[aria-label="Settings"]');
await waitFor(client, `Boolean([...document.querySelectorAll(".settings-section-title")].find((node) => node.textContent === "Interface style"))`, 10000, "Interface style settings");
const interfaceStyleScreenshot = await screenshot(client, "ui-settings-interface-style.png");
const interfaceStyleEvidence = await evaluate(client, `(() => {
  const section = [...document.querySelectorAll(".settings-section")].find((node) => node.textContent?.includes("Interface style"));
  const sliders = section ? [...section.querySelectorAll('input[type="range"]')].map((input) => ({ id: input.id, value: input.value })) : [];
  return {
    sectionPresent: Boolean(section),
    sliderCount: sliders.length,
    resetPresent: Boolean(section?.querySelector("button")),
    edgeBrightness: getComputedStyle(document.documentElement).getPropertyValue("--glass-edge-highlight-brightness").trim()
  };
})()`);
```

断言 `sectionPresent === true`、`sliderCount === 3`、`resetPresent === true`、`edgeBrightness === "5"`。

- [ ] **Step 4: 增加 drag/no-drag 与双击最大化/还原验证**

使用 CDP 在 `.shell-titlebar-center` 空白处双击，并用 desktop bridge 查询：

```js
async function doubleClickTitlebarBlank(client) {
  const rect = await evaluate(client, `(() => {
    const element = document.querySelector(".shell-titlebar-center");
    const rect = element.getBoundingClientRect();
    return { x: rect.left + rect.width * 0.78, y: rect.top + rect.height * 0.5 };
  })()`);
  await client.send("Input.dispatchMouseEvent", { type: "mousePressed", x: rect.x, y: rect.y, button: "left", clickCount: 2 });
  await client.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: rect.x, y: rect.y, button: "left", clickCount: 2 });
  await delay(350);
}

const beforeMaximized = await evaluate(client, `window.mangooDesktop?.windowControls?.isMaximized?.()`);
await doubleClickTitlebarBlank(client);
const afterFirstDoubleClick = await evaluate(client, `window.mangooDesktop?.windowControls?.isMaximized?.()`);
await doubleClickTitlebarBlank(client);
const afterSecondDoubleClick = await evaluate(client, `window.mangooDesktop?.windowControls?.isMaximized?.()`);
```

断言 `afterFirstDoubleClick !== beforeMaximized` 且 `afterSecondDoubleClick === beforeMaximized`。再点击 SearchBar 并确认焦点进入 input：

```js
await clickSelector(client, ".shell-titlebar-center .search-bar-input");
const focusedSearch = await evaluate(client, `document.activeElement?.classList.contains("search-bar-input")`);
if (!focusedSearch) hardFailures.push("titlebar no-drag controls cannot receive focus");
```

- [ ] **Step 5: 更新 pointer evidence**

将 `pointerEvidenceExpression()` 中 `topbar` 改为 titlebar 三列：

```js
return {
  titlebarLeft: surfaceState(".shell-titlebar-left"),
  titlebarCenter: surfaceState(".shell-titlebar-center"),
  titlebarRight: surfaceState(".shell-titlebar-right"),
  inspector: surfaceState(".inspector-panel"),
  sidebar: surfaceState(".library-sidebar"),
  edgeBrightness: getComputedStyle(document.documentElement).getPropertyValue("--glass-edge-highlight-brightness").trim()
};
```

保留近边缘只激活当前 surface 的断言，新增 `edgeBrightness === "5"`。

- [ ] **Step 6: 保留并更新 preview 截图**

保留以下截图场景，文件名可更新但语义不变：

```js
"ui-selected-portrait-right-preview.png"
"ui-central-landscape-fit-long-edge.png"
"ui-central-landscape-actual-100.png"
"ui-central-portrait-fit-long-edge.png"
"ui-local-edge-highlight.png"
```

断言继续检查：
- 右侧 preview source 为 `original`。
- 右侧 ready/stage background 透明、border 为 `0px none`。
- 中央 preview 默认 `fit-long-edge`，双击切到 `actual`，第二次双击回 `fit-long-edge`。
- 中央 preview stage 无 border、无 inset focus ring、无 outline。

- [ ] **Step 7: 任务级命令**

Run: `if (Test-Path .tmp\visual-check\desktop-ui-regression.mjs) { node .tmp\visual-check\desktop-ui-regression.mjs } else { Write-Output "visual harness missing; skipped" }`

Expected: 存在时生成 `.tmp\visual-check\logs\desktop-ui-regression-summary.json` 且进程退出码为 `0`；不存在时只输出 `visual harness missing; skipped`。

## Task 10: 完整验证与交付检查

**Files:**
- Review only: all files changed by Tasks 1-9

- [ ] **Step 1: 静态 UI 契约**

Run: `npm run check:ui-design`

Expected: 退出码 `0`，输出 pass 文案；没有旧 `ShellTopBar` 全宽横栏、全窗口灰底、preview 黑底/描边、缺失 no-drag 的失败信息。

- [ ] **Step 2: Web 检查**

Run: `npm run check:web`

Expected: 退出码 `0`，Web boundary 和 `tsc -p apps/web/tsconfig.json --noEmit` 均通过。

- [ ] **Step 3: Desktop 检查**

Run: `npm run check:desktop`

Expected: 退出码 `0`，Electron/Core process boundary 和 `tsc -p apps/desktop/tsconfig.json --noEmit` 均通过。

- [ ] **Step 4: Web build**

Run: `npm --workspace @mangoo/web run build`

Expected: 退出码 `0`，Vite build 完成，无 TypeScript 或 bundling error。

- [ ] **Step 5: Desktop build**

Run: `npm --workspace @mangoo/desktop run build`

Expected: 退出码 `0`，desktop main/preload build 完成。

- [ ] **Step 6: Desktop visual harness**

Run: `if (Test-Path .tmp\visual-check\desktop-ui-regression.mjs) { node .tmp\visual-check\desktop-ui-regression.mjs } else { Write-Output "visual harness missing; skipped" }`

Expected: 存在时退出码 `0`，summary JSON 中没有 `fatalError`、`consoleErrors`、`networkProblems`、`hardFailures`；不存在时输出 skip 文案。

- [ ] **Step 7: 汇总人工检查点**

打开实现 diff，逐项确认：
- `App.tsx` 不再渲染 `<ShellTopBar`。
- `AppShell.tsx` 有 `titlebarLeft` / `titlebarCenter` / `titlebarRight` slots。
- `LibraryView.tsx` 不再拥有搜索/筛选/排序/刷新/返回 toolbar。
- `SettingsView.tsx` 有 `Interface style` section 和 3 个 slider。
- `.app-shell`、`.workspace`、`.grid-surface`、`.central-preview-stage` 没有全窗口灰色底或 blur。
- `.preview-panel .preview-image` 和 `.central-preview-stage .preview-image` 都保留 `object-fit: contain`。

Implementation controller may stage/commit after review, but staging/commit is not part of this plan because the worktree has parallel edits.

## Self-review

- 规格 1 删除旧全局 `ShellTopBar` 横栏：Task 2 替换 Shell 结构，Task 8 增加静态断言，Task 10 人工检查。
- 规格 2 左侧标题栏主导航：Task 2 `ShellPrimaryNav` 放入 `titlebarLeft`，无品牌标题块。
- 规格 3 中间标题栏 Library/Preview 工具：Task 3 上移搜索、筛选、排序、刷新、返回、上一项/下一项、缩放状态、reset、100%/长边填充。
- 规格 4 右侧标题栏 Tasks/Recent/WindowChrome：Task 2 `ShellRightActions` 负责，WindowChrome 仍只负责窗口按钮。
- 规格 5 drag/no-drag 与双击最大化/还原：Task 4 CSS 契约，Task 9 Electron visual harness 验证。
- 规格 6 Liquid Glass blur 恢复且避免全窗口灰底：Task 5 控制 blur 层级，Task 8 静态禁止 root/workspace/content blur 与非透明底。
- 规格 7 本机 UI 偏好：Task 1 localStorage hook/module、CSS variables、Settings sliders、reset。
- 规格 8 中央预览无边框透明和交互：Task 6 覆盖长边填充、双击切换、`Ctrl + wheel`、普通滚轮翻页、拖拽平移、透明无边框。
- 规格 9 右侧横竖图完整 contain：Task 7 和 Task 8/9 覆盖 `source="original"`、透明、无边框、`object-fit: contain`。
- 静态契约更新：Task 8 覆盖 `validate-ui-design.mjs`。
- visual harness 更新：Task 9 覆盖 `.tmp/visual-check/desktop-ui-regression.mjs` 存在时的截图与断言。
- 完整验证命令：Task 10 包含 `npm run check:ui-design`、`npm run check:web`、`npm run check:desktop`、`npm --workspace @mangoo/web run build`、`npm --workspace @mangoo/desktop run build`、`node .tmp\visual-check\desktop-ui-regression.mjs` 的条件执行。
- 占位符扫描：本文所有步骤均为具体执行项。
- 执行模型：任务按单子代理串行执行，没有并行调度设计。

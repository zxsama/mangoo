# Mangoo 三列融合标题栏与玻璃样式设计规格

Updated: 2026-05-20

## 1. 背景

Mangoo 当前已经有无边框 Electron 窗口、Liquid Glass 控制层、Library/Plugins/Settings 工作区、Tasks 与 Recent 浮层，以及右侧检视/预览面板。上一版全局 Shell 方案仍保留了一条独立的 `ShellTopBar` 横栏：左侧品牌标题，中间导航，右侧任务、最近操作和窗口按钮。

用户已批准 UI 方案 B：三列融合标题栏。该方案删除旧的全局横栏，把桌面标题栏区域直接变成 Mangoo 的主要控制带。左、中、右三区都到顶，占用原生标题栏高度和原应用顶栏高度，并与下方侧栏、工作区和右侧检视器融为一体。

本规格是后续实现的准入文档。实现必须以本文为最终设计契约，不再回到旧的“独立全宽 ShellTopBar 横栏”结构。

## 2. 目标

1. 删除旧全局 `ShellTopBar` 横栏，改成三列融合标题栏。
2. 左侧标题栏承载 `Library`、`Plugins`、`Settings` 主导航，并与左侧库侧栏在视觉上连续。
3. 中间标题栏承载 Library 的搜索、筛选、排序、刷新，以及预览打开后的返回与预览工具。
4. 右侧标题栏承载 `Tasks`、`Recent` 和窗口控制按钮，并与右侧检视/预览面板在视觉上连续。
5. 保留无边框窗口的拖动、双击最大化/还原和窗口控制能力。
6. 恢复 Liquid Glass 的真实 blur，但只在玻璃材质层产生 blur，不重新引入全窗口灰色底。
7. 新增本机 UI 偏好，让用户能在 Settings 中调整 glass blur、pointer glow brightness、edge highlight brightness。
8. 让中间预览成为无边框、透明背景、以媒体为中心的画布交互，而不是卡片或弹窗。
9. 保证右侧选中预览横图、竖图都完整 `contain`，没有黑底、描边或裁切。

## 3. 非目标

- 不复制 Eagle、Apple 或 Windows 原生标题栏的像素细节。
- 不引入新的路由框架、状态库或 UI 组件库。
- 不把媒体网格、缩略图、中央预览画布改成大面积玻璃背景。
- 不恢复独立的品牌标题块、全宽顶栏或工作区上方的重复导航带。
- 不把 Tasks 重新做成主工作区页面；Tasks 与 Recent 仍是右侧标题栏触发的浮层能力。
- 不改变 Core API、数据库、缩略图生成、文件操作和插件执行边界。

## 4. 设计方案

### 4.1 总体结构

应用顶层从“全局横栏 + 下方三栏”调整为“三列融合标题栏 + 三栏工作台”：

- 左列：顶部是 `Library`、`Plugins`、`Settings` 主导航；下方是 Library sidebar 或当前工作区需要的左侧区域。
- 中列：顶部是当前工作流工具带；下方是 Library 网格、Plugins 主体、Settings 主体或中央预览。
- 右列：顶部是 `Tasks`、`Recent`、窗口控制；下方是右侧选中预览、元数据检视器或相关浮层锚点。

左、中、右三区都从窗口最顶部开始绘制。标题栏不再是一条盖在三栏上方的横向组件，而是每一列自己的顶部控制区域。三列的顶部高度必须一致，视觉上形成一条连续但分区清晰的桌面 chrome。

### 4.2 左侧标题区

左侧标题区只承载主导航：

- `Library`
- `Plugins`
- `Settings`

按钮使用当前 Liquid Glass 控制语法和 lucide 图标。当前工作区有明确 active 状态。左侧不再保留独立的 `Mangoo` 品牌标题块或副标题，品牌可以通过窗口标题、应用图标或后续轻量标识表达，但不占用高频控制空间。

### 4.3 中间标题区

中间标题区根据当前上下文切换内容：

- Library 常规状态：搜索输入、筛选菜单、排序菜单、刷新按钮。
- Library 预览状态：返回网格按钮、上一项/下一项、缩放状态、重置视图、可选的 100%/长边填充切换。
- Plugins 状态：插件搜索、过滤、刷新或启用状态入口。
- Settings 状态：保留必要的页面级操作；没有操作时留出可拖动空白。

中间标题区是主要工作流工具带，不再在 Library 内容区内部重复渲染另一条大型 toolbar。搜索、筛选、排序、刷新等高频能力应从内容区上移到融合标题栏。

### 4.4 右侧标题区

右侧标题区从左到右承载：

- `Tasks`：打开/关闭任务浮层，扫描或后台任务活跃时保留状态提示。
- `Recent`：打开/关闭最近文件操作浮层。
- 窗口控制：最小化、最大化/还原、关闭。

窗口控制仍通过桌面桥能力执行。在 Web 开发环境没有桌面桥时，窗口控制不渲染，右侧标题区仍保留布局稳定性。

## 5. 组件与文件边界

后续实现必须保持以下边界：

- `apps/web/src/app-shell/AppShell.tsx`：拥有顶层三列布局 slot 和 overlay host 挂载点，不直接处理 Library 数据、任务列表或媒体预览细节。
- `apps/web/src/app-shell/ShellTopBar.tsx`：旧全局横栏的职责要被拆除或替换为融合标题栏组件；不再渲染全宽品牌标题块。
- `apps/web/src/features/window-chrome/WindowChrome.tsx`：继续只负责桌面窗口按钮和窗口状态，不承载导航或业务工具。
- `apps/web/src/features/library/LibraryView.tsx`：继续拥有 Library 工作流状态和预览打开状态；搜索、筛选、排序、刷新控制通过 titlebar slot 上移，不在内容区重复大型 toolbar。
- `apps/web/src/features/library/SearchBar.tsx`、`FilterMenu.tsx`、`SortMenu.tsx`：可复用到中间标题区，行为不应依赖原内容区 DOM 层级。
- `apps/web/src/features/preview/CentralPreviewStage.tsx`：拥有中央预览的缩放、拖拽平移、滚轮翻页和视图模式切换。
- `apps/web/src/features/preview/PreviewPanel.tsx`、`MediaPreview.tsx`：负责右侧选中预览和媒体元素渲染，必须保证 `contain` 且不添加黑底/描边。
- `apps/web/src/features/settings/SettingsView.tsx`：新增 `Interface style` 区域，承载本机 UI 偏好控制。
- `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx` 和 `apps/web/src/styles.css`：负责玻璃材质、CSS variables、drag/no-drag、blur 和局部高亮表现。
- `apps/web/src/core/desktop.ts`：只保留桌面桥类型与调用边界；标题栏拖动语义优先由 CSS `app-region` 处理。

命名可以在实现中调整，但职责边界不能倒退为“业务组件直接拼接一个全局横栏”。

## 6. 状态持久化

新增本机 UI 偏好，存储在 renderer 的 `localStorage` 中。偏好只影响当前用户当前设备，不写入 Core 数据库，不同步到媒体库。

存储键采用明确命名，例如 `mangoo.interfaceStyle`。值使用 JSON 对象，至少包含：

```json
{
  "glassBlur": 1,
  "pointerGlowBrightness": 1,
  "edgeHighlightBrightness": 5
}
```

三个字段挂到 CSS variables：

- `glassBlur` 控制玻璃材质层 blur 强度，例如映射到 `--glass-blur`, `--glass-elevated-blur`, `--glass-control-blur` 或派生变量。
- `pointerGlowBrightness` 控制 pointer glow 亮度，例如映射到 `--glass-pointer-glow-brightness`。
- `edgeHighlightBrightness` 控制局部边缘描边高亮，例如映射到 `--glass-edge-highlight-brightness`。

默认决策：

- `glassBlur` 默认值为 `1`，表示使用设计基准 blur。
- `pointerGlowBrightness` 默认值为 `1`，表示使用设计基准 pointer glow。
- `edgeHighlightBrightness` 默认值为 `5`，表示局部边缘描边高亮默认为当前强度的 5 倍。

读取失败、JSON 损坏或字段缺失时使用默认值并继续渲染。Settings 中的重置按钮恢复以上默认值。

## 7. 交互规则

### 7.1 窗口拖动

- 左、中、右标题区的空白区域均可拖动窗口。
- 空白标题区域双击触发最大化/还原。
- 按钮、输入框、下拉菜单、滑杆、可滚动菜单、任务/最近操作触发器、窗口控制按钮必须是 `no-drag`。
- 可拖动区域不能覆盖控件 hit target；控件点击、文本选择、输入焦点不应触发窗口拖动。

### 7.2 主导航

- `Library`、`Plugins`、`Settings` 位于左侧标题区。
- 切换主导航不关闭无关浮层的规则保持现有 overlay 约定：非破坏性浮层可以由外部点击关闭，任务状态不因切换页面丢失。
- 当前主导航按钮必须提供视觉 active 状态和可访问状态。

### 7.3 中央预览

- 中间预览区没有边框，没有卡片底，没有黑色填充背景。
- 默认视图为长边填充：媒体长边贴合预览舞台可用空间，短边按比例留透明空间。
- 双击预览舞台在 `100%` 与 `长边填充` 之间切换。
- `Ctrl + 滚轮` 按鼠标位置缩放，缩放锚点是当前鼠标所在媒体点。
- 普通滚轮翻页：向下下一项，向上上一项。
- 按住拖拽平移；平移只在预览舞台内生效。
- 关闭预览后返回 Library 网格，选择项保持不变。

### 7.4 右侧选中预览

- 右侧 `PreviewPanel` 是选中项摘要预览，不是中央预览画布。
- 横图、竖图、方图都必须完整显示，使用 `object-fit: contain`。
- 预览区域不出现黑底、描边、裁切或额外相框。
- 未选中、加载中、失败状态使用现有 placeholder 语法，但不改变成功预览的透明 contain 规则。

## 8. 视觉规则

### 8.1 Liquid Glass blur

Liquid Glass 必须恢复真实 `backdrop-filter: blur(...) saturate(...)`。blur 只属于玻璃材质层：

- 标题区玻璃层
- 侧栏玻璃层
- 右侧检视器玻璃层
- 菜单、浮层、任务面板、Recent 面板
- Settings 的设置区块和控件

禁止为了 blur 效果给整个窗口、整个 workspace 或媒体内容区添加半透明灰色大底。内容区保持清晰、深色、低噪声；玻璃材质通过局部 surface 表达。

### 8.2 边缘与 hover 高亮

- 默认局部边缘描边高亮为当前强度的 5 倍。
- 整体边缘 hover 高亮不得回归；鼠标靠近玻璃边缘时仍有可见但不过曝的局部高亮。
- pointer glow 与 edge highlight 分别由不同 CSS variables 控制，避免调亮局部边缘时同时污染所有 hover 背景。

### 8.3 标题栏分区

- 三列顶部高度一致。
- 左侧标题区和左侧 sidebar 在背景、边线、圆角上连续。
- 中间标题区与工作区内容之间只用轻量分隔，不添加厚重横栏。
- 右侧标题区与 inspector/preview panel 在背景、边线、圆角上连续。
- 窗口边缘保留 Liquid Glass 的整体轮廓和圆角，不因三列拆分出现断裂。

### 8.4 Settings Interface style

Settings 新增 `Interface style` 区域，放在现有设置页内，使用同一套 `settings-section` 视觉语法。该区域至少包含：

- Glass blur 滑杆。
- Pointer glow brightness 滑杆。
- Edge highlight brightness 滑杆。
- Reset interface style 按钮。

滑杆改变后立即更新 CSS variables，并持久化到 `localStorage`。控件必须是 `no-drag`。

## 9. 风险

- 三列标题栏到顶后，Electron `app-region: drag` 容易覆盖输入框和按钮；实现必须用静态契约和视觉 harness 验证 no-drag 区域。
- blur 若挂在根窗口或 workspace，会重新出现全窗口灰色底，削弱媒体内容清晰度；实现必须限制 blur 层级。
- 把 Library toolbar 上移后，搜索、筛选、排序状态可能与 `LibraryView` 数据流脱节；实现应通过 props 或轻量 shell context 传递，不让 shell 直接读取 Core 数据。
- 中央预览的滚轮翻页和 `Ctrl + 滚轮` 缩放存在事件冲突；必须以 modifier key 明确分流，并阻止浏览器页面缩放。
- 右侧预览去掉背景和描边后，透明图片边界可能不明显；这是批准的视觉取舍，不能用黑底或描边回补。
- Settings 本机偏好只存 `localStorage`，清浏览器数据会恢复默认值；这是明确产品决策。

## 10. 验证契约

后续实现完成后必须通过以下验证：

1. 静态 UI 契约：
   - 不再渲染旧的全宽 `ShellTopBar` 横栏。
   - 三列标题区都到顶。
   - 可交互控件带有 no-drag 语义。
   - blur 只出现在玻璃 surface 相关选择器。
   - 中央预览没有边框、卡片底或黑底。
   - 右侧预览成功态使用 contain。
2. Web typecheck/build 通过。
3. Desktop typecheck/build 通过。
4. 启动 Web 与 Desktop 时控制台没有 warning/error。
5. 视觉 harness 截图覆盖：
   - 初始 Library 三列标题栏。
   - 右侧选中预览，横图和竖图都完整 contain。
   - 中间预览，默认长边填充、透明背景、无边框。
   - 局部边缘高亮，默认亮度为当前 5 倍且 hover 不回归。
   - Settings 的 `Interface style` 滑杆和 reset 控件。

验证失败时不能以“视觉接近”合并，必须修正到契约一致。

## 11. 后续实施计划入口

后续实现按以下顺序推进：

1. 重构 app shell：建立三列标题栏 slot，拆除旧全宽 `ShellTopBar` 渲染路径。
2. 上移主导航和 Library 工具：左侧放主导航，中间放搜索、筛选、排序、刷新和预览工具，右侧放 Tasks/Recent/窗口控制。
3. 修正 drag/no-drag：空白标题区可拖动和双击最大化，所有控件 no-drag。
4. 调整 Liquid Glass tokens：恢复真实 blur，移除全窗口灰色底风险，新增 pointer glow 与 edge highlight 亮度变量。
5. 新增 Interface style 偏好：Settings 滑杆、localStorage、CSS variables、reset。
6. 修正中央预览：透明无边框、长边填充默认、双击 100%/长边填充、`Ctrl + 滚轮` 缩放、普通滚轮翻页、拖拽平移。
7. 修正右侧选中预览：横竖图 contain，去除黑底和描边。
8. 补齐静态契约、typecheck/build 和视觉 harness 截图验证。

以上步骤是实现入口，不允许在第一步之外重新引入旧横栏作为过渡 UI。

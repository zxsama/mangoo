# Megle UI Layered Acrylic Frosted Glass Design

Updated: 2026-05-16

## 1. 背景

Megle 是一个 Windows-first 的本地媒体浏览与管理工具。它索引用户已有目录，不导入原始文件到私有库中；左侧目录树映射真实文件夹；中间区域承担高密度媒体浏览；右侧区域承担元数据、操作和上下文说明。

当前项目文档已经确定以下基础约束：

- 桌面壳使用 Electron。
- 主界面使用 React + TypeScript。
- Core 使用 Rust，UI 只通过 Core API 访问数据。
- UI 交互参考 Eagle 的高效率资产管理工作流。
- UI 风格采用分层亚克力毛玻璃设计（Layered Acrylic / Frosted Glass），但不能破坏高密度媒体浏览性能。
- 首版是 Windows 桌面版，但后续需要复用同一套 React UI 支撑 Web/Docker 部署。

本设计文档在此基础上，定义 Megle 的统一 UI 设计方向、组件语法和实现路线。

## 2. 设计目标

Megle 的 UI 必须同时满足 5 个目标：

1. 保留接近 Eagle 的高密度资产管理效率。
2. 整个应用的界面、菜单和交互使用同一套分层亚克力毛玻璃设计语言。
3. 玻璃效果只增强导航和控制层，不干扰媒体内容本身。
4. 所有页面共享同一套桌面工作台壳层，而不是各页单独设计。
5. 风格系统必须可工程化，能够拆成 tokens、primitives 和 product components。

## 3. 非目标

- 不复制 Eagle 的视觉细节、资源、布局像素或品牌表达。
- 不把 Apple 的系统界面逐像素搬到 Windows 上。
- 不将整个内容区做成高强度毛玻璃界面。
- 不为了玻璃效果降低媒体网格密度、预览清晰度和键盘操作效率。
- 不让设置页、插件页、任务页退化为另一套“普通后台 UI”。

## 4. 最终设计方向

采用：

`Eagle-like information architecture + Frameless Electron desktop chrome + Layered Acrylic / Frosted Glass design system + Dense dark content stage`

这意味着：

- 信息架构参考高效率本地资产管理工具。
- Electron 使用自绘无边框窗口，不保留原生 Windows 标题栏和系统菜单外观。
- 整个应用属于同一套分层玻璃材质语言，但按层级控制亚克力与毛玻璃强度。
- 媒体网格、预览画布、列表内容区属于稳定深色工作台，不覆盖大面积 blur。

## 5. 核心原则

### 5.1 分层而不是铺满

Megle 统一的是语言，不是把所有区域变成同一种材质。

- 控制层：窗口 chrome、工具栏、侧栏、检视器、菜单、右键菜单、弹层、命令面板、设置页、插件页、任务页。
- 内容层：媒体网格、缩略图墙、预览画布、列表主体、日志主体。

控制层使用亚克力毛玻璃材质。内容层使用高对比、低噪声、深色工作台材质。

### 5.2 内容优先

所有视觉决策都必须服从媒体浏览与操作效率：

- 缩略图必须稳定、不跳动。
- 媒体预览必须清晰、不被背景干扰。
- 右键、搜索、筛选、批量操作必须在高频使用时保持直接。

### 5.3 全应用统一

“统一设计语言”在 Megle 中意味着：

- 同一套 token。
- 同一套圆角、边线、阴影、玻璃层级。
- 同一套交互反馈、焦点态、危险态和加载态。
- 同一套桌面壳层与导航层级。

主工作区、设置页、插件页、任务页、导入流程和确认对话框都必须属于同一套系统。

### 5.4 性能是硬约束

亚克力与毛玻璃效果只能建立在已验证的性能底线上：

- 不能对媒体网格区域施加持续大面积 `backdrop-filter`。
- 不能让预览切换依赖高成本动画。
- 不能让菜单、Inspector、Dialog 的样式影响滚动和选择响应。
- 必须提供 Performance Mode 和 Reduced Transparency 降级路径。

## 6. 全局信息架构

所有页面共用统一桌面工作台壳层：

1. `Window Chrome + Global Toolbar`
2. `Primary Navigation / Library Sidebar`
3. `Content Stage`
4. `Context Inspector`
5. `Task / Activity Drawer`
6. `Global Overlay Layer`

其中：

- `Window Chrome + Global Toolbar` 负责窗口操作、全局搜索、主命令、视图切换和当前上下文。
- `Primary Navigation / Library Sidebar` 负责一级区域入口和当前区域的层级结构。
- `Content Stage` 负责主要工作对象，例如媒体网格、插件列表、设置面板、任务时间线。
- `Context Inspector` 负责当前对象的属性、说明和相关操作。
- `Task / Activity Drawer` 负责扫描、缩略图、文件操作和插件任务。
- `Global Overlay Layer` 统一承载 command palette、menus、dialogs、context menus、toasts。

## 7. 页面布局规则

### 7.1 Library 主工作区

Library 使用 Eagle 式三栏骨架：

- 左栏：真实目录树、收藏、筛选入口、根目录切换。
- 中栏：媒体网格、列表、预览主舞台。
- 右栏：Metadata、Tags、Rating、File Ops、Plugin actions。
- 顶栏：搜索、排序、视图模式、缩放、批量操作、导入入口。
- 任务抽屉：扫描、预取、文件操作与失败恢复。

布局规则：

- 左栏和右栏是玻璃控制层。
- 中栏是深色内容工作台。
- 媒体内容不能被厚玻璃包裹。
- 任意窗口宽度下，中栏优先保面积。

### 7.2 Preview 模式

Preview 模式保留同一套顶栏语言，但弱化左右栏视觉存在感。

- 主舞台以内容为中心。
- 顶部和底部控件轻量悬浮。
- Inspector 可折叠为右侧抽屉。
- 切图、缩放、播放控制仍然使用统一组件语法。

### 7.3 Settings

Settings 使用相同壳层，但中间区域以设置内容为主：

- 左侧为设置分组导航。
- 中间为设置内容。
- 右侧可选说明面板、风险说明或结果摘要。

Settings 不是单独的一套后台风格，而是 Megle 桌面工作台的一个区域。

### 7.4 Plugin Manager

Plugin Manager 使用：

- 左侧插件分类。
- 中间插件列表或卡片。
- 右侧插件详情、权限、日志和启停状态。

插件页仍然使用玻璃侧栏、玻璃工具栏和统一 Inspector 语言。

### 7.5 Task Center

Task Center 使用：

- 中央任务队列或时间线。
- 右侧任务详情、错误原因和恢复操作。
- 顶栏提供过滤、状态分组和清理入口。

任务页的目标是把系统状态可视化，而不是把状态散落在零碎 banner 里。

### 7.6 Import / Root Management

添加索引根、导入源、危险确认等流程统一用向导式内容区：

- 主内容区解释当前步骤。
- 右侧提供路径摘要、风险提示和结果确认。
- 所有 destructive 流程都用高对比 Dialog 或 Sheet，而不是低强调提示条。

## 8. 导航与交互层级

导航只保留 3 层：

1. 一级区域：Library / Favorites / Tags / Tasks / Plugins / Settings
2. 当前区域结构：目录树、筛选组、插件类别、设置分组
3. 当前对象上下文：Inspector、详情面板、右键菜单、二级弹层

不引入第四层持久导航。更深路径只通过 drill-in、popover、dialog 或 command palette 进入。

搜索和命令入口严格分离：

- `Search`：查找内容，作用于当前库或当前视图。
- `Command Palette`：查找动作，作用于整个应用。

## 9. 视觉系统

### 9.1 字体

Megle 使用专业工具导向而不是营销页面导向的排版：

- UI 主字体：`Geist Sans`
- 技术信息字体：`JetBrains Mono`
- 主要字号层级：`12 / 13 / 14 / 16 / 20`

文件名、标签、元数据和筛选信息优先保证扫描效率与对比度。

### 9.2 色彩与材质 tokens

建议建立以下 token 分组：

- `bg.canvas`
- `bg.surface`
- `glass.low`
- `glass.mid`
- `glass.high`
- `accent.primary`
- `accent.warn`
- `accent.danger`
- `stroke.soft`
- `shadow.glass`

建议色彩方向：

- 深墨灰内容工作台
- 偏冷中性玻璃层
- 冰青/湖蓝主强调色
- 琥珀警告色
- 珊瑚红危险色

不使用紫色主视觉路线，不使用纯黑或过度发亮的霓虹表达。

### 9.3 圆角与边线

- 顶栏、侧栏、检视器面板：`18px - 24px`
- 卡片、菜单、输入框：`12px - 16px`
- 小按钮和 segmented item：`10px - 12px`
- 玻璃表面统一使用 `1px` 软边线和轻微内拾光

媒体缩略图本身不套厚玻璃外壳；选中态依靠边框、阴影和状态环表达。

### 9.4 图标

图标系统使用简洁线性图标，统一描边粗细和角圆角策略。

图标语义优先服务于：

- 文件/文件夹
- 视图切换
- 标签/评分/收藏
- 扫描/任务/警告/恢复
- 插件/权限/日志

### 9.5 动效

亚克力毛玻璃材质的动态感主要通过控制层运动表达：

- 菜单、Popover、Dialog 使用短距离漂浮进入
- Inspector 展开与收起使用轻微黏性动画
- 选中切换使用轻微提亮和缩放反馈
- 右键菜单打开快速、低位移、无夸张弹跳
- 网格内容层滚动不做玻璃形变

## 10. 组件系统

组件系统分为三层：

### 10.1 Headless primitives

提供可访问性和交互骨架：

- menu
- context menu
- popover
- dialog
- tooltip
- tabs
- slider
- select
- checkbox
- radio group

### 10.2 Megle primitives

将视觉语言封装为产品级基础组件：

- `GlassButton`
- `GlassIconButton`
- `GlassInput`
- `GlassSearchField`
- `GlassSegmentedControl`
- `GlassMenu`
- `GlassContextMenu`
- `GlassPopover`
- `GlassDialog`
- `GlassSheet`
- `GlassSidebar`
- `GlassToolbar`
- `InspectorPanel`
- `TaskDrawer`
- `Toast`

### 10.3 Product composites

将 Megle 业务界面建立在统一 primitives 上：

- `LibraryToolbar`
- `FolderTree`
- `MediaTile`
- `MediaGrid`
- `PreviewStage`
- `MetadataInspector`
- `TaskTimeline`
- `PluginCard`
- `PluginDetailPanel`
- `SettingsSection`
- `RootWizard`

## 11. UI 数据流

Megle 的 UI 数据流必须严格服从现有三层架构：

1. Electron Desktop Shell
2. React UI
3. Rust Core API

规则：

- UI 不直接访问文件系统。
- UI 不直接生成缩略图或读取数据库。
- 桌面壳只负责窗口、菜单、系统对话框、拖放、快捷键和本地 session token。
- 所有业务数据都通过 Core API 进入 React UI。

推荐状态分工：

- `TanStack Query`：roots、folders、media pages、preview state、tasks、plugins、settings data
- `Zustand`：selected item、grid zoom、panel widths、view mode、transient filter draft、overlay state
- `App Shell overlay host`：menus、dialogs、command palette、toasts、context menus 的统一挂载层

关键交互流：

- Library 浏览：Toolbar/Sidebar 触发查询条件变更，Query 请求 Core API，Content Stage 只消费分页结果
- 选中切换：UI 先更新本地 selected state，再异步请求 preview / metadata
- 文件操作：UI 发起命令，Core 执行真实文件操作，Task Center 回收状态和错误
- 插件状态：Plugin Manager 展示启停、权限、日志，但插件执行结果仍经由 Core API 回流

## 12. 关键交互规则

### 12.1 Window Chrome

Electron 使用自绘无边框窗口：

- 自定义标题栏
- 自定义窗口控制按钮
- 明确拖拽区与非拖拽区
- 顶栏整合全局搜索、区域切换和高频命令

不保留默认 Windows 标题栏外观。

### 12.2 菜单

菜单规则：

- 顶部主菜单负责应用级动作，数量少且稳定
- 工具栏下拉菜单负责视图、排序、筛选模板
- 右键菜单只放当前上下文高频动作
- 危险动作不直接一级执行，必须进入确认层

### 12.3 选择与状态

- `hover`：全局鼠标邻近层负责同一光标中心的柔和光晕与径向边缘高光；承载径向边缘高光的伪元素只能绘制遮罩后的局部渐变，必须清除继承的整圈边框与内阴影；光标真正进入控件后只增强前景/图标，不再叠加整块背景、边框或阴影变化
- `selected`：清晰状态环 + 轻背景抬升
- `focus-visible`：高可见焦点环
- `pressed`：短促压感反馈
- `disabled`：降低饱和和对比，但保持可读
- `error`：统一危险语义，不做刺眼纯红面板

### 12.4 危险操作

重命名、移动、删除、批量覆盖等操作必须使用统一危险语义：

- 高对比 Dialog 或 Sheet
- 明确结果说明
- 明确可恢复与不可恢复边界
- 操作完成后进入 Task / Activity 语义，而不是只弹一次 toast

## 13. 错误处理与空状态

Megle 的错误处理不应打断主工作流，也不能只靠一次性 toast。

规则：

- 浏览错误：显示在当前内容区的空状态或轻量错误区，并提供重试
- 扫描与后台任务错误：进入 Task Center，保留错误详情与恢复动作
- 文件操作失败：必须记录在任务与操作日志里，并在相关对象上下文中可追踪
- 插件错误：局部隔离在插件详情和日志区，不能污染主工作区
- 危险确认失败：明确告诉用户哪些文件已成功、哪些失败、是否可恢复

空状态必须使用同一套视觉语言：

- Library 空库状态
- 空文件夹状态
- 无搜索结果状态
- 无插件状态
- 无任务状态

这些状态仍属于统一壳层，不能退化成另一套占位页风格。

## 14. 无障碍与可配置项

Megle 必须在设计系统级别支持：

- `Reduced Motion`
- `Reduced Transparency`
- `High Contrast`
- `Performance Mode`
- 完整键盘导航
- 清晰 `focus-visible`
- 不依赖颜色单独表达状态

这些能力必须是 tokens 和组件级能力，而不是页面特判。

## 15. 性能约束

UI 设计必须服从现有性能基线：

- 选中更新目标小于 50ms
- 缓存预览需要快速可见
- 网格滚动不因玻璃效果掉帧
- 缩略图加载不能造成布局抖动
- 菜单、Inspector、Dialog 的效果不能把性能问题传导到内容层

硬性规则：

- 媒体网格区不允许长期悬挂大面积 blur 图层
- 预览舞台不允许因玻璃动画触发高成本重绘链
- 所有 blur、阴影、动画参数必须有降级配置

## 16. 工程实现路线

### 16.1 基础设施优先

在进入大规模页面实现前，先完成 4 组基础设施：

1. design tokens
2. UI primitives
3. frameless desktop chrome
4. theme runtime

### 16.2 推荐代码结构

建议新增以下结构：

```text
packages/
  design-tokens/
  ui/
apps/
  desktop/
    src/window-chrome/
  web/
    src/app-shell/
    src/features/
```

职责：

- `packages/design-tokens`：颜色、半径、阴影、模糊、间距、动画、z-index、字体
- `packages/ui`：Megle primitives
- `apps/desktop/src/window-chrome`：Electron 自绘窗口与菜单桥接
- `apps/web/src/app-shell`：顶栏、侧栏、Inspector、Task Drawer、overlay host

技术基线：

- 组件交互骨架：Radix UI primitives
- 样式体系：Tailwind CSS + CSS variables tokens
- 图标：Lucide
- 远程数据：TanStack Query
- 本地 UI 状态：Zustand

### 16.3 分阶段落地

按当前 roadmap 建议这样接入：

#### Phase 1

- 建 frameless Electron shell
- 建 `App Shell`
- 建主导航、顶栏、Inspector、Task Drawer 模板
- 接入 design tokens、字体和玻璃 primitives

#### Phase 2

- 建 Library 主工作区
- 接真实目录树、媒体网格、Metadata Inspector
- 接搜索、排序、视图切换和缩放控件

#### Phase 3

- 让 thumbnail / preview 状态进入统一视觉系统
- 建 tile loading、selected、prefetch 和 preview transition 语义

#### Phase 4

- 建 Task Center
- 接扫描、索引、watcher、失败恢复、后台任务状态

#### Phase 5

- 建过滤和搜索系统
- 完成 search field、scope、chips、saved filters、sort menu

#### Phase 6

- 建文件操作交互层
- 完成 rename / move / delete 的菜单、确认、进度和恢复路径

#### Phase 8

- 建 Plugin Manager 全套 UI
- 完成插件列表、详情、权限、日志、启停状态

#### Phase 9

- 在不重做设计系统的前提下适配 Web/Docker

## 17. 验证与测试策略

在实现该设计时，验证分为 4 层：

### 17.1 Tokens 与 primitives

- 校验颜色、圆角、边线、阴影、层级是否只来自统一 tokens
- 校验按钮、输入框、菜单、Dialog、Popover、Sidebar、Inspector 是否复用统一 primitives

### 17.2 组件与交互

- 组件测试覆盖菜单、右键菜单、Dialog、Inspector、Toolbar、Task Drawer
- 键盘导航测试覆盖搜索、菜单、选择、关闭、危险确认
- 空状态、错误态和加载态必须有组件级测试

### 17.3 页面与回归

- Library、Settings、Plugins、Tasks、Import 流程必须有页面级截图或视觉回归基线
- 自绘窗口 chrome 与页面壳层必须在桌面环境下做手动回归

### 17.4 性能验证

- 验证玻璃层不会影响媒体网格滚动
- 验证 Inspector、menus、dialogs 的动画不会影响选中切换
- 验证 Performance Mode 与 Reduced Transparency 可明显降低视觉成本

## 18. 验收标准

设计与实现完成后，Megle 应达到以下状态：

1. 主工作区、设置页、插件页、任务页、导入流程和弹层使用同一套视觉 tokens 与材质层级。
2. Library 维持高密度资产管理效率，不因玻璃风格降低信息密度。
3. 自绘窗口、菜单、右键菜单、Popover、Dialog 和 Inspector 的视觉与交互语法一致。
4. 玻璃效果不会影响现有性能目标和内容层稳定性。
5. 存在可切换的 Performance Mode、Reduced Transparency 和 Reduced Motion。
6. Web/Docker 未来复用 React UI 时，只需替换桌面壳能力，不需重做页面设计。

## 19. 对当前仓库的直接影响

当前仓库已经具备早期骨架，但尚未形成完整设计系统：

- `apps/web/src/styles.css` 目前仍是单文件原型样式。
- `apps/web` 尚未接入正式的 tokens/primitives 层。
- `apps/desktop` 目前仍是普通 Electron 窗口，未进入自绘桌面壳阶段。
- 现有文档已经认可“玻璃主要用于 chrome 与控制层，不覆盖网格”，本设计将其升级为正式系统规则。

因此下一步不应直接“美化当前页面”，而应先建设设计系统基础设施，再让页面挂载到统一壳层。

## 20. 参考资料

- Apple: Adopting Liquid Glass
  - https://developer.apple.com/documentation/TechnologyOverviews/adopting-liquid-glass
- Apple Human Interface Guidelines
  - https://developer.apple.com/design/human-interface-guidelines
- Apple Human Interface Guidelines: Materials
  - https://developer.apple.com/design/human-interface-guidelines/materials
- Apple Human Interface Guidelines: Toolbars
  - https://developer.apple.com/design/human-interface-guidelines/toolbars
- Apple Human Interface Guidelines: Search fields
  - https://developer.apple.com/design/human-interface-guidelines/search-fields
- Apple Human Interface Guidelines: Sidebars
  - https://developer.apple.com/design/human-interface-guidelines/sidebars
- rdev/liquid-glass-react
  - https://github.com/rdev/liquid-glass-react
- Megle project docs
  - `docs/product-brief.md`
  - `docs/implementation-roadmap.md`
  - `docs/architecture.md`
  - `docs/final-solution.md`
  - `docs/component-library-review.md`
  - `docs/performance-plan.md`

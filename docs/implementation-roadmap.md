# Implementation Roadmap

## 已确认的 UI 基线

当前 UI 基线已确定，详见 `docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md`。
基础设施实施计划详见 `docs/superpowers/plans/2026-05-16-megle-ui-foundation.md`。
完整产品主计划详见 `docs/superpowers/plans/2026-05-16-megle-complete-product-plan.md`。

固定规则：

- 使用自绘的 frameless Electron desktop chrome。
- 整个应用采用分层亚克力毛玻璃设计（Layered Acrylic / Frosted Glass）。
- 亚克力与毛玻璃材质用于 chrome 和控制层，不覆盖大面积媒体网格和预览内容层。
- Library、Settings、Plugins、Tasks 必须共享同一套 app shell、tokens 和交互语法；Tasks 作为浮动工具抽屉 / Task Center，而不是永久右栏。

## 整体产品目标

目标不是只完成桌面壳或浏览原型，而是交付一个可长期演进的完整产品：

- Windows-first 桌面版主工作流闭环可用。
- 真实目录浏览、预览、筛选、元数据和文件操作完整。
- 任务、设置、插件管理和错误恢复进入统一 UI 体系。
- 高级格式、视频和插件扩展路径成立。
- 同一套 React UI 可继续复用到 Web/Docker。

## 整体执行顺序

UI 计划不是单独的一条支线，而是贯穿整个产品路线的横向主线：

1. 先完成 UI foundation，使后续所有产品页面都挂到统一的 shell、tokens 和 primitives 上。
2. 再推进真实目录浏览主路径，把 Library 变成可用工作区。
3. 随后把缩略图、预览、任务、筛选、文件操作和插件页逐阶段接入同一设计系统。
4. 最后完成 Web/Docker 复用、发布硬化和完整产品收尾。

## Phase 0: 性能门槛和技术决策

状态：已完成，2026-05-16。

输出：

- 技术栈定稿。
- SQLite 1M/5M 元数据压测。
- 前台读 + 后台写并发压测。
- 200 项分页 API 查询和 JSON 序列化压测。
- React/TanStack Virtual 百万级网格压测。
- 缩略图格式、320px 规则和生成队列压测。
- 缓存 WebP 预览切换压测。
- 真实文件重命名/移动/软删除一致性压测。

原始 JSON 归档在 `docs/performance-results/raw/2026-05-16/`。

## Phase 1: 决策落地和骨架

输出：

- monorepo 结构。
- Core API 草案。
- SQLite schema v0。
- UI 信息架构。
- UI 设计基线定稿。
- frameless desktop chrome 和共享 app shell 基础设施。
- `design-tokens` / `ui` 共享包与主题运行时。

建议目录：

```text
apps/
  desktop/
  web/
crates/
  core/
  media/
  indexer/
  thumbnails/
  fsops/
  plugins/
docs/
```

首版使用 Electron：

```text
apps/desktop      Electron main process
apps/web          React UI
crates/core       Rust Core Service
```

Phase 1 的 UI 重点：

- 建立 layered acrylic/frosted-glass tokens。
- 建立共享 UI primitives。
- 建立 Library / Settings / Plugins / Tasks 统一 app shell。
- 将当前早期样式原型迁移到正式设计系统边界。

## Phase 2: 可浏览真实目录

目标：

- 添加 root。
- 扫描目录。
- 写入 SQLite。
- 左侧目录树。
- 中间媒体网格。
- 真实路径打开预览。
- 将 Library 工作区挂到共享 app shell。
- 建立顶部工具栏、右侧 inspector、基础右键菜单和预览入口。

验收：

- 10 万文件可以打开和滚动。
- 未生成缩略图时也能稳定显示占位。

## Phase 3: 缩略图和预取

目标：

- 生成 tiny/grid/retina thumbnail。
- 实现视口优先级队列。
- 实现图片切换预取。
- 实现内存 LRU。
- 将 tile loading、preview transition、selection feedback 接入统一 UI 状态语法。

验收：

- 缓存命中时，目录首屏缩略图快速显示。
- 左右切图不被原图解码卡住。

## Phase 4: 文件监听和增量索引

目标：

- root watcher。
- 新增/删除/重命名/移动事件处理。
- watcher overflow 后局部重扫。
- 后台任务面板。
- 统一任务抽屉 / Task Center。

验收：

- 外部资源管理器修改文件后，Megle 能自动更新。

## Phase 5: 用户元数据

目标：

- 标签。
- 评分。
- 收藏。
- 备注。
- 基础筛选和排序。
- 搜索栏、筛选 chips、排序菜单与保存视图。

验收：

- 用户元数据只写 Megle 数据库，不修改原文件。

## Phase 6: 真实文件操作

目标：

- 重命名。
- 移动。
- 删除到回收站。
- 操作日志。
- 失败恢复。
- 危险确认弹层、操作进度和结果恢复界面进入统一交互体系。

验收：

- 文件系统和数据库在失败场景下保持一致。

## Phase 7: 视频和高级格式

目标：

- FFmpeg 视频 metadata。
- 视频 poster thumbnail。
- 常见视频预览播放。
- 接入更完整的图片解码器链。

验收：

- 常见图片、GIF、WebP、TIFF、HEIC、RAW、PSD、MP4、MOV、MKV、AVI 有合理降级策略。

## Phase 8: 插件框架

目标：

- plugin manifest。
- 插件设置页。
- decoder plugin 内部接口。
- action plugin 内部接口。
- Plugin Manager 纳入统一 app shell 和 design system。

验收：

- 可以安装/启用/禁用内部插件。
- 插件失败不影响主程序。

## Phase 9: Web/Docker 准备

目标：

- Core headless mode。
- Web auth。
- mounted root 管理。
- Dockerfile。
- server adapter。
- 在不重做设计系统的前提下复用 React UI。

验收：

- 同一个 React UI 可以通过浏览器访问。
- Docker 版可以索引挂载目录并显示缩略图。

## Phase 10: 产品硬化和发布准备

状态：Phase 10 功能已进入硬化阶段；2026-05-18 已补入亚克力毛玻璃交互、产品级窗口布局、浮动 Tasks 抽屉、点击预览和设计系统落地检查，详见 `docs/superpowers/plans/2026-05-18-megle-liquid-glass-interaction-completion.md`。

目标：

- 设置页、任务页、插件页和导入流程补齐完整产品细节。
- 完成桌面壳 polish、空状态、错误态、快捷键和无障碍收尾。
- 完成 Electron Windows 亚克力窗口材质、全局可操作控件鼠标反馈、菜单/Dialog/抽屉/Inspector 的统一亚克力毛玻璃交互语言。
- 完成产品级玻璃材质 primitive：独立折射/透镜层、SVG displacement filter、指针跟随高光、按压弹性、清晰内容层和无障碍降级。
- 完成圆角亚克力 app shell、图标式顶部导航、浮动 Tasks 工具抽屉、亚克力毛玻璃预览弹层和资源管理器式右键动作覆盖。
- 完成端到端回归、性能回归和手动验收清单。
- 形成 Windows 桌面版 release candidate。

验收：

- 主工作流、异常流和危险操作流全部经过验证。
- UI 设计系统覆盖完整产品而不是只覆盖主工作区。
- `npm test` 必须通过 `check:ui-design`，防止后续界面、菜单和交互退化成单纯换色，或把玻璃材质错误应用到媒体网格 / 预览内容层。
- 能给出可演示、可继续迭代、可发布的完整桌面产品版本。

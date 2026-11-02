# Mangoo Preview Pipeline Refactor Design

Updated: 2026-05-23

## 1. 背景

Mangoo 当前的预览链路仍然基于 Phase 7 的单一缩略图模型：

- Core 只维护 `grid_320` 一档缩略图状态。
- 持久化资产落在数据库旁的 `thumbnail-cache/` 文件树。
- `/api/media/{fileId}/thumbnail` 返回状态，`/thumbnail/blob` 读取实际 WebP 字节。
- Grid 依赖 `grid_320`。
- 右侧 `PreviewPanel` 当前走原图读取。
- 中央预览打开后直接走原图。

`tmp/perf/preview-pipeline-report.md` 的结论已经说明，这条链路的主要问题不是 resize 算法本身，而是：

1. 持久缓存形态不合理，百万级素材下会形成高体积小文件树。
2. 扫描期将大量时间消耗在预烘焙 `320` WebP 上。
3. 前端没有把 placeholder、小图预览和中央原图预览组织成统一的渐进式状态机。

本设计文档定义一次全量改穿的预览链路重构方案，覆盖 DB、Core API、Core 路由、前端加载语义、迁移与验证。

## 2. 设计目标

这次重构必须同时满足以下目标：

1. Grid 首帧可立即显示轻量占位，不再依赖缩略图任务先完成。
2. 右侧 `PreviewPanel` 只承担轻预览职责，使用 `grid_320` 级别资源即可。
3. 中央视图点开后最终必须使用原图查看，保持当前产品交互预期。
4. 持久小图资产改为数据库 BLOB，不再依赖 `thumbnail-cache/` 文件树作为运行时真相。
5. Core 和 Web 统一到一套预览目标语义，避免 grid、右侧预览、中央预览各走各的链路。
6. 保持当前已确认的 UI 契约：单击选中，双击 / Enter / Space 打开中央预览，Liquid Glass 仅作用于 chrome 与控制层。

## 3. 非目标

- 不引入 `retina_640` 或 `preview_1600` 作为新的产品预览档位。
- 不在本次重构中重新设计预览交互手势、布局骨架或视觉系统。
- 不把媒体内容区做成 Liquid Glass 材质。
- 不继续扩展磁盘 `thumbnail-cache/` 文件树。
- 不为了“预加载一切”而把中央原图预览重新做成预烘焙持久缓存。

## 4. 最终目标语义

本次重构后，预览目标只保留两类：

- `grid_320`
- `original`

这两个目标覆盖所有 UI 面：

- Grid tile：`placeholder -> grid_320`
- 右侧 `PreviewPanel`：`placeholder -> grid_320`
- 中央视图：`placeholder 或 grid_320 -> original`

其中：

- `grid_320` 是轻预览与列表浏览的统一持久小图目标。
- `original` 是中央查看模式的最终目标。
- placeholder 是首帧兜底数据，不是最终展示目标。

## 5. 数据模型变更

### 5.1 `media` 表

给 `media` 增加以下列：

- `preview_placeholder BLOB`
- `preview_placeholder_format TEXT NOT NULL DEFAULT 'image/webp'`

约束：

- 首发只要求写入极小占位 WebP。
- 占位数据是 UI 首帧资源，不要求高保真。
- 扫描期同步生成并写入，不单独创建后台任务。

### 5.2 `thumb_blobs` 表

新增持久化小图表：

```sql
CREATE TABLE thumb_blobs (
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  profile TEXT NOT NULL CHECK(profile IN ('grid_320')),
  data BLOB NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL,
  output_format TEXT NOT NULL CHECK(output_format = 'image/webp'),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(file_id, profile)
);
```

职责：

- 只保存 `grid_320` 持久小图。
- 替代旧 `thumbnail-cache/` 文件树。
- 为 Grid 和 `PreviewPanel` 提供稳定复用资产。

### 5.3 `thumbs` 表

现有 `thumbs` 从“磁盘缓存索引表”改成“预览目标状态账本”。

保留：

- `file_id`
- `profile`
- `state`
- `width`
- `height`
- `byte_size`
- `output_format`
- `error`
- `source_fingerprint`
- `updated_at`

新增：

- `served_by TEXT`

移除运行时语义：

- `cache_key`

说明：

- `thumbs` 继续记录目标状态，但不再把外部文件路径当成系统真相。
- `served_by` 用于记录最近一次响应命中的 source，便于诊断与测试。
- `profile` 这次仍只要求支持 `grid_320`，因为中央原图不需要在 `thumbs` 中持久化独立副本。

## 6. Core Source 与 Router 设计

### 6.1 Source 分层

Core 引入统一的预览资源 source 抽象，但本次只落与当前目标匹配的 4 个 source：

1. `MemLruSource`
2. `DbBlobSource`
3. `PlaceholderSource`
4. `OriginalSource`

不引入本次不会被 UI 使用的 `640/1600` 中间预览 source。

### 6.2 `MemLruSource`

职责：

- 缓存最近请求过的最终响应字节。
- key 为 `(file_id, target, source_fingerprint)`。
- 缓存 `grid_320` 和 `original` 请求结果。

用途：

- 列表回滚时快速命中 `grid_320`
- 中央预览来回切换时复用最近原图结果
- 邻居预取结果保留在内存中

### 6.3 `DbBlobSource`

职责：

- 从 `thumb_blobs` 读取 `grid_320` 持久小图。

用途：

- Grid tile 的主资源来源
- 右侧 `PreviewPanel` 的主资源来源
- 中央视图在原图尚未到达前，可用作稳定过渡图

### 6.4 `PlaceholderSource`

职责：

- 从 `media.preview_placeholder` 读取占位字节。

用途：

- Grid 与 `PreviewPanel` 的零 fetch 首帧
- 中央视图首次打开时的空白兜底

说明：

- placeholder 主要由前端首帧直渲染消费。
- Core 不需要把它当成 `/thumbnail/blob` 的最终响应主体。

### 6.5 `OriginalSource`

职责：

- 流式返回原始文件字节。

用途：

- 中央视图打开后的最终资源
- 视频与其他不适合小图放大的媒体类型
- 所有 UI 语义上的 `original` 目标请求

### 6.6 Router 行为

UI 目标语义统一使用：

- `grid_320`
- `original`

Router 规则：

- `grid_320`
  - 先看 `MemLruSource`
  - 再看 `DbBlobSource`
  - 无命中则触发后台 `grid_320` 任务并返回状态
- `original`
  - 先看 `MemLruSource`
  - 再走 `OriginalSource`

HTTP 端点映射：

- `grid_320` 通过 `/api/media/{fileId}/thumbnail` 与 `/thumbnail/blob`
- `original` 通过 `/api/media/{fileId}/preview`

中央预览的渐进原则不是让 Core 返回另一个中间目标，而是让前端先显示已有 placeholder 或 `grid_320`，然后独立发起 `original` 请求替换。

## 7. Core API Contract 变更

### 7.1 `MediaRecord`

新增：

- `previewPlaceholder`
- `previewPlaceholderFormat`

要求：

- `listMedia` 和 `getMedia` 都要返回这两个字段。
- UI 首帧不得再依赖 `"pending"` 文本占位。

### 7.2 `/api/media/{fileId}/thumbnail`

查询参数从 `profile` 改为 `target`：

- `grid_320`

说明：

- 本端点只承载持久小图目标。
- UI 语义上的 `original` 不走这里。

返回体继续使用 `ThumbnailResponse`，但语义更新为“目标请求状态”：

- `fileId`
- `target`
- `state`
- `width`
- `height`
- `byteSize`
- `outputFormat`
- `servedBy`
- `error`
- `updatedAt`

### 7.3 `/api/media/{fileId}/thumbnail/blob`

保留端点，但查询参数同样改为 `target`。

本次只要求支持：

- `target=grid_320`

返回响应头新增：

- `x-mangoo-served-by`

### 7.4 `/api/media/{fileId}/preview`

保持“原始文件直通”语义不变。

主 UI 中央视图的 `original` 目标由这条端点承载；contract 本身不再被描述成 placeholder 或 queued 端点。

### 7.5 `packages/core-client`

必须同步更新：

- `generated-contract.ts`
- `client.ts`
- contract check
- tests

Web 侧统一通过 core-client 消费：

- `getThumbnail(...target=grid_320)`
- `getThumbnailBlob(...target=grid_320)`
- `getPreviewBlob(fileId)` 作为 `original` 目标的唯一传输入口

## 8. 后台任务模型

### 8.1 扫描期职责

扫描 worker 只新增一项同步职责：

- 生成并写入 `preview_placeholder`

扫描期不再依赖全量持久小图先完成，目录在写入 `media` 行后即可浏览。

### 8.2 缩略图任务职责

缩略图后台任务只负责：

- 生成 `grid_320`
- 把字节写入 `thumb_blobs`
- 更新 `thumbs` 中对应 `grid_320` 的状态账本

它不负责：

- 中央原图预览
- 原图持久缓存
- 额外中间档位

### 8.3 原图预览职责

中央原图预览不进入持久后台任务体系。

它通过：

- 原图流请求
- 前端 abort
- 相邻项原图预取
- `MemLruSource`

来提升切换体验。

## 9. Web 渲染与交互时序

### 9.1 Grid tile

显示顺序：

1. 直接渲染 `previewPlaceholder`
2. 进入视口并稳定后请求 `grid_320`
3. 到达后替换为真实小图

离开视口：

- abort 在途请求

### 9.2 `PreviewPanel`

右侧 `PreviewPanel` 只使用 `grid_320`：

- 首帧使用 `previewPlaceholder`
- 后续请求 `grid_320`
- 不请求原图

目标：

- 始终是轻预览
- 不承担高保真查看职责

### 9.3 中央视图

中央视图点开后要求最终使用原图查看。

显示顺序：

1. 先显示 placeholder 或已拿到的 `grid_320`
2. 立即请求原图
3. 原图就绪后替换显示

要求：

- 打开时不空白
- 最终必须切到原图
- 切换当前项时立即 abort 旧原图请求

### 9.4 预取

中央预览打开时：

- 预取当前项前后各 1 张原图
- 预取结果进入 `MemLruSource`

不对 `PreviewPanel` 或 Grid 触发原图预取。

### 9.5 状态展示

前端不再以 `"pending"` / `"queued"` 文字作为主显示内容：

- Grid：以 placeholder 为主，失败时显式提示
- `PreviewPanel`：以 placeholder / `grid_320` 为主
- 中央视图：允许 placeholder 或 `grid_320` 过渡，但原图成功后必须替换

## 10. 迁移策略

### 10.1 Schema migration

新增 migration，完成：

1. `media` 增加 placeholder 列
2. 创建 `thumb_blobs`
3. 调整 `thumbs` 语义与字段

### 10.2 旧缓存导入

启动期执行一次性导入：

- 扫描旧 `thumbnail-cache/`
- 将能识别的 `grid_320` WebP 导入 `thumb_blobs`
- 成功导入后删除旧文件

无法导入的旧缓存：

- 直接丢弃
- 不阻塞启动

### 10.3 运行时真相切换

迁移完成后：

- 运行时不再读取 `thumbnail-cache/`
- 旧文件树只存在于迁移代码路径中

## 11. 验证要求

### 11.1 Contract / check

必须更新并通过：

- `npm run check:core-api`
- `npm run check:core-client`
- `npm run check:web`
- `npm run check:ui-design`
- `npm run check:schema`

### 11.2 Rust tests

至少新增或更新以下覆盖：

- migration 升级测试
- 旧 `thumbnail-cache` 导入测试
- `thumb_blobs` 读写测试
- `grid_320` 任务状态测试
- placeholder 写入与读取测试
- 中央原图链路不依赖缩略图任务的测试
- stale source / retry / cancel / attempt_generation 继续保持

### 11.3 Web behavior

至少验证：

- Grid `placeholder -> grid_320`
- `PreviewPanel` 只请求 `grid_320`
- 中央视图打开后最终切到原图
- 切换项目时原图请求可 abort
- 邻居原图预取不污染错误态

### 11.4 GUI smoke

要求至少一次真实 GUI smoke：

- `npm run dev`
- 配合 `MANGOO_AUTO_ADD_ROOT`
- 观察首帧 placeholder、右侧轻预览、中央原图替换是否符合预期

## 12. 交付判定

本次重构只有在以下条件同时成立时才算完成：

1. DB 可从当前仓库状态升级到新预览模型
2. 旧 `thumbnail-cache/` 不再是运行时依赖
3. Grid、`PreviewPanel`、中央预览统一到 `grid_320 | original` 目标语义
4. `npm test` 全绿
5. 至少一轮真实 GUI smoke 通过

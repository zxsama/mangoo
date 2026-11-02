# Mangoo Dynamic Priority Disclosure Scan Design

Updated: 2026-05-24

## 1. 背景

Mangoo 当前预览链路已经完成了以下方向：

- `grid_320` 运行时真相已转到 `thumb_blobs`。
- 右侧 `PreviewPanel` 使用轻预览路径。
- 中央视图最终使用原图。
- placeholder、thumbnail blob、original preview 的基本链路已经可用。

但当前扫描与可浏览性仍然不符合目标：

1. 新增根目录后，要等很久才能真正开始浏览。
2. 扫描进行中虽然目录树按钮可点击，但当前文件夹内容不会按批次持续刷新，体感上仍像“不能切文件夹”。
3. 扫描线程仍同步执行尺寸探测与 placeholder 生成，导致“去掉 320 全量预生成”后总耗时仍与旧方案接近。
4. 当前视图、右侧轻预览、中心原图预览还没有形成完整的动态优先级调度。

用户当前要求已经明确为：

- 最快看到图片清晰的预览。
- 点开图片后原图尽快显示出来。
- 动态优先级，因为用户会频繁切换文件夹。
- 接受“当前打开的文件夹和可见区优先，其他文件夹后台延后”。
- 从全局调度角度处理优先级，不是只改单个环节。
- 要考虑同一张图片在双通道之间切换时的刷新一致性，不能出现旧状态回写覆盖新状态。
- 当前可见区缩略图必须最快刷新。
- 当前点击项的缩略图必须高于普通可见区。
- 当前可见区下方即将滚入视口的区域需要预刷新，优先级略低于当前可见区，但高于后台刷新。
- 整体速度略微降低可以接受，但当前视图的主观速度必须优先。

## 2. 设计目标

本设计的目标不是继续压缩旧扫描批次，而是将扫描与预览生成彻底解耦，形成“当前视图优先”的系统。

必须同时满足：

1. 新增根目录后，目录与文件基础信息应尽快变成可浏览数据。
2. 扫描进行中必须允许切换已存在和新出现的文件夹。
3. 当前文件夹内容必须在扫描进行中增量出现，而不是只在任务 `succeeded` 后整体刷新。
4. 当前文件夹可见区应优先获得清晰 `grid_320` 预览。
5. 当前点击项的轻预览应高于普通可见区缩略图。
6. 当前可见区下方即将滚入视口的区域应做预刷新，且优先级高于后台任务。
7. 中央视图必须优先获得当前项 original，并支持邻居 original 预取与复用。
8. 后台其他文件夹的预览生成必须自动降级，不得阻塞当前视图。
9. 同一张图片在后台 `root_scan`、前台互动扫描、可见区缩略图刷新之间切换时，必须保证只发布最新状态。

## 3. 非目标

- 不在本次设计中重做 Liquid Glass 视觉系统。
- 不引入新的中间预览档位（例如 640 / 1600）。
- 不让 root scan 重新承担 `grid_320` 或 original 级别预处理。
- 不一次性重写所有 watcher / task 持久化架构。
- 不为了“披露式扫描”牺牲当前已有的 DB-blob runtime truth。

## 4. 根因分析

当前体验问题不是单点 bug，而是三个系统性耦合：

### 4.1 扫描线程职责过重

`scan_root_with_options(...)` 在批量写入 `files/media` 后，会同步调用 `probe_image_dimensions(...)`，后者又对每张图片做：

- header 维度探测
- `generate_preview_placeholder(...)`
- `update_media_preview_placeholder(...)`

这意味着扫描线程仍然承担了完整图像解码与编码工作。

### 4.2 批量可见性仍偏向“写完整批再看”

即使默认 `DEFAULT_SCAN_WRITE_BATCH_SIZE` 已经降低到 10，数据可见性仍取决于：

- 当前批次是否 flush
- flush 后是否完成同步 probe
- 前端是否主动重新请求当前文件夹内容

系统仍然不是“发现 -> 立即可看”。

### 4.3 前端刷新策略仍偏向“扫描完成后整体刷新”

前端扫描中主要轮询 `tasks`，并在任务从非完成态变成 `succeeded` 时整体 `loadLibrary()`。

当前缺失：

- 扫描进行中对当前 root / 当前 folder 的增量刷新
- 当前视图变化驱动预览优先级重排
- 当前文件夹与后台文件夹之间的强优先级差异

## 5. 总体方案

采用：

`Background root ingest + interactive folder ingest + viewport-driven preview priority`

这意味着系统分成三层：

1. **Background ingest 层**
   - 负责整棵 root 的持续发现
   - 继续使用小批量写入，后台默认批次保持 `10`
2. **Interactive ingest 层**
   - 只服务当前正在查看的 folder
   - 用单文件或极小批次披露当前 folder 内容
   - 在 folder 切换时立即抢占后台披露优先级
3. **Preview 层**
   - 负责 placeholder 补齐、`grid_320`、original prefetch/reuse
   - 完全由当前 UI 状态驱动优先级

扫描任务的完成定义不再是“图也准备好了”，而是“库已经变成可浏览”。

## 6. 数据边界

### 6.1 Ingest 层负责的数据

- `roots`
- `folders`
- `files`
- `media.kind`
- `media.size`
- `media.mtime`
- 其他无需完整 decode 的基础元信息

它的职责是让用户尽快看到：

- 文件夹树
- 当前文件夹中的文件名与基础项目

### 6.2 Preview 层负责的数据

- `media.preview_placeholder`
- `media.preview_placeholder_format`
- `thumbs` 目标状态
- `thumb_blobs` 的 `grid_320`
- 中心 original preview 的内存缓存/预取

Preview 层不再作为 root scan 主线程的一部分同步运行。

## 7. 动态优先级模型

预览与披露任务按当前 UI 状态动态重排，优先级从高到低为：

1. 当前中心预览的 original
2. 当前点击项的轻预览 / `grid_320`
3. 当前视口内可见 tile 的 `grid_320`
4. 当前视口下方即将滚入区域的 `grid_320`
5. 当前视口上方近邻区域的 `grid_320`
6. 当前正在查看 folder 的条目披露
7. 当前 folder 内其他未显示条目的 `grid_320`
8. 其他文件夹后台补齐与后台扫描写入

行为要求：

- 切换文件夹时，新文件夹的任务立即升权，旧文件夹后台任务立即降权。
- 打开中心预览时，当前 original 抢占所有低优先级 thumbnail 工作。
- 点击某个 tile 时，该 tile 的轻预览优先级立刻高于普通可见区。
- 滚动时，新的视口内项目立即成为最高缩略图优先级；将要滚入视口的项目成为次高优先级。
- 关闭中心预览时，系统重新把可见区 `grid_320` 作为前台任务。
- 长时间不可见、非当前文件夹的任务可以被取消或直接丢弃重建。

## 8. Ingest 扫描策略

### 8.1 root scan 只做快路径

root scan 只做：

- 目录发现
- 文件发现
- `files/media` 小批量写入
- reconciliation / mark scanned

它不再同步执行：

- placeholder 生成
- 图片尺寸 probe
- `grid_320` 生成

### 8.2 批量提交

默认写入批次继续保持小值，目标是尽快让当前文件夹进入可查询状态。

这次已将默认值从 1000 调整到 10，但真正的收益来自“去掉批后同步 decode”，不是仅靠批量大小。

### 8.3 双通道 ingest

系统需要并行保留两种 ingest 通道：

1. **后台 `root_scan`**
   - 负责整棵 root 的顺序发现
   - 批次保持 `10`
   - 保证长时间运行时整体最终一致
2. **前台 `interactive_folder_scan`**
   - 由当前选中的 folder 触发
   - 只扫描当前 folder 及当前窗口布局会用到的直接内容
   - 披露粒度为单文件或极小批次
   - 在切换 folder 或滚动导致可见窗口变化时，立即重新计算前台范围

前台通道不是后台通道的替代，而是覆盖当前视图的加速层。

### 8.4 目录树披露

目录一旦发现并落库，就应允许：

- 展开
- 选中
- 请求其 children

不要求整棵 root 扫描完成。

## 9. 前端增量刷新策略

### 9.1 当前 root / 当前 folder 刷新

扫描进行中，前端不应只等任务 `succeeded` 才整体 `loadLibrary()`。

新的规则是：

- 如果当前 root 的 `root_scan` 任务仍在 `pending/running`
- 并且当前视图停留在 `library`
- 则按节流策略持续重新拉取：
  - 当前 folder 的 `listMedia`
  - 当前展开链上的 `listFolderChildren`

### 9.2 文件夹切换

切换文件夹时：

- 立即更新 `selectedFolderId`
- 立即请求目标文件夹的 `listMedia`
- 不允许旧文件夹的后台任务阻塞这个请求

如果目标文件夹还没完全写入库：

- 内容区进入“增量发现”状态
- 随后继续增量刷新当前 folder，直到数据逐步出现

### 9.3 视口驱动的前台范围

前端必须持续上报当前窗口布局中的三个集合：

1. 当前点击项
2. 当前可见区 tile
3. 当前可见区下方的预刷新区

预刷新区数量应可配置，例如：

- `visible_ahead_rows`
- 或换算后的 `visible_ahead_item_count`

默认值不追求极小，而是优先保证主观流畅度。上方近邻区可以保留较小范围，主要用于回滚时减少冷启动。

## 10. Preview 队列拆分

### 10.1 `placeholder_fill`

职责：

- 给旧库或新落库但无 placeholder 的图片补首帧占位

优先级：

- 低于可见区 `grid_320`
- 高于其他文件夹后台任务仅在当前 folder 内成立

### 10.2 `grid_thumbnail`

职责：

- 生成 `grid_320`

用途：

- Grid tile
- `PreviewPanel`

优先级：

- 当前点击项最高
- 当前可见区次高
- 当前可见区下方预刷新区再次之
- 其他全部降级

### 10.3 `original_prefetch`

职责：

- 中心预览前后邻居的 original 预取与复用

限制：

- 不落持久存储
- 只保存在小型内存缓存中

## 11. 双通道一致性与发布规则

### 11.1 同图双通道竞争

同一张图片可能同时出现在：

- 后台 `root_scan`
- 前台 `interactive_folder_scan`
- 当前可见区缩略图状态请求
- 当前点击项轻预览请求

系统必须假设这些请求可以乱序完成。

### 11.2 唯一真相

缩略图与媒体状态仍以当前 DB / API 返回的最新 source identity 为准，至少包括：

- `file_id`
- `mtime`
- `size`
- 必要时 `updated_at` / 内容签名

任何旧请求返回时，只要 identity 不再匹配，就必须丢弃，不得覆盖当前 UI。

### 11.3 发布规则

- 后台 `root_scan` 不得覆盖前台已发布的更新状态。
- 前台 `interactive_folder_scan` 发现当前 folder 内容时，可以先发布最小媒体行，再由 preview 层继续清晰化。
- 当前点击项 / 可见区的 thumbnail state 如果已经推进到更高状态，旧的 `queued/pending` 回写必须被拒绝。
- 切 folder 后，旧 folder 的 in-flight 结果必须按 selection token / request generation 失效。

## 12. 取消与抢占

- `placeholder_fill` 可随时取消，稍后重建
- `grid_thumbnail` 对非当前视图项可取消或延后
- `original_prefetch` 在切图时立即取消旧邻居
- 当前中心原图请求除非切图，否则不应让位给低优先级 thumbnail
- `interactive_folder_scan` 在 folder 切换时应立即取消或降级旧 folder 工作
- 视口下方预刷新区在继续下滚时可升权，在反向离开时可降权或丢弃

## 13. 三阶段落地顺序

### 阶段 1：先恢复可浏览性

目标：

- root scan 去掉同步 placeholder / probe
- 当前 folder 扫描中可切换、可增量看到内容
- 引入前台 `interactive_folder_scan`，保证当前 folder 内容比后台更早披露

完成标志：

- 新加根目录后不必等扫描完成才能进入当前文件夹
- 扫描中切其他文件夹时，内容区立即切换

### 阶段 2：再恢复当前视图优先清晰化

目标：

- 当前点击项优先生成轻预览
- 当前文件夹可见区优先生成 `grid_320`
- 当前可见区下方预刷新区提前清晰化
- 右侧 `PreviewPanel` 立即跟随当前选中项变清楚

完成标志：

- 当前点击项先清晰，可见区紧随其后
- 当前可见区下方预刷新区先于后台变清晰
- 当前可见区先清晰，后台其他文件夹延后
- 大面积 `queued` 不再作为主视觉

### 阶段 3：最后优化中心原图速度感

目标：

- 中心 current original 最高优先
- previous/next original 小范围预取

完成标志：

- 双击后原图尽快可见
- 连续切图时明显快于完全冷启动

## 14. 测试与验收

### 14.1 真实目录基准

使用：

- `G:\AI_Painter\stable-diffusion\stable-diffusion-webui\outputs`

验收指标：

- 从 add root 到“当前文件夹第一批内容可见”的时间
- 扫描中切文件夹是否立即生效
- 同一时刻当前点击项、当前可见区、预刷新区、后台区的清晰化顺序是否符合预期
- 当前可见区清晰预览是否优先于后台文件夹
- 双击后 original 显示时间

### 14.2 行为验收

必须满足：

1. 扫描中可以切换到其他文件夹
2. 已切入的当前文件夹在扫描中增量出现内容
3. 当前点击项先清晰，随后是当前可见区，再随后是预刷新区，后台最后
4. 同一图片在双通道切换时不会出现旧状态覆盖新状态
5. 右侧永远使用轻预览
6. 中央视图最终使用 original

### 14.3 回归保护

- Rust tests：
  - flush 后数据立即可查
  - scan 不再绑定 placeholder 同步生成
  - interactive folder ingest 不会被后台 root ingest 旧结果覆盖
- Web checks：
  - 当前 folder 增量刷新
  - 当前视图优先级驱动请求
  - 当前点击项 / 可见区 / 预刷新区优先级顺序
  - 导航与滚动导致的旧请求失效
- Real smoke：
  - 使用 `outputs` 目录计时

## 15. 风险与边界

### 15.1 接受的权衡

- 新增根目录时，不保证全库第一时间都清晰
- 优先保证当前正在看的内容
- 为了保证当前视图主观速度，允许整体后台吞吐略微下降
- 前台双通道与更多轮询会增加一定请求频率，这是本次接受的成本

### 15.2 本次不做

- Core 侧复杂多 worker 持久化优先级系统
- 全局原图大缓存
- 扫描过程中对所有旧库一次性 placeholder 回填

这些可以后续继续演进，但不是这次把体验从“不能切、全是 queued、很慢”拉回来的前提。

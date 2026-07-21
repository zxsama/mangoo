# Megle Architecture

## 总体形态

采用三层结构：

1. Desktop Shell
   - Windows 桌面壳。
   - 负责窗口、菜单、托盘、拖放、系统文件对话框、快捷键。

2. Web UI
   - React + TypeScript。
   - 负责目录树、媒体网格、预览器、属性面板、设置页和插件页。
   - UI 不直接访问文件系统，全部通过 Core API。

3. Core Service
   - 独立后台核心服务。
   - 负责索引、数据库、缩略图、搜索、文件监听、文件操作、插件运行时。
   - Windows 桌面版由 Shell 启动本地 Core Service。
   - 未来 Web/Docker 版复用同一个 Core Service，以 HTTP API 暴露能力。

最终技术组合：

- Desktop Shell: Electron。
- UI: React + TypeScript。
- Core Service: Rust。
- Database: SQLite WAL mode。
- Media Processing: FFmpeg + libvips/ImageMagick/WIC/LibRaw 的解码器抽象层。

最终建议：Electron + React + Rust Core。

理由：

- Electron 的 Chromium 渲染环境稳定，适合做接近 Eagle 的复杂媒体网格和液态玻璃 UI。
- Core Service 独立后，Electron 只承担界面，不参与重型索引和解码。
- Rust Core 可复用于未来 Docker/Linux 服务端。
- 后续如果要减小体积，可以保留 Core/UI，再评估替换 Shell 为 Tauri。

## 进程模型

```text
Megle.exe
  |
  |-- UI Renderer
  |     - React
  |     - virtual grid
  |     - preview viewer
  |
  |-- Desktop Main Process
  |     - window lifecycle
  |     - native dialogs
  |     - app updates
  |
  |-- megle-core.exe
        - API server on localhost / named pipe
        - SQLite
        - index workers
        - thumbnail workers
        - watcher workers
        - file operation workers
        - plugin host
```

Core API 首选本地 HTTP 或 named pipe。为了未来 Web/Docker，业务 API 设计为 HTTP/JSON 或 HTTP + streaming，桌面端再包一层安全 token 和本机访问限制。

## Unified Notification Center

统一通知中心按“库内事实、桌面跨库镜像、preload 类型边界、Web 展示”分层：

```text
Core library DB
  notification_operation_heads + notification_outbox
          |
          v
Desktop main
  userData/notifications.sqlite durable all-library mirror
  bounded local process-log buffer
          |
          v
Preload @megle/notification-contract
  snapshots + paged history + actions + subscriptions
          |
          v
Web notification popover/window
```

- Core 在每个 library DB 内事务性更新 operation head，并向 `notification_outbox` 写入带 revision 的事件。Desktop 只有在持久化镜像成功后才确认 Outbox sequence，因此切库、重启和短暂故障不会丢失历史。
- Desktop 的 `notifications.sqlite` 聚合所有已打开库的 operation-level 状态与历史；本地 Core/Desktop/renderer 进程日志进入有界会话缓冲区，不写回 media library DB。
- Preload 暴露类型化 compact snapshot、分页 history/facets、cancel/retry action、change subscription，以及 process-log snapshot-plus-subscribe。Web 不直接请求 notification Core endpoint，也不轮询通知。
- 缩略图完成/失败按 library 聚合，禁止每个 thumbnail 生成一条通知。每个后台任务必须提供稳定 logical grouping key 和 presentation class，确保高频进度只更新同一个 operation head。

## 数据模型

几百万文件不能简单把完整路径重复写进一张大表。建议拆分目录和文件。

核心表：

- `roots`
  - `id`
  - `path`
  - `display_name`
  - `enabled`
  - `created_at`
  - `last_scan_at`

- `folders`
  - `id`
  - `root_id`
  - `parent_id`
  - `name`
  - `path_hash`
  - `mtime`
  - `status`

- `files`
  - `id`
  - `root_id`
  - `folder_id`
  - `name`
  - `ext`
  - `size`
  - `mtime`
  - `ctime`
  - `file_key`
  - `status`

- `media`
  - `file_id`
  - `kind`
  - `width`
  - `height`
  - `duration_ms`
  - `codec`
  - `orientation`
  - `has_alpha`
  - `dominant_color`
  - `phash`
  - `metadata_status`

- `user_metadata`
  - `file_id`
  - `rating`
  - `favorite`
  - `note`
  - `updated_at`

- `tags`
  - `id`
  - `name`
  - `color`

- `file_tags`
  - `file_id`
  - `tag_id`

- `thumbs`
  - `file_id`
  - `profile`
  - `cache_key`
  - `width`
  - `height`
  - `byte_size`
  - `state`
  - `updated_at`

- `file_operations`
  - `id`
  - `operation`
  - `source_path`
  - `target_path`
  - `status`
  - `created_at`
  - `error`

关键索引：

- `folders(root_id, parent_id, name)`
- `files(folder_id, name)`
- `files(folder_id, mtime)`
- `files(ext)`
- `media(kind, width, height)`
- `user_metadata(rating, favorite)`
- `file_tags(tag_id, file_id)`

查询必须使用 keyset pagination，不用大 offset 翻页。

## 缩略图系统

几百万媒体文件下，缩略图是性能核心。

推荐缩略图层级：

- `tiny`: 96px，用于极密网格和快速占位。
- `grid`: 短边 320px，用于普通网格。
- `retina`: 短边 640px，用于高 DPI 网格和快速预览过渡，MVP 后再做。
- `preview`: 1600px，用于大图预览的首屏快速显示。

`grid` 规则：

- 原图短边小于 320px 时，不生成独立 `grid` 缩略图。
- 数据库记录 `skipped_small` 状态，UI 直接使用原图引用或低成本路径。
- `skipped_small` 只适用于 UI 可直接显示的源格式；RAW/PSD/HEIC 等不可直接显示格式仍需生成 WebP。
- 原图短边达到 320px 或以上时，生成短边 320px 的 `grid` 缩略图。
- 所有生成类缩略图统一保存为 WebP：`.webp` / `image/webp`。
- 视频 poster 也保存为 WebP。

缓存策略：

- 原图不复制。
- 缩略图和预览图放在 Megle cache 目录。
- 缓存 key 基于 `root_id + folder_id + file name + size + mtime` 或稳定文件身份。
- 缩略图生成采用优先队列：
  - 当前视口最高优先级。
  - 当前选中项前后 N 个次高优先级。
  - 后台补齐最低优先级。

存储策略：

- MVP 可用按 hash 分片的文件缓存，例如 `ab/cd/cache-key.webp`。
- 百万级正式版建议使用 thumbnail pack：
  - 多个 128MB 到 512MB pack 文件。
  - SQLite 记录 `pack_id + offset + length`。
  - 追加写入，后台压缩和清理。
  - 减少 NTFS 上几百万小文件带来的元数据压力。

## 索引流程

初始扫描：

1. 用户添加 root。
2. Core 快速枚举目录树，先入库路径和基础文件信息。
3. UI 立即可浏览已发现的文件。
4. 后台逐步读取媒体尺寸、EXIF、视频时长、颜色、缩略图。
5. 进度按 root/folder 展示，允许暂停。

增量更新：

- Windows 使用文件系统 watcher 监听 root。
- watcher 事件只作为提示，不作为唯一事实。
- 事件溢出、网络盘不稳定、休眠恢复后必须触发局部重扫。
- NTFS 上可后续接入 USN Journal 提高大规模重命名/移动检测能力。

## 预览与切图

切换不卡顿的原则：

- UI 切换选中项时只改变状态，不等待原图解码。
- 先显示 `preview` 或 `retina` 缩略图。
- 后台加载原图或视频流。
- 选中项前后各预取 5 到 20 个媒体的元数据和预览图。
- 解码队列区分 interactive 和 background。
- interactive 任务可以取消或抢占 background 任务。
- 内存中保留当前项、前后项、最近访问项的解码结果。

超大图策略：

- 不在 UI 线程直接解码原始巨图。
- 先展示缓存预览图。
- 对大图生成多级预览或 tile pyramid。
- 缩放到 100% 以上时再按需加载局部 tile。

视频策略：

- 用 FFmpeg 提取 poster frame 和基础元数据。
- 网格只显示 poster thumbnail。
- 预览器使用原视频路径或 Core streaming。
- 后续可增加低码率 proxy，但首版不强制。

## 文件操作

因为 Megle 映射真实目录，文件操作必须谨慎。

重命名：

- 先检查目标名合法性和冲突。
- 执行文件系统 rename。
- 成功后在同一事务内更新数据库。
- 失败时保留旧数据库状态。

移动：

- 同卷优先 rename/move。
- 跨卷使用 copy + verify + delete 到回收站或原子替代策略。
- 移动大量文件时使用后台任务和进度面板。

删除：

- Windows 桌面版默认删除到回收站。
- 永久删除必须二次确认。
- 删除成功后将数据库记录标记为 deleted，再由后台清理索引和缓存。

操作日志：

- 每个真实文件操作写入 `file_operations`。
- Desktop 将操作组同步到跨库通知历史；统一通知中心展示进度、结果和失败项。
- 后续可以做撤销，但首版至少要可追踪。

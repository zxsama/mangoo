# Open Source Stack Review

Updated: 2026-05-16

## 结论

为了降低开发成本，同时保住几百万级媒体库的性能目标，推荐路线是：

```text
Electron + React + TypeScript UI
Rust Core Service
SQLite WAL
libvips / FFmpeg / optional decoder plugins
TanStack Virtual
```

核心判断：

- UI 层选择 Electron 是为了开发效率、Chromium 一致性和接近 Eagle 的复杂交互能力。
- 性能关键路径不能放在 Electron/Node 主进程里，必须放进 Rust Core。
- SQLite 足够承载百万级媒体元数据，但必须使用 WAL、复合索引、批量事务、单写多读和 keyset pagination。
- 缩略图流水线比“支持格式数量”更重要。首版应优先稳定支持常见格式，长尾格式通过隔离插件补齐。
- 文件监听只能作为增量提示，不能作为唯一事实。大规模库需要 watcher + 局部重扫，后续可做 NTFS USN Journal 优化。
- 液态玻璃视觉不能覆盖大面积媒体网格，只适合工具栏、侧栏、浮层和设置页。

## 推荐库清单

### Shell 和 UI

| 模块 | 推荐 | 作用 | 取舍 |
| --- | --- | --- | --- |
| Desktop shell | Electron | Windows 桌面壳、窗口、菜单、托盘、拖放 | 体积和内存较高，但开发成本低，Chromium 行为稳定 |
| Alternative shell | Tauri | 后续轻量壳替代 | 适合以后优化包体；首版不建议先承担 WebView2 差异 |
| UI framework | React + TypeScript | 主 UI | 与未来 Web/Docker UI 复用 |
| Build tool | Vite | Web UI 构建 | 简单、快、生态成熟 |
| Virtual grid | TanStack Virtual | 百万级列表/网格虚拟滚动 | 只渲染视口内容，避免全量 DOM |
| Deep zoom viewer | OpenSeadragon | 超大图 tile 预览，可后续接入 | 不作为首版普通预览基础 |
| WebGL renderer | PixiJS | 可选，用于极高密度画布网格实验 | 会提高复杂度，先用 DOM/CSS grid + virtualization |

推荐决策：

- 首版用 Electron，不先用 Tauri。
- 首版网格用 TanStack Virtual，不手写滚动引擎。
- 只有当 DOM 网格在 100 万测试库下无法达标时，再评估 PixiJS/Canvas 网格。

### Core Service

| 模块 | 推荐 | 作用 | 取舍 |
| --- | --- | --- | --- |
| Runtime | Rust | 索引、缩略图、查询、文件操作 | 开发成本高于 Node，但避免后期重写 |
| Async runtime | Tokio | API、队列、IO 调度 | Rust 事实标准 |
| HTTP API | Axum | 本地 API 和未来 Docker/Web API | 比把所有能力绑死在 Electron IPC 更利于复用 |
| Serialization | Serde | API 和配置 | Rust 生态标准 |
| Logging | tracing | 结构化日志 | 对后台任务和插件排错重要 |
| CPU workers | rayon | 图片处理、hash、批量计算 | 与 Tokio IO 队列分开，避免互相阻塞 |

推荐决策：

- Core 以独立进程 `mangoo-core.exe` 运行。
- 桌面版通过 localhost HTTP 或 named pipe 调用 Core。
- API 从第一天按未来 Web/Docker 设计，不把业务逻辑写进 Electron main。

### Database 和 Search

| 模块 | 推荐 | 作用 | 取舍 |
| --- | --- | --- | --- |
| Database | SQLite | 媒体索引、目录树、标签评分 | 嵌入式、部署简单，适合本地库 |
| SQLite binding | rusqlite | Rust SQLite 访问 | 同步 API 简单，适合单写队列模型 |
| Full text | SQLite FTS5 | 文件名、路径、备注、标签基础搜索 | 首版够用 |
| Advanced search | Tantivy | 后续全库复杂全文搜索 | 不放进首版关键路径 |
| Vector search | 暂缓 | AI 搜索 | 后续插件或独立索引 |

推荐决策：

- 首版只用 SQLite + FTS5。
- 使用单 writer queue 批量事务写入，读请求走只读连接。
- 查询必须 keyset pagination，禁止大 offset 分页。
- 不建议首版引入 PostgreSQL、RocksDB、DuckDB。它们会增加部署和一致性成本。

### File System

| 模块 | 推荐 | 作用 | 取舍 |
| --- | --- | --- | --- |
| Directory traversal | jwalk 或 walkdir | 递归扫描目录 | jwalk 可并行；walkdir 更简单稳定 |
| Ignore rules | ignore | 可选，处理忽略规则 | 若支持 `.gitignore` 类语义再引入 |
| File watcher | notify | 文件系统事件 | watcher 事件不可靠时必须重扫 |
| Recycle bin | trash-rs 或 windows-rs | 删除到回收站 | 首版可用 trash-rs，复杂行为用 windows-rs |
| Windows APIs | windows-rs | 文件属性、Shell、回收站、文件 ID | Windows 深度集成必备 |
| File identity | same-file + windows-rs | 判断同一文件、追踪移动 | NAS/exFAT 上不能完全依赖 |
| Hash | BLAKE3 | 快速内容 hash | 不要默认对所有大文件全量 hash，按需或后台做 |

推荐决策：

- 首版扫描先用 walkdir 或 jwalk；如果初始扫描吞吐不足，再切 jwalk 并行遍历。
- watcher 只做事件提示，所有事件最终通过 stat/局部重扫确认。
- 删除默认进回收站，永久删除必须单独确认。

### Media Processing

| 模块 | 推荐 | 作用 | 取舍 |
| --- | --- | --- | --- |
| Common image thumbnails | libvips | 高性能低内存缩略图 | 首选主路径 |
| Node image alternative | sharp | 基于 libvips 的 Node 库 | 如果做 Node 原型很快；正式 Core 不建议依赖 Node |
| Rust image fallback | image crate | 基础格式解码和简单处理 | 生态纯 Rust，但格式和性能不是全部场景最佳 |
| Resize | fast_image_resize | 高性能 resize | 可用于已解码像素的快速缩放 |
| Video metadata/thumb | FFmpeg | 视频元数据、首帧、预览 | 需要认真处理 LGPL/GPL 构建和分发 |
| EXIF | exif-rs 或 ExifTool sidecar | 元数据读取 | ExifTool 覆盖广但外部进程成本高 |
| RAW | LibRaw plugin | RAW 缩略图/预览 | 建议插件化，不进首版主路径 |
| HEIC/AVIF | libheif plugin | HEIC/AVIF | 编解码授权和依赖复杂，插件化 |
| Long-tail formats | ImageMagick plugin | PSD/TIFF/奇异格式兜底 | 支持广但安全和性能风险更高，必须隔离 |

推荐决策：

- 常见图片主路径：libvips。
- 视频主路径：FFmpeg sidecar/子进程。
- RAW/HEIC/PSD 等高级格式：decoder plugin，失败不影响主程序。
- 不要让 ImageMagick 处理所有图片，它适合作为长尾兜底，不适合作为唯一主路径。

### Plugin Runtime

| 模块 | 推荐 | 作用 | 取舍 |
| --- | --- | --- | --- |
| Process plugin | 首选 | decoder/action/import 插件 | 最简单，隔离性好，跨语言 |
| WASM plugin | Wasmtime/Extism，后续 | 轻量权限沙箱 | 对媒体解码这类 native 依赖不一定合适 |
| Browser extension bridge | Native Messaging，后续 | 浏览器扩展保存到真实目录 | 不做首版功能，只保留 import provider 接口 |

推荐决策：

- 首版插件系统只做 manifest、权限、启用禁用、日志、内部 decoder/action 接口。
- 解码器插件优先子进程模型，不让第三方代码进主进程。

## 性能路线审查

### 保留

现有架构文档里的以下判断是正确的：

- Rust Core 独立承担索引、数据库、缩略图、文件监听、文件操作。
- UI 使用虚拟滚动，不全量加载目录或搜索结果。
- SQLite 使用 WAL 和批量事务。
- 缩略图分 tiny/grid/retina/preview 多级。
- 视口优先、选中项优先、后台补齐的优先级队列。
- 预览时先显示缓存图，再异步加载原图或视频。
- 真实文件操作必须有操作日志。
- 插件不能进入性能关键路径。

### 调整

建议调整或细化：

1. 首版明确采用 Electron，而不是 Electron/Tauri 二选一。
   - 这能降低 UI 和桌面集成开发成本。
   - Tauri 保留为后续壳替换选项。

2. 缩略图缓存分两阶段。
   - MVP 用 hash 分片文件缓存，开发最快。
   - 当压测证明 NTFS 小文件压力明显，再做 thumbnail pack。
   - 不建议首版直接实现 pack，除非性能原型已证明必须。

3. 视频缩略图必须限速。
   - 视频首帧提取很吃 CPU 和 IO。
   - 后台队列要按设备负载动态降速。

4. 高级格式采用插件，不承诺首版全部完美支持。
   - 产品上可以说“支持主流图片/视频，长尾格式通过插件扩展”。
   - 否则开发和测试成本会失控。

5. 文件 hash 不要默认全量计算。
   - 几百万文件全量 hash 会造成初始扫描不可接受。
   - 初始索引用 size + mtime + path + file id。
   - 内容 hash 放到后台低优先级任务，只对去重/相似图需要的文件计算。

6. 目录树也必须分页/懒加载。
   - 不要一次性加载全部 folder rows。
   - 展开节点时查询 children。

7. 视觉 blur 要有性能开关。
   - 低端机器、大目录滚动、远程桌面环境下应自动降低玻璃效果。

### 避免

不要走这些路线：

- 纯 Electron/Node 实现全部索引和缩略图。
- 前端一次性加载所有文件路径。
- 用 offset 翻页浏览百万级列表。
- 把所有缩略图都存成一个目录里的小文件。
- 初始扫描时同步生成所有缩略图再让用户浏览。
- 默认对所有文件全量 hash。
- 主进程直接调用不受控解码器处理长尾格式。
- 为了视觉效果在媒体网格背景上大面积 backdrop-filter blur。

## 建议首版依赖组合

首版最小依赖：

```text
apps/desktop
  electron
  vite
  react
  typescript
  @tanstack/react-virtual

crates/core
  tokio
  axum
  serde
  tracing
  rusqlite
  walkdir
  notify
  rayon
  blake3
  trash
  windows
```

媒体处理依赖按阶段接入：

```text
Phase 1
  image crate
  fast_image_resize

Phase 2
  libvips binding or libvips sidecar
  ffmpeg sidecar

Phase 3
  exif-rs
  optional ExifTool sidecar

Phase 4
  decoder plugins for RAW / HEIC / PSD / long-tail formats
```

## 压测原型优先级

为了验证这套路线，下一步不要先做完整 UI。

先做三个原型：

1. SQLite million rows prototype
   - 生成 100 万和 500 万 file/media rows。
   - 测目录查询、筛选、排序、keyset pagination。

2. Virtual grid prototype
   - 用假数据和本地缩略图测试 10 万、100 万项滚动。
   - 验证 TanStack Virtual + CSS 方案是否足够。

3. Thumbnail pipeline prototype
   - 对真实图片/视频目录生成 tiny/grid 缩略图。
   - 测 libvips/FFmpeg 吞吐、CPU、IO、缓存命中延迟。

只有这三个原型通过后，再进入完整桌面应用骨架。

## Sources

- Electron performance docs: https://www.electronjs.org/docs/latest/tutorial/performance
- Tauri docs: https://tauri.app/
- TanStack Virtual docs: https://tanstack.com/virtual/latest/docs
- SQLite WAL docs: https://www.sqlite.org/wal.html
- SQLite FTS5 docs: https://www.sqlite.org/fts5.html
- notify-rs repository: https://github.com/notify-rs/notify
- libvips docs: https://www.libvips.org/
- sharp docs: https://sharp.pixelplumbing.com/
- FFmpeg docs: https://ffmpeg.org/documentation.html
- FFmpeg license docs: https://ffmpeg.org/legal.html
- ImageMagick docs: https://imagemagick.org/
- rusqlite repository: https://github.com/rusqlite/rusqlite
- Tantivy repository: https://github.com/quickwit-oss/tantivy
- image-rs repository: https://github.com/image-rs/image
- fast_image_resize repository: https://github.com/Cykooz/fast_image_resize
- ExifTool docs: https://exiftool.org/
- windows-rs repository: https://github.com/microsoft/windows-rs
- trash-rs repository: https://github.com/Byron/trash-rs
- BLAKE3 repository: https://github.com/BLAKE3-team/BLAKE3

# Open Source Products Review

Updated: 2026-05-16

## 目标

这份文档抛开 Eagle 的技术路线，只评估现有开源产品或半成品能否作为 Mangoo 的基础、分支、参考实现或替代方案。

Mangoo 的硬约束：

- Windows 首发。
- 索引已有目录，不导入到私有库。
- 几百万图片/视频规模。
- 快速缩略图浏览和切图不卡顿。
- 标签/评分只保存在数据库。
- 支持真实文件重命名、移动、删除。
- UI 交互接近高效媒体管理器，视觉可做液态玻璃风格。
- 后续支持 Web/Docker。
- 浏览器扩展以后以插件形式支持。

## 总体结论

没有一个现成开源项目完整覆盖这些约束。

最值得关注的是：

1. **Lap**
   - 最接近 Windows 桌面本地图片管理器。
   - 适合做桌面端参考或 GPL 项目基础。
   - 不足是目标规模和成熟度未证明能到几百万，且 GPL-3.0 会影响闭源/商业分发。

2. **Photofield**
   - 最接近未来 Web/Docker 高性能大图浏览核心。
   - MIT 许可，架构和性能目标很值得借鉴。
   - 不足是偏只读浏览器/服务端图库，不覆盖桌面真实文件操作和完整标签管理。

3. **digiKam**
   - 功能最成熟的开源桌面照片管理器。
   - 适合学习数据库、目录/相册、元数据、缩略图和批处理设计。
   - 不适合作为 Mangoo 的直接基础，C++/Qt/GPL/KDE 体系改造成本高，UI 目标也不一致。

4. **Immich / PhotoPrism / PiGallery2 / Photoview**
   - 更适合 Web/Docker 或自托管相册。
   - 不适合作为 Windows 桌面本地真实文件管理器的直接基础。

推荐策略：

- 如果项目必须闭源或商业友好：不要 fork GPL/AGPL 项目，继续自研 Core，但大量参考 Photofield 的 Web/Docker 与高性能浏览思路。
- 如果项目可以 GPL-3.0 开源：可以认真评估 fork Lap 作为桌面端起点，但仍要重做百万级索引、缩略图队列和文件操作一致性。
- 不建议 fork Immich/PhotoPrism/digiKam 作为 Mangoo 主线，它们会把项目拖进完全不同的产品形态和技术栈。

## 候选项目对比

| 项目 | 类型 | 技术倾向 | 许可风险 | 与 Mangoo 匹配度 | 结论 |
| --- | --- | --- | --- | --- | --- |
| Lap | 桌面本地照片管理器 | Tauri/Vue/Rust/SQLite | GPL-3.0 | 高 | 最像 Mangoo，可参考或 GPL fork |
| Photofield | Web/Docker 图片浏览器 | Go/Vue/SQLite | MIT | 中高 | 最值得借鉴高性能浏览和 Web 路线 |
| digiKam | 成熟桌面照片管理器 | C++/Qt/KDE | GPL | 中 | 功能参考，不建议 fork |
| Immich | 自托管照片/视频平台 | Server/Web/Mobile/Postgres | AGPL | 中 | Web/Docker 参考，不适合桌面主线 |
| PhotoPrism | 自托管照片管理 | Go/Web/Docker | AGPL/商业混合风险 | 中 | 索引/格式参考，不建议作为基础 |
| PiGallery2 | 轻量 Web 相册 | Node/Angular | MIT | 中 | 目录图库参考，不适合桌面文件操作 |
| Photoview | 自托管图库 | Go/React | GPL-3.0 | 中 | Web 相册参考，不适合桌面主线 |
| TagStudio | 本地标签管理器 | Python/Qt | GPL-3.0 | 中低 | 标签模型参考，不适合百万级主线 |
| Hydrus Network | 本地媒体标签数据库 | Python/Qt | 开源 | 低 | 强标签系统，但导入库模型不匹配 |
| HomeGallery | Web 本地图库 | Node/Web | 开源 | 低中 | 浏览体验参考，规模上限不匹配 |
| Oculante | 高性能图片查看器 | Rust/native UI | MIT | 低中 | 预览器和切图缓存参考，不是管理器 |

## 重点项目审查

### 1. Lap

项目定位：

- 开源、跨平台、本地优先的照片管理器。
- 支持导入现有文件夹、浏览、标签、搜索、AI/语义搜索、地图、视频。
- 技术栈接近 `Tauri + Vue + Rust + SQLite`。

优点：

- 产品形态非常接近 Mangoo。
- 已经是桌面本地应用，而不是纯 Web 相册。
- 从公开说明看，强调不把照片上传云端。
- 对“已有文件夹 + 本地索引 + 标签”的方向较贴近。
- 如果能接受 GPL-3.0，fork 成本可能低于从零搭桌面壳。

风险：

- GPL-3.0：如果直接复用或 fork，Mangoo 基本也需要 GPL-3.0 开源。
- 目标数据规模公开描述更偏 10 万级，不足以证明几百万级。
- UI/交互不一定符合你想要的 Eagle 式高密度资产管理体验。
- 若已有 AI/地图/相册等功能耦合较深，删减和重构成本可能不低。
- 后续 Web/Docker 路线未必天然顺滑。

建议：

- 值得 clone 后做一次代码审计和性能跑分。
- 如果项目定位允许 GPL，可以作为“桌面 MVP 起点”。
- 如果项目要闭源/商业授权，不能直接复用代码，只能参考产品和架构。

### 2. Photofield

项目定位：

- 非侵入式、快速的照片查看器。
- 支持本地目录、Docker、Web UI、SQLite。
- 强调大量照片的快速浏览和 zoomable wall。

优点：

- MIT 许可，商业友好。
- 与未来 Web/Docker 路线非常接近。
- 高性能照片墙、缩略图/多分辨率、服务端扫描思路很值得借鉴。
- Go 单服务形态部署成本较低。
- 对“已有目录只索引不导入”理念很匹配。

风险：

- 更像只读/轻管理图片浏览器，不是完整 Windows 桌面文件管理器。
- 标签、评分、真实文件移动/删除不是核心能力。
- 视频和长尾格式能力未必覆盖 Mangoo 目标。
- UI 风格是 Web gallery，不是本地高密度资产管理工具。

建议：

- 强烈建议作为 Web/Docker 与大规模浏览的参考项目。
- 可以评估是否复用部分设计甚至 fork server 作为实验基础。
- 若要保持 Rust Core 路线，则重点学习其扫描、缩略图、浏览和 Docker 部署设计。

### 3. digiKam

项目定位：

- 成熟的开源照片管理器。
- 支持相册、标签、评分、搜索、EXIF/IPTC/XMP、RAW、批处理、数据库。
- 支持本地、可移动和网络收藏。

优点：

- 功能极成熟，覆盖照片管理大量边界情况。
- 对元数据、相册、标签、缩略图、批处理的设计很有参考价值。
- Windows 可用。

风险：

- C++/Qt/KDE 体系庞大，学习和改造成本高。
- GPL/KDE 生态约束明显。
- UI 和 Mangoo 目标差异较大。
- 作为 fork 改成液态玻璃、Web/Docker 复用几乎等于重做。
- 公开性能目标常见描述是 100K+ 级照片，不等于几百万级媒体库。

建议：

- 不建议 fork。
- 可作为功能清单、元数据策略、批处理和边界场景参考。

### 4. Immich

项目定位：

- 开源自托管照片和视频管理平台。
- 有 Web、移动端、Docker、外部库、缩略图、机器学习、地图、人物等功能。

优点：

- Web/Docker 产品成熟度高。
- 照片/视频管理体验现代。
- 支持 external libraries，可索引已有目录。
- 有完整 Web UI、后台任务、缩略图、元数据和搜索体系。

风险：

- AGPL-3.0：直接复用/fork 会影响授权策略。
- 架构是自托管服务器，不是 Windows 桌面本地文件管理器。
- 依赖 Postgres、Redis、机器学习服务等，部署和调试复杂。
- 外部库适合只读或半管理场景，不适合直接重命名/移动/删除真实文件作为核心。
- 产品目标偏云相册/手机备份，不是本地目录资产管理。

建议：

- 不建议作为 Mangoo 主线基础。
- 可以参考其 Web/Docker、缩略图任务、后台 job、外部库体验。

### 5. PhotoPrism

项目定位：

- 自托管 AI 照片应用。
- 扫描 originals 目录，生成缩略图和索引。
- 支持 Docker、Web UI、元数据和多格式导入。

优点：

- Go 后端，部署形态清晰。
- 格式、EXIF、缩略图、索引经验丰富。
- Docker/Web 路线成熟。

风险：

- 产品不是 Windows 桌面工具。
- UI 不以真实目录树和本地文件操作为核心。
- 授权和商业功能边界要仔细审查。
- 如果 Mangoo 直接采用，会继承大量不需要的云相册/AI/账户模型复杂度。

建议：

- 不建议 fork。
- 可参考 originals/index/cache/sidecar 的工程边界。

### 6. PiGallery2

项目定位：

- 轻量、快速、自托管目录相册。
- 使用已有目录结构，生成缩略图，Docker 部署。

优点：

- MIT 许可。
- 目录结构优先，理念接近“映射真实目录”。
- Web/Docker 成本低。
- 适合快速验证 Web gallery。

风险：

- 不是桌面应用。
- 文件操作、标签评分、百万级资产管理能力不是核心。
- Node/Angular 栈与当前 Rust Core 方案不同。

建议：

- 不作为主线基础。
- 可参考 Docker、目录浏览、轻量索引和缩略图策略。

### 7. Photoview

项目定位：

- 自托管照片图库。
- 扫描已有目录，Web UI 浏览。

优点：

- Go 后端 + Web UI，部署比大型相册简单。
- 与 existing folders / web gallery 有一定匹配。

风险：

- GPL-3.0。
- 桌面文件操作、Windows 原生体验和高密度资产管理不是目标。
- 项目活跃度和长期路线需要单独确认。

建议：

- 只作为 Web gallery 参考。

### 8. TagStudio

项目定位：

- 面向本地文件的标签管理工具。
- 强调给文件建立非破坏式标签数据库。

优点：

- 标签/别名/关系模型值得参考。
- 与“标签只进数据库，不写原文件”的方向接近。

风险：

- Python/Qt 体系不适合百万级性能主线。
- 产品仍在较早期阶段。
- 不以高速缩略图浏览和视频管理为核心。

建议：

- 参考标签数据模型。
- 不建议作为 Mangoo 基础。

### 9. Oculante

项目定位：

- 快速、跨平台、硬件加速图片查看器。
- 强调 threaded image loading、configurable image caching 和快速启动/加载。

优点：

- MIT 许可，授权友好。
- 对“快速切图”和“预览缓存”的实现思路值得参考。
- Rust 技术方向与 Mangoo Core 有一定重合。

风险：

- 它是图片查看器/轻编辑器，不是媒体资产管理器。
- 不提供目录索引、标签评分、百万级数据库、文件操作工作流。
- 视频和完整 DAM 能力不是目标。

建议：

- 不作为基础项目。
- 可审计其图片加载、缓存和预览交互实现。

## 与当前自研路线对比

### 继续当前路线

```text
Electron + React + Rust Core + SQLite + libvips/FFmpeg + plugins
```

优点：

- 完全贴合你的产品约束。
- 授权可控。
- 桌面体验和未来 Web/Docker 可以同时规划。
- 性能关键路径可以从第一天按百万级设计。
- UI 可以做成你想要的高密度资产管理器。

缺点：

- 初期开发成本比 fork 高。
- 需要自己实现索引、缩略图、文件操作、队列、插件系统。

适用条件：

- 想掌控授权和产品方向。
- 目标是长期项目，不只是快速试用。
- 必须支撑几百万级库。

### Fork Lap

优点：

- 桌面本地图片管理基础最接近。
- 可以快速看到完整产品形态。
- 技术栈现代。

缺点：

- GPL-3.0。
- 需要验证性能和代码质量。
- 可能仍要大改 Core。

适用条件：

- Mangoo 可以 GPL-3.0 开源。
- 你希望最快做出桌面可用版本。

### Fork Photofield

优点：

- MIT。
- 高性能图片墙和 Web/Docker 基础最好。
- 非侵入式索引理念接近。

缺点：

- 桌面文件管理、标签评分、真实文件操作缺口大。
- UI 产品形态和 Mangoo 差异明显。

适用条件：

- 优先做 Web/Docker 版本。
- Windows 桌面只是壳或未来再做。

### Fork Immich / PhotoPrism / digiKam

结论：

- 不推荐作为 Mangoo 主线。
- 它们都足够强，但强在另一个方向。
- fork 后为了适配 Mangoo 的约束，改造成本可能超过自研。

## 更新后的推荐方案

更务实的路线不是“纯自研”或“直接 fork”，而是两步验证：

### Step 1: 做开源代码审计和跑分

对两个项目做本地验证：

1. Lap
   - 能否在 Windows 顺利构建。
   - SQLite schema 是否适合真实目录映射。
   - 网格滚动和缩略图队列如何实现。
   - 10 万/100 万数据下表现如何。
   - 代码是否容易剥离 AI/非必要功能。

2. Photofield
   - Docker/Windows 本地运行是否顺畅。
   - 扫描吞吐、缩略图缓存、照片墙性能。
   - SQLite schema 和 thumbnail cache 设计。
   - 能否扩展标签/评分/文件操作。

### Step 2: 决策

决策建议：

- 如果 Lap 代码质量高、性能可改、且 GPL 可接受：以 Lap 为桌面 MVP 基础。
- 如果 GPL 不可接受：不要碰 Lap 代码，继续当前自研路线。
- 如果 Photofield 的浏览性能非常好：借鉴或复用其 Web/Docker 侧设计，但桌面端仍自研。
- 如果两个项目都不能支撑 100 万级测试：维持当前 `Rust Core + SQLite + libvips/FFmpeg` 路线。

## 当前最佳判断

在不确定授权策略的情况下，推荐：

```text
主线：继续自研 Rust Core + Electron/React UI
参考：Photofield 的高性能 Web/Docker 浏览设计
审计：Lap 作为可能的 GPL 桌面起点
借鉴：digiKam 的元数据/批处理边界，TagStudio 的标签模型
参考：Oculante 的快速预览和切图缓存
不 fork：Immich、PhotoPrism、PiGallery2、Photoview、digiKam
```

如果目标是降低开发成本但不牺牲长期路线，最有价值的下一步不是马上换方案，而是对 Lap 和 Photofield 做 1 到 2 天的本地 spike。

## Spike 验收清单

Lap spike：

- Windows 构建成功。
- 添加一个 10 万文件测试目录。
- 记录首次扫描耗时、内存、数据库体积。
- 滚动 30 秒，记录是否掉帧/卡顿。
- 连续左右切图 100 次，记录延迟。
- 判断代码是否能改成百万级优先队列和预取模型。

Photofield spike：

- Docker 或 Windows 运行成功。
- 添加 10 万/100 万图片目录。
- 记录扫描吞吐。
- 记录首屏出图时间。
- 记录照片墙缩放/滚动体验。
- 判断能否扩展为标签/评分/真实文件操作。

## Sources

- Lap repository: https://github.com/julyx10/lap
- Photofield site: https://photofield.dev/
- Photofield repository: https://github.com/SmilyOrg/photofield
- digiKam site: https://www.digikam.org/
- digiKam documentation: https://docs.digikam.org/
- Immich repository: https://github.com/immich-app/immich
- Immich external libraries docs: https://immich.app/docs/features/libraries/
- PhotoPrism repository: https://github.com/photoprism/photoprism
- PhotoPrism docs: https://docs.photoprism.app/
- PiGallery2 docs: https://bpatrik.github.io/pigallery2/
- Photoview docs: https://photoview.github.io/docs/
- TagStudio repository: https://github.com/TagStudioDev/TagStudio
- Hydrus Network repository: https://github.com/hydrusnetwork/hydrus
- HomeGallery repository: https://github.com/xemle/home-gallery
- Oculante repository: https://github.com/woelper/oculante

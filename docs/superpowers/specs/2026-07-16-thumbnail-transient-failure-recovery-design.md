# 缩略图瞬态失败自动恢复(根可达性门)设计

- **日期:** 2026-07-16
- **状态:** 设计已确认,待写实现计划
- **区域:** Rust Core(`crates/core`)缩略图失败分类 + 根可达性探针;Web UI 侧栏断连图标

## 问题

一次全量缩略图生成期间,NAS(`\\?\UNC\192.168.50.77\NAS_Repository_H\...`,root 1)发生**瞬态网络中断**,导致 **18,807** 个 NAS 文件缩略图以 `找不到网络路径 (os error 53)` 失败,被记为 `thumb_state='failed'`。加上 `database is locked`(312)、`Memory limit exceeded`(8),共 **19,127** 个**瞬态**失败。

**核心缺陷:** `failed` 缩略图是**终态,永不自动重试**(只能靠用户手动 `retry_task` 或 `retryFailedAndStale` 模式)。Mangoo **不区分瞬态失败(网络抖动、锁、内存,源恢复后可成功)与永久失败(格式不支持、文件损坏)**。所以 NAS 恢复后(现已可达,已用 `os.path.exists` 验证失败文件 `005.jpg`/id 180545 存在且 2.8MB 可读),这些瓦片仍永久显示 `tile-thumb-failed`。

**为何"点开能看到图":** 预览走**实时读原图**的独立路径(NAS 已回来,成功);网格瓦片信任陈旧的终态 `failed`,从不重读。这解释了"非必现"——取决于所浏览文件夹的文件是否在 NAS 掉线期间失败过。

**证据(系统化调试):** 后端完全健康——`/api/health` 200、`/api/media` 200、已缓存 blob(file 1)返回 200 image/webp 23KB;WAL 健康(192MB,单写入修复生效);495,377 个 ready 缩略图全部有 blob(**0** 个 ready-却-缺-blob 的真 bug)。失败纯粹是源在生成时不可达。

## 目标

区分瞬态与永久失败;当失败缩略图所属根**可达**时,自动重新生成瞬态失败——既即时恢复用户正在浏览的内容,也后台清掉积压——且在根不可达(NAS 离线)时**零重试**并给出清晰视觉反馈。

## 关键决策(已与用户确认)

1. **重试时机:两者结合** — 浏览时自动重试可见项(即时)+ 低优先级后台扫描清积压。
2. **分类方式:新增 `thumbs.failure_kind` 字段**(需 migration + schema-check 更新),而非按 error 文本临时匹配。
3. **防报风暴:只用根可达性门**(用户洞察)——探测**每个根自身路径**一次即代表整个 NAS 在线状态;不可达则跳过该根所有重试并显示断连图标;可达则瞬态失败直接重试(源已确认在线,无需 per-thumbnail 退避)。

## 非目标(显式推迟)

- **扩展解码器支持**(AVIF 440、PSD 287、.mp4 ffmpeg 错误 80)——让当前 permanent 失败里"其实能支持"的重新生成。用户要求**最后单独处理**,单独 brainstorm → spec,不在本 spec 内。真正损坏的 JPEG(约 5)保持 permanent。
- per-thumbnail 指数退避 / retry 计数字段——被根可达性门取代,不做。
- 修改单写入架构(上一修复的成果)——重试入队仍经 `DbWriter`,不变。

## 架构

**两块独立能力,由一个廉价探针门控:**

### 1. 失败分类(`failure_kind`)

- **新列** `thumbs.failure_kind TEXT`(可空;仅当 `state='failed'` 时有值)。取值 `transient` | `permanent`。
- **迁移** `crates/core/migrations/0018_thumb_failure_kind.sql`:`ALTER TABLE thumbs ADD COLUMN failure_kind TEXT;`,在 `db/migrations.rs` 声明、`Database::apply_migrations`(`db/mod.rs`)门控、`tools/checks/validate_sqlite_schema.py`(`MIGRATIONS` + 列断言)断言。
- **纯分类函数** `classify_thumbnail_failure(error: &str) -> FailureKind`(缩略图模块):
  - `transient`:error 含网络签名(`os error 53`、中文"找不到网络路径"/"网络"、"network path")、`database is locked`、`Memory limit exceeded`。
  - `permanent`:其余(格式不支持、文件损坏、不支持的媒体类型)。
- 失败记录路径 `publish_thumbnail_failure_for_attempted_source_for_task_attempt`(`db/mod.rs:2591`)新增 `failure_kind` 参数,与现有 `error` 一起写入。
- **回填**:0018 迁移对现有 `state='failed'` 行按其 `error` 文本运行同一分类,一次性 `UPDATE thumbs SET failure_kind=... WHERE state='failed'`。这即是清掉当前 19k 积压的动作——它们被分类为 `transient` 从而重试合格。

### 2. 根可达性门(防报风暴 + 断连图标)

- **每根可达性探针**:对每个 enabled 根,`stat` 其自身根目录路径(一次系统调用)判断可达。这一个探针代表整个 NAS 在线状态(用户洞察:主文件夹可达 = NAS 在线)。
- **时机/存储**:后台低频周期(每 30s)刷新所有根可达性,结果缓存在 `AppState` 内存(`Arc<Mutex<HashMap<root_id, bool>>>` 或等价)。**不落库**——可达性是瞬时状态,重启后重探即可,故**不需要额外 migration**。
- **API**:`RootRecord` DTO 新增 `reachable: boolean`(契约 `openapi.yaml` + `generated-contract.ts` 同步);`GET /api/roots` 从内存缓存读出该字段(零额外 IO)。
- **门控**:
  - **根不可达** → NAS 离线 → 侧栏该根显示未连接图标 + 跳过该根下所有 transient 重试(viewport 与后台扫描都不入队)。
  - **根可达** → NAS 在线 → 该根下 `transient` 失败直接重试,无退避(源已确认在线)。

## 重试触发(两条路径)

### 触发一:浏览时自动重试(即时恢复)

打开文件夹、其所属根**可达**时,视口内 `state='failed' AND failure_kind='transient'` 的缩略图重新入队,走现有 viewport-priority 调度。复用滚动时 POST 的 `/api/thumbnails/priority-scope` + `RetryFailedAndStale` 候选查询,只在候选条件里加"根可达 AND failure_kind='transient'"这一门。

### 触发二:后台低优先级扫描(清积压)

一个 `bulk(-10)` 优先级后台过程,分批把"可达根下 transient 失败"重新入队,无需浏览即可清 19k 积压。尊重根可达性门——不可达根直接跳过,NAS 离线时零动作。

### 重试即"重新生成"

复用现有 `reset_failed_thumbnail_for_manual_retry` / `RetryFailedAndStale`,把 `failed` 行重置为 `queued`,worker 正常处理。成功 → `ready` + blob;再次失败则重新分类:仍 transient 留 `failed/transient`(下次可达再试),变 permanent 落 `failed/permanent`(不再自动重试)。**无无限循环**:根不可达时根本不入队,可达时要么成功要么明确终态。

## 错误处理

- 单写入仍经 `DbWriter`(上一修复成果);重试入队是普通状态写,同样经 writer。
- permanent 失败永远终态,不进任何重试候选。
- 分类为纯函数,失败即分类,不因线程时序改变。
- 根探针 `stat` 失败(权限、超时)按不可达处理——保守跳过重试,不误判为可达而反复失败。

## 测试

- **分类函数单测**:真实错误串(`os error 53`、中文"找不到网络路径"、AVIF、PSD、`database is locked`、`Memory limit exceeded`、corrupt JPEG)断言归类正确。
- **迁移回填测试**:插入各类 `error` 的 `failed` 行,应用 0018,断言 `failure_kind` 正确回填。
- **门控测试**:根可达时 transient 失败进入重试候选;根不可达时**不**进候选(即使 transient);permanent 任何时候不进候选。
- **根可达性探针测试**:可达路径 → `reachable=true`;不存在路径 → `reachable=false`;`stat` 出错 → `reachable=false`。
- **API 契约**:`GET /api/roots` 返回含 `reachable`;`check:core-api` / `check:core-client` 同步通过。
- 全量 `npm test`(结构、契约、schema-check、typecheck、`cargo fmt --check`、`cargo test --workspace`)。

## UI(侧栏)

`RootNode`(`apps/web/src/features/library/LibrarySidebar.tsx`)在 `root.reachable === false` 时把文件夹图标换为断连图标(Lucide `FolderX` 或类似),附浅色 tooltip"资源库离线";可达时正常显示。仅新增一个基于已有 `reachable` 字段的条件渲染,不改树结构。

## 回滚 / 风险

- 行为增量:新增列可空、新增 DTO 字段、新增后台探针 + 后台重试扫描;不改既有写入语义。
- 主要风险:后台重试扫描在大库上可能与交互缩略图争调度——用 `bulk(-10)` 最低优先级 + 尊重可达性门缓解,与现有优先级模型一致。
- 可分段:分类字段 / 根探针 / viewport 重试 / 后台扫描 / UI 图标各自相对独立,可独立回退。

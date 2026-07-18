# 缩略图瞬态失败自动恢复(根可达性门)实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 区分缩略图瞬态失败与永久失败,并在其所属根可达时自动重新生成瞬态失败(浏览时即时 + 后台清积压),根不可达时显示断连图标且零重试。

**Architecture:** 新增 `thumbs.failure_kind`(migration 0018 + 回填现有失败);失败记录时用纯函数 `classify_thumbnail_failure` 分类;Core 内存缓存每根可达性(`stat` 根路径,30s 刷新,不落库),经 `RootRecord.reachable` 暴露给 UI;重试候选门 = `failure_kind='transient' AND 根可达`,由现有 viewport-priority 调度 + 一个 bulk 后台扫描驱动;侧栏对不可达根显示 `FolderX` 图标。

**Tech Stack:** Rust(rusqlite/axum/tokio)、React+TS、契约优先 OpenAPI。

## Global Constraints

- MSVC 工具链。cargo 不在 PATH 时:`export PATH="$HOME/.cargo/bin:$PATH"`。megle-core 是 BIN 不是 lib:用 `cargo test -p megle-core <filter>`,`--lib` 无效。
- 每个任务结束 `cargo test -p megle-core`(或指定测试)绿;`cargo fmt --all` 后再提交,仓库强制 `cargo fmt --all --check`。
- 新 migration 必须:(1) 建 SQL 文件 `crates/core/migrations/0018_thumb_failure_kind.sql`;(2) 在 `crates/core/src/db/migrations.rs` 声明 `pub const`;(3) 在 `Database::apply_migrations`(`db/mod.rs`)以 `if !self.migration_applied(18)?` 门控;(4) 在 `tools/checks/validate_sqlite_schema.py`(`MIGRATIONS` + 列/表断言)断言。
- 契约优先:改 `RootRecord` DTO 必须同步 `contracts/core-api/openapi.yaml`、`packages/core-client/src/generated-contract.ts`,并让 `npm run check:core-api` / `check:core-client` 通过。
- 分类只在纯函数 `classify_thumbnail_failure(error: &str) -> ThumbnailFailureKind` 一处;transient = error 含 `os error 53` 或 中文 `找不到网络路径` 或 `网络` 或 `network path` 或 `database is locked` 或 `Memory limit exceeded`;其余 permanent。
- 可达性存内存(`AppState`),**不落库**(无额外 migration);`stat` 出错按不可达处理。
- 单写入仍经 `DbWriter`(勿绕过)。permanent 失败永远终态,不进任何重试候选。
- 不在本计划:AVIF/PSD/mp4 解码器支持(单独 effort)。

## 文件结构

- `crates/core/migrations/0018_thumb_failure_kind.sql` — 新建:加 `failure_kind` 列。
- `crates/core/src/db/migrations.rs` — 声明 `THUMB_FAILURE_KIND_MIGRATION`。
- `crates/core/src/db/mod.rs` — 门控 migration 18 + 回填;`publish_thumbnail_failure_...` 加 `failure_kind` 参数;`RootRecord` 加 `reachable` 字段;重试候选查询加 `failure_kind='transient'` 门。
- `crates/core/src/thumbnails/mod.rs` — 新增 `ThumbnailFailureKind` 枚举 + `classify_thumbnail_failure` 纯函数。
- `crates/core/src/roots/mod.rs`(或 `roots.rs`)— 根可达性探针 + 内存缓存类型。
- `crates/core/src/api/mod.rs` / `routes.rs` — `AppState` 持有可达性缓存;`list_roots` 附加 `reachable`;启动后台探针 + 后台重试扫描。
- `crates/core/src/tasks.rs` — 失败记录处(714)传入分类结果。
- `contracts/core-api/openapi.yaml` + `packages/core-client/src/generated-contract.ts` — `RootRecord.reachable`。
- `apps/web/src/features/library/LibrarySidebar.tsx` — 不可达根显示 `FolderX`。
- `tools/checks/validate_sqlite_schema.py` — migration 18 + `failure_kind` 列断言。

任务顺序:**1**(migration + 分类 + 失败记录写入)→ **2**(回填现有失败)→ **3**(根可达性探针 + 缓存)→ **4**(`RootRecord.reachable` + 契约 + API)→ **5**(重试候选门:`transient` + 可达)→ **6**(后台重试扫描)→ **7**(侧栏断连图标 + 全量门)。

---

## Task 1: `failure_kind` 列 + 分类函数 + 写入分类

**Files:**
- Create: `crates/core/migrations/0018_thumb_failure_kind.sql`
- Modify: `crates/core/src/db/migrations.rs`
- Modify: `crates/core/src/db/mod.rs`(`apply_migrations` 加门控;`publish_thumbnail_failure_for_attempted_source_for_task_attempt` 加参数)
- Modify: `crates/core/src/thumbnails/mod.rs`(枚举 + 分类函数)
- Modify: `crates/core/src/tasks.rs:714`(传入分类)
- Modify: `tools/checks/validate_sqlite_schema.py`
- Test: `crates/core/src/thumbnails/mod.rs`(分类单测)、`crates/core/src/db/mod.rs`(写入断言)

**Interfaces:**
- Produces: `pub enum ThumbnailFailureKind { Transient, Permanent }`(在 `thumbnails/mod.rs`);`pub fn classify_thumbnail_failure(error: &str) -> ThumbnailFailureKind`;`ThumbnailFailureKind::as_str(&self) -> &'static str`("transient"/"permanent")。
- Produces: `publish_thumbnail_failure_for_attempted_source_for_task_attempt(... , failure_kind: &str, ...)` 新增 `failure_kind: &str` 参数(放在 `error: &str` 之后),写入新列。

- [ ] **Step 1: 分类函数失败测试**

在 `crates/core/src/thumbnails/mod.rs` 的 `#[cfg(test)] mod tests` 加:

```rust
#[test]
fn classify_thumbnail_failure_splits_transient_and_permanent() {
    use super::{classify_thumbnail_failure, ThumbnailFailureKind::*};
    // transient
    for e in [
        "thumbnail decode failed: 找不到网络路径。 (os error 53)",
        "database is locked",
        "thumbnail decode failed: Memory limit exceeded",
        "read failed: network path not found",
    ] {
        assert!(matches!(classify_thumbnail_failure(e), Transient), "should be transient: {e}");
    }
    // permanent
    for e in [
        "thumbnail decode failed: The image format Avif is not supported",
        "thumbnail decode failed: The file extension `.\"psd\"` was not recognized as an image format",
        "thumbnail decode failed: Format error decoding Jpeg: Error parsing image",
        "thumbnail decode failed: unsupported media kind \"text\"",
    ] {
        assert!(matches!(classify_thumbnail_failure(e), Permanent), "should be permanent: {e}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core classify_thumbnail_failure -- --nocapture`
Expected: FAIL — `classify_thumbnail_failure` / `ThumbnailFailureKind` 未定义。

- [ ] **Step 3: 实现枚举 + 分类函数**

在 `crates/core/src/thumbnails/mod.rs`(非 test 区):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailFailureKind {
    Transient,
    Permanent,
}

impl ThumbnailFailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThumbnailFailureKind::Transient => "transient",
            ThumbnailFailureKind::Permanent => "permanent",
        }
    }
}

/// Classify a thumbnail generation error as transient (source momentarily
/// unreachable / contention / memory — will succeed on retry once conditions
/// recover) or permanent (unsupported format / corrupt file — retrying is
/// futile). Pure function on the stored error string.
pub fn classify_thumbnail_failure(error: &str) -> ThumbnailFailureKind {
    const TRANSIENT_SIGNATURES: &[&str] = &[
        "os error 53",         // Windows: network path not found
        "找不到网络路径",       // localized "network path not found"
        "network path",
        "database is locked",
        "Memory limit exceeded",
    ];
    if TRANSIENT_SIGNATURES.iter().any(|sig| error.contains(sig)) {
        ThumbnailFailureKind::Transient
    } else {
        ThumbnailFailureKind::Permanent
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p megle-core classify_thumbnail_failure -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 建 migration 文件**

`crates/core/migrations/0018_thumb_failure_kind.sql`:

```sql
-- Distinguish transient thumbnail failures (network/lock/memory — retryable
-- once the source is reachable) from permanent ones (unsupported/corrupt).
-- Nullable; only meaningful when thumbs.state = 'failed'.
ALTER TABLE thumbs ADD COLUMN failure_kind TEXT;
```

- [ ] **Step 6: 声明 migration 常量**

在 `crates/core/src/db/migrations.rs` 末尾加(紧跟 `THUMBNAIL_TASK_INDEXES_MIGRATION` 之后,匹配现有风格):

```rust
pub const THUMB_FAILURE_KIND_MIGRATION: &str =
    include_str!("../../migrations/0018_thumb_failure_kind.sql");
```

- [ ] **Step 7: 门控 migration 18**

在 `crates/core/src/db/mod.rs` 的 `apply_migrations`,`if !self.migration_applied(17)? { ... }` 块之后、`self.backfill_media_fts_if_empty()?;` 之前加:

```rust
        if !self.migration_applied(18)? {
            self.connection
                .execute_batch(migrations::THUMB_FAILURE_KIND_MIGRATION)?;
        }
```

- [ ] **Step 8: 给失败记录写入 failure_kind**

在 `crates/core/src/db/mod.rs` 的 `publish_thumbnail_failure_for_attempted_source_for_task_attempt` 签名(约 2591 行)在 `error: &str,` 之后加 `failure_kind: &str,`。把写 `state='failed'` 的 `upsert_thumbnail_state_in_transaction` 调用改为在同一事务里再 `UPDATE thumbs SET failure_kind = ?` —— 具体做法:紧接 `upsert_thumbnail_state_in_transaction(...)?;`(约 2626 行,`is_current_attempt` 分支内)之后加:

```rust
            transaction.execute(
                "UPDATE thumbs SET failure_kind = ?1 WHERE file_id = ?2 AND profile = ?3",
                (failure_kind, file_id, profile),
            )?;
```

(注:`upsert_thumbnail_state_in_transaction` 不认识 `failure_kind`,故用紧随其后的定向 UPDATE 落该列,保持同一事务原子性。)

- [ ] **Step 9: 更新调用点传入分类**

`crates/core/src/tasks.rs:714` 的调用现传 `error`;在其上方计算 `let failure_kind = crate::thumbnails::classify_thumbnail_failure(&error_string).as_str();`(用实际的 error 变量名——grep 确认该处 error 局部变量名,用真实名),并把 `failure_kind` 作为新参数传入。`db/mod.rs:13879` 的测试调用同样补一个参数(可直接传 `"permanent"` 或对应值,保持编译)。

- [ ] **Step 10: 写入断言测试**

在 `crates/core/src/db/mod.rs` test 区加一个测试:seed 一个 file + running thumbnail task,调用 `publish_thumbnail_failure_...(..., "transient", ...)`,断言该 `thumbs` 行 `state='failed'` 且 `failure_kind='transient'`。(参考同文件已有 `stale_thumbnail_attempt_cannot_publish_failure_after_retry` 的 seed 方式。)

- [ ] **Step 11: schema-check 断言**

在 `tools/checks/validate_sqlite_schema.py`:把 `0018_thumb_failure_kind.sql` 加入 `MIGRATIONS` 列表;在 `thumbs` 表的必需列断言里加 `failure_kind`(匹配该文件现有断言写法——grep `failure_kind`/`thumbs` 附近的列检查风格)。

- [ ] **Step 12: 跑 schema-check + rust,fmt,提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
npm run check:schema
cargo fmt --all
cargo test -p megle-core thumbnails:: && cargo test -p megle-core db::tests
git add crates/core/migrations/0018_thumb_failure_kind.sql crates/core/src/db/migrations.rs crates/core/src/db/mod.rs crates/core/src/thumbnails/mod.rs crates/core/src/tasks.rs tools/checks/validate_sqlite_schema.py
git commit -m "feat(core): classify thumbnail failures transient vs permanent (failure_kind)"
```

---

## Task 2: 回填现有失败的 failure_kind

**Files:**
- Modify: `crates/core/src/db/mod.rs`(在 migration 18 门控块内回填)
- Test: `crates/core/src/db/mod.rs`

**Interfaces:**
- Consumes: `classify_thumbnail_failure`(Task 1)。
- Produces: 应用 0018 后,所有 `state='failed'` 行的 `failure_kind` 依其 `error` 文本被填。

- [ ] **Step 1: 回填失败测试**

在 `db/mod.rs` test 区加:

```rust
#[test]
fn migration_0018_backfills_failure_kind_from_error_text() {
    let dir = unique_db_temp_dir("failkind-backfill");
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("bf.sqlite");
    // Open + migrate through 0017 by applying migrations, then manually insert
    // failed rows WITHOUT failure_kind (simulating pre-0018 data) is hard once
    // 0018 is auto-applied; instead: open, apply_migrations (adds column), then
    // NULL out failure_kind on inserted failed rows, then call the backfill fn.
    let db = Database::open(&db_path).unwrap();
    db.apply_migrations().unwrap();
    // seed a root+folder+2 files+2 failed thumbs with known errors, failure_kind NULL
    let (net_fid, fmt_fid) = seed_two_failed_thumbs_null_kind(&db,
        "thumbnail decode failed: 找不到网络路径。 (os error 53)",
        "thumbnail decode failed: The image format Avif is not supported");
    db.backfill_thumbnail_failure_kind().unwrap();
    assert_eq!(read_failure_kind(&db, net_fid), Some("transient".to_string()));
    assert_eq!(read_failure_kind(&db, fmt_fid), Some("permanent".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}
```

(`seed_two_failed_thumbs_null_kind` / `read_failure_kind` 是本测试内的小 helper;grep 现有测试的 root/folder/file/thumb seed 方式复用真实 upsert 方法,插入后 `UPDATE thumbs SET state='failed', error=?, failure_kind=NULL`。)

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p megle-core migration_0018_backfills -- --nocapture`
Expected: FAIL — `backfill_thumbnail_failure_kind` 未定义。

- [ ] **Step 3: 实现回填函数 + 在 migration 门控内调用**

在 `db/mod.rs` 加方法:

```rust
/// One-time backfill: classify every existing failed thumbnail's failure_kind
/// from its stored error text. Idempotent — only touches rows where
/// failure_kind IS NULL. Run inside the 0018 migration gate.
pub fn backfill_thumbnail_failure_kind(&self) -> anyhow::Result<()> {
    let mut stmt = self
        .connection
        .prepare("SELECT file_id, profile, coalesce(error, '') FROM thumbs WHERE state = 'failed' AND failure_kind IS NULL")?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for (file_id, profile, error) in rows {
        let kind = crate::thumbnails::classify_thumbnail_failure(&error).as_str();
        self.connection.execute(
            "UPDATE thumbs SET failure_kind = ?1 WHERE file_id = ?2 AND profile = ?3",
            (kind, file_id, profile),
        )?;
    }
    Ok(())
}
```

在 `apply_migrations` 的 `if !self.migration_applied(18)? { ... }` 块内、`execute_batch` 之后加 `self.backfill_thumbnail_failure_kind()?;`(仅首次应用 0018 时跑一次)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p megle-core migration_0018_backfills -- --nocapture`
Expected: PASS。

- [ ] **Step 5: fmt,提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all && cargo test -p megle-core db::tests
git add crates/core/src/db/mod.rs
git commit -m "feat(core): backfill failure_kind for existing failed thumbnails (migration 0018)"
```

---

## Task 3: 根可达性探针 + 内存缓存

**Files:**
- Create: `crates/core/src/roots/reachability.rs`(或加入现有 `roots` 模块;grep `mod roots` 确认结构)
- Test: 同文件

**Interfaces:**
- Produces: `pub fn probe_root_reachable(path: &str) -> bool` — `std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)`,任何错误(网络不通/权限/超时)→ `false`。
- Produces: `pub type RootReachability = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i64, bool>>>;`
- Produces: `pub fn refresh_root_reachability(reach: &RootReachability, roots: &[(i64, String)])` — 对每个 `(root_id, path)` 调 `probe_root_reachable`,写入 map。

- [ ] **Step 1: 探针失败测试**

```rust
#[test]
fn probe_root_reachable_true_for_existing_dir_false_for_missing() {
    use super::probe_root_reachable;
    let dir = std::env::temp_dir();
    assert!(probe_root_reachable(dir.to_str().unwrap()));
    assert!(!probe_root_reachable(r"\\?\UNC\10.255.255.1\nonexistent\share"));
    assert!(!probe_root_reachable(""));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core probe_root_reachable -- --nocapture`
Expected: FAIL — 未定义。

- [ ] **Step 3: 实现探针 + 刷新**

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type RootReachability = Arc<Mutex<HashMap<i64, bool>>>;

/// True iff the root's own directory can be stat'd as a directory right now.
/// One syscall stands in for the whole share's online status: unreachable root
/// path == NAS offline. Any error (network down, permission, timeout) => false.
pub fn probe_root_reachable(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

pub fn refresh_root_reachability(reach: &RootReachability, roots: &[(i64, String)]) {
    let mut next = HashMap::with_capacity(roots.len());
    for (id, path) in roots {
        next.insert(*id, probe_root_reachable(path));
    }
    if let Ok(mut guard) = reach.lock() {
        *guard = next;
    }
}
```

在 crate 里声明该模块(`mod roots { pub mod reachability; }` 或匹配现有结构;grep `mod roots` 与 `roots.rs` 确认真实路径,按实际接线)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p megle-core probe_root_reachable reachability -- --nocapture`
Expected: PASS(含一个刷新测试:构造 map、放一个存在路径 + 一个不存在路径,`refresh_root_reachability` 后断言值)。加该刷新测试。

- [ ] **Step 5: fmt,提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all && cargo test -p megle-core reachability
git add crates/core/src/roots/
git commit -m "feat(core): per-root reachability probe + in-memory cache"
```

---

## Task 4: `RootRecord.reachable` + 契约 + 后台探针启动

**Files:**
- Modify: `crates/core/src/db/mod.rs`(`RootRecord` 加 `reachable: bool`;`list_roots` 不设它,默认 true——见下)
- Modify: `crates/core/src/api/mod.rs`(`AppState` 持 `RootReachability`;启动 30s 周期探针)
- Modify: `crates/core/src/api/routes.rs`(`list_roots` 用缓存覆盖 `reachable`)
- Modify: `contracts/core-api/openapi.yaml`、`packages/core-client/src/generated-contract.ts`
- Test: `crates/core/src/api/routes.rs`

**Interfaces:**
- Consumes: `RootReachability`, `refresh_root_reachability`(Task 3)。
- Produces: `RootRecord.reachable: bool`(Rust)/`reachable: boolean`(TS)。`list_roots` 返回项的 `reachable` 反映内存缓存;缓存里没有该 root_id 时默认 `true`(尚未探测视为可达,避免冷启动误报离线)。

- [ ] **Step 1: 契约与 DTO 更新 + API 测试**

在 `RootRecord`(Rust struct `db/mod.rs:59`)加 `pub reachable: bool,`;构造点(`db/mod.rs:6261` 的 `Ok(RootRecord { ... })`)加 `reachable: true,`(DB 层默认可达,API 层覆盖)。在 `openapi.yaml` 的 `RootRecord` schema 与 `generated-contract.ts` 的 `interface RootRecord` 加 `reachable: boolean`(放在 `mediaCount` 之后)。

加 API 测试:构造带一个真实存在路径 root 的 AppState,刷新可达性,`GET /api/roots`,断言该 root `reachable=true`;再把缓存该 id 置 false,断言返回 `reachable=false`。

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core roots -- --nocapture`
Expected: FAIL(字段缺失/未覆盖)。

- [ ] **Step 3: AppState 持缓存 + list_roots 覆盖**

在 `AppState`(`api/mod.rs`)加 `pub root_reachability: RootReachability`(构造处 `Arc::new(Mutex::new(HashMap::new()))`)。`routes.rs::list_roots`(639)改为:

```rust
async fn list_roots(State(state): State<AppState>) -> ApiResult<Json<ListResponse<RootRecord>>> {
    let reach = state.root_reachability.clone();
    let mut items = run_read_database(state, |database| Ok(database.list_roots()?)).await?;
    if let Ok(map) = reach.lock() {
        for item in items.iter_mut() {
            item.reachable = *map.get(&item.id).unwrap_or(&true);
        }
    }
    Ok(Json(ListResponse { items, next_cursor: None, total_count: None }))
}
```

- [ ] **Step 4: 启动后台探针**

在 Core 启动接线(`api/mod.rs` 的 AppState 构造 / worker 启动附近)`tokio::spawn` 一个 30s interval 循环:读所有 enabled roots 的 `(id, path)`,调 `refresh_root_reachability`。首刷在循环开头立即跑一次(冷启动尽快有真实值)。

- [ ] **Step 5: 跑测试 + 契约检查确认通过**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p megle-core roots
npm run check:core-api && npm run check:core-client
```
Expected: PASS。

- [ ] **Step 6: fmt,提交**

```bash
cargo fmt --all
git add crates/core/src/db/mod.rs crates/core/src/api/mod.rs crates/core/src/api/routes.rs contracts/core-api/openapi.yaml packages/core-client/src/generated-contract.ts
git commit -m "feat(core): expose per-root reachable + background probe"
```

---

## Task 5: 重试候选门(transient + 根可达)

**Files:**
- Modify: `crates/core/src/db/mod.rs`(`RetryFailedAndStale` 候选查询加 `failure_kind='transient'`;失败重置只重置 transient)
- Test: `crates/core/src/db/mod.rs`

**Interfaces:**
- Consumes: `failure_kind`(Task 1)。
- Produces: `RetryFailedAndStale` 模式只把 `state='failed' AND failure_kind='transient'` 的行视为重试候选;permanent 失败永不入候选。(根可达门在调用层——只对可达根发起 retry-enqueue;见 Task 6。)

- [ ] **Step 1: 候选门失败测试**

在 `db/mod.rs` test 区加:seed 同一 root 下两个 failed thumb,一个 `failure_kind='transient'` 一个 `'permanent'`;调用 `RetryFailedAndStale` 的候选/bulk-enqueue 查询(用现有 `enqueue_thumbnail_cache`/候选函数——grep `RetryFailedAndStale` 的实际调用入口);断言 transient 的进入 pending/queued,permanent 的仍 `failed` 未动。

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core retry_failed -- --nocapture`
Expected: FAIL(当前 permanent 也被重试)。

- [ ] **Step 3: 在候选查询加门**

`db/mod.rs` 的 `thumbnail_cache_candidate_query` / `thumbnail_cache_pending_bulk_task_query` 里 `RetryFailedAndStale` 分支(约 6908)的 `state = 'failed'` 条件改为 `state = 'failed' AND failure_kind = 'transient'`。同样,`reset_failed_thumbnail_for_manual_retry_in_transaction`(约 3027)与相关 WHERE `state='failed'` 处——**注意**:显式的用户手动 retry(`retry_task`)应仍能重试任意 failed(用户明确要求);只有**自动**的 `RetryFailedAndStale` 批量路径加 transient 门。仔细区分这两条路径(grep 两者调用链),只给自动路径加门,手动 retry 保持不变。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p megle-core retry_failed -- --nocapture`
Expected: PASS(transient 重试,permanent 不动,手动 retry 仍能重试 permanent——补一个手动 retry permanent 的测试断言不被门挡)。

- [ ] **Step 5: fmt,提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all && cargo test -p megle-core db::tests
git add crates/core/src/db/mod.rs
git commit -m "feat(core): auto retry candidate gate = failed AND failure_kind=transient"
```

---

## Task 6: 后台重试扫描(尊重可达性门)

**Files:**
- Modify: `crates/core/src/api/mod.rs`(启动一个 bulk 优先级后台扫描)
- Test: `crates/core/src/api/routes.rs` 或集成测试

**Interfaces:**
- Consumes: `RootReachability`(Task 3)、transient 候选门(Task 5)、`RetryFailedAndStale` enqueue。
- Produces: 后台过程周期性(如每 60s)对**每个可达 root** 以 `RetryFailedAndStale` + `bulk(-10)` 优先级分批重新入队其 transient 失败;不可达 root 跳过。

- [ ] **Step 1: 扫描门测试**

集成风格测试:两个 root(A 可达、B 不可达,通过在缓存里置 A=true/B=false 模拟);各有 transient failed thumb;跑一次后台扫描的核心函数(把扫描主体抽成可单测的 `fn run_transient_retry_sweep(db, reach) -> enqueued_count`);断言 A 的被入队、B 的未动。

- [ ] **Step 2: 跑测试确认失败**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core transient_retry_sweep -- --nocapture`
Expected: FAIL — 未定义。

- [ ] **Step 3: 实现扫描 + 启动**

抽出纯逻辑函数 `run_transient_retry_sweep(database, reachability) -> anyhow::Result<usize>`:列 enabled roots;对可达的每个 root,以 `RetryFailedAndStale` + `bulk` 优先级 enqueue 其 transient 失败(复用 Task 5 的候选门 + 现有 enqueue),返回入队数。在 Core 启动 `tokio::spawn` 60s interval 调它;分批(单次上限如 500)避免一次性入队过多。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p megle-core transient_retry_sweep -- --nocapture`
Expected: PASS。

- [ ] **Step 5: fmt,提交**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all && cargo test -p megle-core
git add crates/core/src/api/mod.rs
git commit -m "feat(core): background transient-retry sweep gated by root reachability"
```

---

## Task 7: 侧栏断连图标 + 全量门

**Files:**
- Modify: `apps/web/src/features/library/LibrarySidebar.tsx`(不可达根显示 `FolderX`)
- Modify: `apps/web/src/core/types.ts` / 用到 `RootRecord` 处(若需透传 `reachable`)
- Modify: `CLAUDE.md`(记录不变量)
- Test: `apps/web`(check:web / check:ui-design)+ 全量 `npm test`

**Interfaces:**
- Consumes: `RootRecord.reachable`(Task 4)。

- [ ] **Step 1: 侧栏图标条件渲染**

`LibrarySidebar.tsx` `RootNode` 的 label 图标处(约 124 行 `{expanded ? <FolderOpen size={16} /> : <Folder size={16} />}`)改为:

```tsx
{root.reachable === false ? (
  <FolderX size={16} className="tree-root-offline" aria-label="资源库离线" />
) : expanded ? (
  <FolderOpen size={16} />
) : (
  <Folder size={16} />
)}
```

从 `lucide-react` import `FolderX`。若 `root` 类型不含 `reachable`,确认它来自 `RootRecord`(应已含,Task 4);否则透传。加一条浅色样式 `.tree-root-offline { color: var(--text-muted); }`(或类似)到 `styles.css`。

- [ ] **Step 2: typecheck + web/ui 检查**

```bash
cd /d/Megle
npm --workspace @megle/web run typecheck
npm run check:web && npm run check:ui-design
```
Expected: PASS。

- [ ] **Step 3: 更新 CLAUDE.md**

在缩略图规则区加一段:失败缩略图区分 `failure_kind`(transient/permanent);仅 transient 且所属根可达时自动重试(viewport + bulk 后台扫描);根可达性经 `stat` 根路径的内存探针(30s)判定,不可达根显示断连图标并跳过重试;permanent 只能手动 retry。

- [ ] **Step 4: 全量门**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all --check && npm test
```
Expected: 全绿(结构、契约、schema、typecheck、cargo test --workspace)。

- [ ] **Step 5: 提交**

```bash
git add apps/web/src/features/library/LibrarySidebar.tsx apps/web/src/styles.css CLAUDE.md
git commit -m "feat(web): show disconnected icon for unreachable roots; document recovery invariant"
```

---

## 自审记录(已处理)

- **Spec 覆盖**:分类字段+函数(T1)、回填 19k(T2)、根探针+缓存(T3)、reachable DTO+契约+API(T4)、transient 重试门(T5)、后台扫描(T6)、断连图标+全量门(T7)。viewport 即时重试:由浏览时对当前文件夹 scope 触发 retryFailedAndStale(复活 transient-failed→pending)+ 现有 priority-scope promotion 把可见/选中项提升到 viewport 优先级实现(见后补的 Gap B 修复,commit e276cf9);服务端手动 retryFailedAndStale route 也已按根可达性 gate(Gap A,commit 2266a7c)。注:最初本自审曾错误声称 viewport 即时重试"已被现有 priority-scope + 候选门自动覆盖"——priority-scope 只重排已存在的 pending/running,不复活 failed,故那条声明不成立,已由 Gap B 单独实现。
- **占位符扫描**:无 TBD;所有代码步含完整代码;grep-确认项(error 变量名、roots 模块结构、手动 vs 自动 retry 调用链、schema-check 断言风格)明确标注为实现者用真实源确认,而非猜测。
- **类型一致**:`ThumbnailFailureKind`/`classify_thumbnail_failure`(T1)→ T2/T5/T6 消费;`RootReachability`/`probe_root_reachable`/`refresh_root_reachability`(T3)→ T4/T6 消费;`RootRecord.reachable`(T4)→ T7 消费。签名一致。
- **手动 retry 保护**:T5 明确只给自动 `RetryFailedAndStale` 加 transient 门,`retry_task` 手动路径不变(用户可手动重试 permanent),并加断言测试。

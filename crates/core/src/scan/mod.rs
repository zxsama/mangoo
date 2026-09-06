use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs::Metadata;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::db::{
    Database, ExternalFileIdentity, FileUpsert, FolderUpsert, RootRecord, ScanFileUpsert,
    ScanWriteBatch, SyncRootBindingToken, TaskScanProgress,
};
use crate::decode::formats::{lowercase_extension as extension, media_kind_for_extension};

pub const INTERACTIVE_SCAN_WRITE_BATCH_SIZE: usize = 1;
pub const BACKGROUND_SCAN_WRITE_BATCH_SIZE: usize = 256;
pub const DEFAULT_SCAN_WRITE_BATCH_SIZE: usize = BACKGROUND_SCAN_WRITE_BATCH_SIZE;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPriority {
    Interactive,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub folders_seen: usize,
    pub media_files_seen: usize,
    pub skipped_files: usize,
}

pub struct ScanOptions<'a> {
    pub write_batch_size: usize,
    pub progress_callback: Option<&'a mut dyn FnMut(TaskScanProgress)>,
    pub cancellation_callback: Option<&'a mut dyn FnMut() -> anyhow::Result<()>>,
    pub task_attempt_guard: Option<TaskAttemptGuard>,
    pub root_binding_guard: Option<SyncRootBindingToken>,
    /// When present, batch commits run on the single writer connection instead
    /// of the caller's `&mut Database`, so the WAL is checkpointed and stays
    /// bounded. `None` in unit tests that call `scan_root(&mut db, ...)`
    /// directly (those commit inline on the passed connection).
    pub writer: Option<crate::db::DbWriter>,
    #[cfg(test)]
    pub batch_observer: Option<std::rc::Rc<dyn Fn(ScanBatchStats)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskAttemptGuard {
    pub task_id: i64,
    pub attempt_generation: i64,
}

impl<'a> ScanOptions<'a> {
    pub fn for_priority(priority: ScanPriority) -> Self {
        let write_batch_size = match priority {
            ScanPriority::Interactive => INTERACTIVE_SCAN_WRITE_BATCH_SIZE,
            ScanPriority::Background => DEFAULT_SCAN_WRITE_BATCH_SIZE,
        };
        Self {
            write_batch_size,
            progress_callback: None,
            cancellation_callback: None,
            task_attempt_guard: None,
            root_binding_guard: None,
            writer: None,
            #[cfg(test)]
            batch_observer: None,
        }
    }

    pub fn interactive() -> Self {
        Self::for_priority(ScanPriority::Interactive)
    }

    pub fn background() -> Self {
        Self::for_priority(ScanPriority::Background)
    }
}

impl Default for ScanOptions<'_> {
    fn default() -> Self {
        Self::background()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanBatchStats {
    pub folders: usize,
    pub files: usize,
}

#[cfg(test)]
pub fn scan_root(database: &mut Database, root: &RootRecord) -> anyhow::Result<ScanSummary> {
    scan_root_with_options(database, root, ScanOptions::background())
}

#[allow(dead_code)]
pub fn scan_root_with_progress(
    database: &mut Database,
    root: &RootRecord,
    progress_callback: &mut dyn FnMut(TaskScanProgress),
) -> anyhow::Result<ScanSummary> {
    scan_root_with_options(
        database,
        root,
        ScanOptions {
            progress_callback: Some(progress_callback),
            ..ScanOptions::background()
        },
    )
}

pub fn scan_root_with_options(
    database: &mut Database,
    root: &RootRecord,
    mut options: ScanOptions<'_>,
) -> anyhow::Result<ScanSummary> {
    let root_path = PathBuf::from(&root.path);
    check_scan_cancelled(&mut options)?;
    ensure_root_enabled(database, root.id)?;
    ensure_root_binding_current(database, &options)?;
    let scan_generation = match options.task_attempt_guard {
        Some(guard) => database.begin_root_scan_reconciliation_for_task_attempt(
            root.id,
            guard.task_id,
            guard.attempt_generation,
        )?,
        None => database.begin_root_scan_reconciliation(root.id)?,
    };
    let root_batch = ScanWriteBatch {
        folders: vec![FolderUpsert {
            root_id: root.id,
            parent_id: None,
            name: String::new(),
            path_hash: hash_path(&root_path),
            mtime: metadata_time(root_path.metadata().ok().as_ref(), TimeField::Modified),
        }],
        files: Vec::new(),
        scan_generation: Some(scan_generation),
    };
    observe_scan_batch(&options, root_batch.folders.len(), root_batch.files.len());
    check_scan_cancelled(&mut options)?;
    ensure_root_enabled(database, root.id)?;
    ensure_root_binding_current(database, &options)?;
    let root_result = commit_scan_batch_with_optional_task_guard(database, root_batch, &options)?;
    let root_folder_id = root_result.folder_ids[0];

    let mut folder_ids = HashMap::new();
    folder_ids.insert(root_path.clone(), root_folder_id);
    let mut pending_files = Vec::new();
    let write_batch_size = options.write_batch_size.max(1);

    let mut summary = ScanSummary {
        folders_seen: 1,
        media_files_seen: 0,
        skipped_files: 0,
    };
    let mut items_seen = 0_i64;
    emit_progress(&mut options, items_seen, &summary);
    check_scan_cancelled(&mut options)?;

    // Structure-first pass: commit the whole folder tree up front (batched),
    // BREADTH-FIRST, before indexing any files. This makes the directory tree —
    // especially the top-level folders the user first sees — materialize in the
    // sidebar within seconds. WalkDir is depth-first, so it (and even a
    // dir-only WalkDir) commits one top-level folder's entire deep subtree before
    // the next sibling — the sidebar filled in ~1 folder/second (minutes for a
    // 300-folder root). A manual BFS queue commits every folder at depth N before
    // descending to N+1, so all 300 top-level folders appear almost immediately.
    // Folder upserts are idempotent, so the main pass below reuses these ids.
    {
        let mut pending_folders: Vec<(PathBuf, FolderUpsert)> = Vec::new();
        let folder_batch_size = write_batch_size.max(256);
        // Queue of directories to enumerate, in breadth-first order. The root's
        // children are enumerated first, then their children, and so on.
        let mut queue: std::collections::VecDeque<PathBuf> = std::collections::VecDeque::new();
        queue.push_back(root_path.clone());
        while let Some(dir) = queue.pop_front() {
            check_scan_cancelled(&mut options)?;
            let read_dir = match std::fs::read_dir(&dir) {
                Ok(read_dir) => read_dir,
                Err(error) => {
                    tracing::warn!(%error, path = %dir.display(), "scan could not read directory during structure pass");
                    continue;
                }
            };
            let mut child_dirs: Vec<PathBuf> = Vec::new();
            for entry in read_dir {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => continue,
                };
                if file_type.is_dir() {
                    child_dirs.push(entry.path());
                }
            }
            // Stable order so the sidebar list is deterministic.
            child_dirs.sort();
            for path in child_dirs {
                // The parent (`dir`) is committed before its children are
                // enqueued: flush the buffer if the parent id is not resolved yet.
                if parent_folder_id(&path, &folder_ids).is_err() {
                    flush_scan_folders(
                        database,
                        root.id,
                        Some(scan_generation),
                        &mut options,
                        &mut pending_folders,
                        &mut folder_ids,
                        &mut summary,
                        &mut items_seen,
                    )?;
                }
                let parent_id = parent_folder_id(&path, &folder_ids)?;
                let metadata = std::fs::metadata(&path).ok();
                pending_folders.push((
                    path.clone(),
                    FolderUpsert {
                        root_id: root.id,
                        parent_id: Some(parent_id),
                        name: file_name(&path),
                        path_hash: hash_path(&path),
                        mtime: metadata_time(metadata.as_ref(), TimeField::Modified),
                    },
                ));
                queue.push_back(path);
                if pending_folders.len() >= folder_batch_size {
                    flush_scan_folders(
                        database,
                        root.id,
                        Some(scan_generation),
                        &mut options,
                        &mut pending_folders,
                        &mut folder_ids,
                        &mut summary,
                        &mut items_seen,
                    )?;
                }
            }
            // Flush at the end of each directory level's children so a folder's
            // ids are committed before we descend into it on the next iteration.
            flush_scan_folders(
                database,
                root.id,
                Some(scan_generation),
                &mut options,
                &mut pending_folders,
                &mut folder_ids,
                &mut summary,
                &mut items_seen,
            )?;
        }
    }

    for entry in WalkDir::new(&root_path).min_depth(1) {
        check_scan_cancelled(&mut options)?;
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        items_seen += 1;

        if metadata.is_dir() {
            // Already committed in the structure-first pass; just ensure it is
            // known (defensive — a folder created between passes falls through
            // to a single-folder commit).
            if !folder_ids.contains_key(path) {
                let parent_id = parent_folder_id(path, &folder_ids)?;
                let batch = ScanWriteBatch {
                    folders: vec![FolderUpsert {
                        root_id: root.id,
                        parent_id: Some(parent_id),
                        name: file_name(path),
                        path_hash: hash_path(path),
                        mtime: metadata_time(Some(&metadata), TimeField::Modified),
                    }],
                    files: Vec::new(),
                    scan_generation: Some(scan_generation),
                };
                observe_scan_batch(&options, batch.folders.len(), batch.files.len());
                check_scan_cancelled(&mut options)?;
                ensure_root_enabled(database, root.id)?;
                let result = commit_scan_batch_with_optional_task_guard(database, batch, &options)?;
                let folder_id = result.folder_ids[0];
                folder_ids.insert(path.to_path_buf(), folder_id);
                summary.folders_seen += 1;
                emit_progress(&mut options, items_seen, &summary);
            }
            check_scan_cancelled(&mut options)?;
            continue;
        }

        if !metadata.is_file() {
            summary.skipped_files += 1;
            emit_progress(&mut options, items_seen, &summary);
            check_scan_cancelled(&mut options)?;
            continue;
        }

        // One `lowercase_extension` allocation per file, not two: the guard and
        // the stored `files.ext` are the same string. This runs once per file on
        // multi-million-file libraries.
        let ext = extension(path);
        let Some(kind) = media_kind_for_extension(ext.as_str()) else {
            summary.skipped_files += 1;
            emit_progress(&mut options, items_seen, &summary);
            check_scan_cancelled(&mut options)?;
            continue;
        };

        let folder_id = parent_folder_id(path, &folder_ids)?;
        let file_identity = crate::library_path::local_file_identity(path).unwrap_or_default();
        pending_files.push(ScanFileUpsert {
            file: FileUpsert {
                root_id: root.id,
                folder_id,
                name: file_name(path),
                ext,
                size: metadata.len() as i64,
                mtime: metadata_time(Some(&metadata), TimeField::Modified).unwrap_or(0),
                ctime: metadata_time(Some(&metadata), TimeField::Created),
                file_key: (!file_identity.is_empty()).then(|| file_identity.clone()),
            },
            media_kind: kind.to_string(),
            external_identity: Some(ExternalFileIdentity {
                volume_identity: crate::library_path::local_file_identity(&root_path)
                    .unwrap_or_else(|_| root.path.clone())
                    .into_bytes(),
                file_identity: file_identity.into_bytes(),
                content_fingerprint: crate::sync::identity::stable_content_fingerprint(path).ok(),
                path: path.to_path_buf(),
            }),
        });
        summary.media_files_seen += 1;
        emit_progress(&mut options, items_seen, &summary);
        check_scan_cancelled(&mut options)?;

        if pending_files.len() >= write_batch_size {
            flush_scan_files(
                database,
                root.id,
                Some(scan_generation),
                &mut options,
                &mut pending_files,
            )?;
        }
    }

    flush_scan_files(
        database,
        root.id,
        Some(scan_generation),
        &mut options,
        &mut pending_files,
    )?;
    check_scan_cancelled(&mut options)?;
    ensure_root_enabled(database, root.id)?;
    reconcile_scan_completion_with_optional_writer(database, root.id, scan_generation, &options)?;
    Ok(summary)
}

/// Reconcile+mark-scanned the completed scan, routing through the single
/// writer actor when present (bulk UPDATEs across files/folders plus
/// thumbnail cleanup — the largest remaining un-migrated write before this),
/// falling back to the caller's connection inline for tests that call
/// `scan_root`/`scan_folder` directly with `writer: None`.
fn reconcile_scan_completion_with_optional_writer(
    database: &mut Database,
    root_id: i64,
    scan_generation: i64,
    options: &ScanOptions<'_>,
) -> anyhow::Result<()> {
    crate::debug_log!(
        "scan completion reconcile (root {}){}",
        root_id,
        if options.writer.is_some() {
            " -> writer"
        } else {
            " (inline)"
        }
    );
    let guard = options.task_attempt_guard;
    match options.writer.as_ref() {
        Some(writer) => writer.write_blocking(move |db| {
            reconcile_scan_completion(db, root_id, scan_generation, guard)
        }),
        None => reconcile_scan_completion(database, root_id, scan_generation, guard),
    }
}

/// Shared reconcile+mark-scanned body for the `Some(writer)` (routed through
/// the single writer actor) and `None` (inline on the caller's connection)
/// branches above.
fn reconcile_scan_completion(
    database: &mut Database,
    root_id: i64,
    scan_generation: i64,
    guard: Option<TaskAttemptGuard>,
) -> anyhow::Result<()> {
    if let Some(guard) = guard {
        let reconciled = database.reconcile_root_scan_completion_for_task_attempt(
            root_id,
            scan_generation,
            guard.task_id,
            guard.attempt_generation,
        )?;
        if !reconciled {
            return Err(anyhow::anyhow!(
                "task {} attempt superseded before scan reconciliation",
                guard.task_id
            ));
        }
        database.mark_root_scanned_for_task_attempt(
            root_id,
            guard.task_id,
            guard.attempt_generation,
        )?;
    } else {
        database.reconcile_root_scan_completion(root_id, scan_generation)?;
        database.mark_root_scanned(root_id)?;
    }
    Ok(())
}

/// Resolve/create the folder chain for `folder_path` under `root_path`,
/// routing through the single writer actor when present, falling back to the
/// caller's connection inline for tests that call `scan_folder` directly with
/// `writer: None`.
fn ensure_folder_chain_with_optional_writer(
    database: &mut Database,
    root_id: i64,
    root_path: &Path,
    folder_path: &Path,
    options: &ScanOptions<'_>,
) -> anyhow::Result<i64> {
    crate::debug_log!(
        "scan ensure folder chain ({}){}",
        folder_path.display(),
        if options.writer.is_some() {
            " -> writer"
        } else {
            " (inline)"
        }
    );
    match options.writer.as_ref() {
        Some(writer) => {
            let root_path = root_path.to_path_buf();
            let folder_path = folder_path.to_path_buf();
            let guard = options.task_attempt_guard;
            writer.write_blocking(move |db| match guard {
                Some(guard) => db.ensure_folder_chain_for_path_for_task_attempt(
                    root_id,
                    &root_path,
                    &folder_path,
                    guard.task_id,
                    guard.attempt_generation,
                ),
                None => db.ensure_folder_chain_for_path(root_id, &root_path, &folder_path),
            })
        }
        None => match options.task_attempt_guard {
            Some(guard) => database.ensure_folder_chain_for_path_for_task_attempt(
                root_id,
                root_path,
                folder_path,
                guard.task_id,
                guard.attempt_generation,
            ),
            None => database.ensure_folder_chain_for_path(root_id, root_path, folder_path),
        },
    }
}

pub fn scan_folder_with_options(
    database: &mut Database,
    root: &RootRecord,
    folder_id: i64,
    folder_path: &Path,
    mut options: ScanOptions<'_>,
) -> anyhow::Result<ScanSummary> {
    let root_path = PathBuf::from(&root.path);
    check_scan_cancelled(&mut options)?;
    ensure_root_enabled(database, root.id)?;
    ensure_root_binding_current(database, &options)?;
    let selected_folder_id = ensure_folder_chain_with_optional_writer(
        database,
        root.id,
        &root_path,
        folder_path,
        &options,
    )?;
    if selected_folder_id != folder_id {
        return Err(anyhow::anyhow!(
            "folder path resolved to unexpected folder id: expected {folder_id}, got {selected_folder_id}"
        ));
    }

    let mut folder_ids = HashMap::new();
    folder_ids.insert(folder_path.to_path_buf(), selected_folder_id);
    let mut pending_files = Vec::new();
    let write_batch_size = options.write_batch_size.max(1);

    let mut summary = ScanSummary {
        folders_seen: 1,
        media_files_seen: 0,
        skipped_files: 0,
    };
    let mut items_seen = 0_i64;
    emit_progress(&mut options, items_seen, &summary);
    check_scan_cancelled(&mut options)?;

    for entry in WalkDir::new(folder_path).min_depth(1) {
        check_scan_cancelled(&mut options)?;
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        items_seen += 1;

        if metadata.is_dir() {
            let parent_id = parent_folder_id(path, &folder_ids)?;
            let batch = ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id: root.id,
                    parent_id: Some(parent_id),
                    name: file_name(path),
                    path_hash: hash_path(path),
                    mtime: metadata_time(Some(&metadata), TimeField::Modified),
                }],
                files: Vec::new(),
                scan_generation: None,
            };
            observe_scan_batch(&options, batch.folders.len(), batch.files.len());
            check_scan_cancelled(&mut options)?;
            ensure_root_enabled(database, root.id)?;
            let result = commit_scan_batch_with_optional_task_guard(database, batch, &options)?;
            let nested_folder_id = result.folder_ids[0];
            folder_ids.insert(path.to_path_buf(), nested_folder_id);
            summary.folders_seen += 1;
            emit_progress(&mut options, items_seen, &summary);
            check_scan_cancelled(&mut options)?;
            continue;
        }

        if !metadata.is_file() {
            summary.skipped_files += 1;
            emit_progress(&mut options, items_seen, &summary);
            check_scan_cancelled(&mut options)?;
            continue;
        }

        // One `lowercase_extension` allocation per file, not two: the guard and
        // the stored `files.ext` are the same string. This runs once per file on
        // multi-million-file libraries.
        let ext = extension(path);
        let Some(kind) = media_kind_for_extension(ext.as_str()) else {
            summary.skipped_files += 1;
            emit_progress(&mut options, items_seen, &summary);
            check_scan_cancelled(&mut options)?;
            continue;
        };

        let parent_id = parent_folder_id(path, &folder_ids)?;
        let file_identity = crate::library_path::local_file_identity(path).unwrap_or_default();
        pending_files.push(ScanFileUpsert {
            file: FileUpsert {
                root_id: root.id,
                folder_id: parent_id,
                name: file_name(path),
                ext,
                size: metadata.len() as i64,
                mtime: metadata_time(Some(&metadata), TimeField::Modified).unwrap_or(0),
                ctime: metadata_time(Some(&metadata), TimeField::Created),
                file_key: (!file_identity.is_empty()).then(|| file_identity.clone()),
            },
            media_kind: kind.to_string(),
            external_identity: Some(ExternalFileIdentity {
                volume_identity: crate::library_path::local_file_identity(&root_path)
                    .unwrap_or_else(|_| root.path.clone())
                    .into_bytes(),
                file_identity: file_identity.into_bytes(),
                content_fingerprint: crate::sync::identity::stable_content_fingerprint(path).ok(),
                path: path.to_path_buf(),
            }),
        });
        summary.media_files_seen += 1;
        emit_progress(&mut options, items_seen, &summary);
        check_scan_cancelled(&mut options)?;

        if pending_files.len() >= write_batch_size {
            flush_scan_files(database, root.id, None, &mut options, &mut pending_files)?;
        }
    }

    flush_scan_files(database, root.id, None, &mut options, &mut pending_files)?;
    check_scan_cancelled(&mut options)?;
    ensure_root_enabled(database, root.id)?;
    Ok(summary)
}

fn commit_scan_batch_with_optional_task_guard(
    database: &mut Database,
    batch: ScanWriteBatch,
    options: &ScanOptions<'_>,
) -> anyhow::Result<crate::db::ScanWriteBatchResult> {
    crate::debug_log!(
        "scan batch commit ({} folders, {} files){}",
        batch.folders.len(),
        batch.files.len(),
        if options.writer.is_some() {
            " -> writer"
        } else {
            " (inline)"
        }
    );
    let guard = options.task_attempt_guard;
    match options.writer.as_ref() {
        Some(writer) => writer.write_blocking(move |db| commit_scan_batch(db, batch, guard)),
        None => commit_scan_batch(database, batch, guard),
    }
}

/// Commit one scan batch (folders and/or files) against the given connection,
/// routing through the task-attempt-guarded write when a guard is present.
/// Shared by the `Some(writer)` (routed through the single writer actor) and
/// `None` (inline on the caller's connection) branches above.
fn commit_scan_batch(
    database: &mut Database,
    batch: ScanWriteBatch,
    guard: Option<TaskAttemptGuard>,
) -> anyhow::Result<crate::db::ScanWriteBatchResult> {
    if let Some(guard) = guard {
        return database.commit_scan_batch_for_task_attempt(
            batch,
            guard.task_id,
            guard.attempt_generation,
        );
    }
    database.commit_scan_batch(batch)
}

/// Commit a batch of folder rows (the structure-first pass) in one transaction
/// and record their assigned ids by path so descendant folders/files resolve
/// their parent. `commit_scan_batch` returns folder ids in the same order as the
/// input folders, so we zip the buffered paths back to the returned ids.
#[allow(clippy::too_many_arguments)]
fn flush_scan_folders(
    database: &mut Database,
    root_id: i64,
    scan_generation: Option<i64>,
    options: &mut ScanOptions<'_>,
    pending_folders: &mut Vec<(PathBuf, FolderUpsert)>,
    folder_ids: &mut HashMap<PathBuf, i64>,
    summary: &mut ScanSummary,
    items_seen: &mut i64,
) -> anyhow::Result<()> {
    if pending_folders.is_empty() {
        return Ok(());
    }
    let drained: Vec<(PathBuf, FolderUpsert)> = std::mem::take(pending_folders);
    let (paths, folders): (Vec<PathBuf>, Vec<FolderUpsert>) = drained.into_iter().unzip();
    let batch = ScanWriteBatch {
        folders,
        files: Vec::new(),
        scan_generation,
    };
    observe_scan_batch(options, batch.folders.len(), 0);
    check_scan_cancelled(options)?;
    ensure_root_enabled(database, root_id)?;
    let result = commit_scan_batch_with_optional_task_guard(database, batch, options)?;
    for (path, folder_id) in paths.into_iter().zip(result.folder_ids.into_iter()) {
        folder_ids.insert(path, folder_id);
        summary.folders_seen += 1;
    }
    // `items_seen` is counted once per walked entry by the main file pass (which
    // still walks every folder); the structure-first pass must NOT also count
    // folders here or they would be double-counted.
    emit_progress(options, *items_seen, summary);
    check_scan_cancelled(options)?;
    Ok(())
}

fn parent_folder_id(path: &Path, folder_ids: &HashMap<PathBuf, i64>) -> anyhow::Result<i64> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("scan path has no parent: {}", path.display()))?;
    folder_ids
        .get(parent)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("missing scanned parent folder id for {}", parent.display()))
}

fn flush_scan_files(
    database: &mut Database,
    root_id: i64,
    scan_generation: Option<i64>,
    options: &mut ScanOptions<'_>,
    pending_files: &mut Vec<ScanFileUpsert>,
) -> anyhow::Result<()> {
    if pending_files.is_empty() {
        return Ok(());
    }

    observe_scan_batch(options, 0, pending_files.len());
    check_scan_cancelled(options)?;
    ensure_root_enabled(database, root_id)?;
    commit_scan_batch_with_optional_task_guard(
        database,
        ScanWriteBatch {
            folders: Vec::new(),
            files: std::mem::take(pending_files),
            scan_generation,
        },
        options,
    )?;
    Ok(())
}

fn ensure_root_enabled(database: &Database, root_id: i64) -> anyhow::Result<()> {
    if database.root_enabled(root_id)? {
        return Ok(());
    }
    Err(anyhow::anyhow!("root is disabled: {root_id}"))
}

fn ensure_root_binding_current(
    database: &Database,
    options: &ScanOptions<'_>,
) -> anyhow::Result<()> {
    if let Some(token) = options.root_binding_guard.as_ref() {
        database.require_sync_root_binding_token(token)?;
    }
    Ok(())
}

fn observe_scan_batch(options: &ScanOptions, folders: usize, files: usize) {
    #[cfg(test)]
    if let Some(observer) = &options.batch_observer {
        observer(ScanBatchStats { folders, files });
    }

    #[cfg(not(test))]
    let _ = (options, folders, files);
}

fn check_scan_cancelled(options: &mut ScanOptions<'_>) -> anyhow::Result<()> {
    if let Some(callback) = options.cancellation_callback.as_deref_mut() {
        callback()?;
    }
    Ok(())
}

fn emit_progress(options: &mut ScanOptions<'_>, items_seen: i64, summary: &ScanSummary) {
    if let Some(callback) = options.progress_callback.as_deref_mut() {
        callback(TaskScanProgress {
            items_seen,
            items_total: None,
            folders_seen: summary.folders_seen as i64,
            media_files_seen: summary.media_files_seen as i64,
            skipped_files: summary.skipped_files as i64,
        });
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

fn hash_path(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone, Copy)]
enum TimeField {
    Created,
    Modified,
}

fn metadata_time(metadata: Option<&Metadata>, field: TimeField) -> Option<i64> {
    let metadata = metadata?;
    let time = match field {
        TimeField::Created => metadata.created(),
        TimeField::Modified => metadata.modified(),
    }
    .ok()?;
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::db::{Database, MediaPageQuery, NewRoot};

    #[test]
    fn scan_options_default_uses_background_write_batch() {
        assert_eq!(ScanOptions::default().write_batch_size, 256);
    }

    #[test]
    fn scan_options_interactive_profile_uses_single_file_write_batch() {
        assert_eq!(ScanOptions::interactive().write_batch_size, 1);
    }

    #[test]
    fn scan_root_indexes_media_files_and_skips_non_media() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("photos")).expect("create photos dir");
        fs::write(temp_root.join("photos").join("image.JPG"), b"fake jpg").expect("write image");
        fs::write(temp_root.join("clip.mp4"), b"fake mp4").expect("write video");
        fs::write(temp_root.join("notes.txt"), b"not media").expect("write notes");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "scan-test".to_string(),
            })
            .expect("add root");
        let root = database
            .list_roots()
            .expect("list roots")
            .into_iter()
            .find(|item| item.id == root_id)
            .expect("find root");

        let summary = scan_root(&mut database, &root).expect("scan root");
        assert_eq!(summary.folders_seen, 2);
        assert_eq!(summary.media_files_seen, 2);
        assert_eq!(summary.skipped_files, 1);

        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(page.items.len(), 2);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_indexes_heif_as_an_image_through_the_shared_format_table() {
        // The real join for `.heif`. Scan is the production consumer of
        // `decode::formats::IMAGE_EXTENSIONS`, so this is where "is it in the
        // table" meets "does a real scan actually index it". The old private
        // per-file tables listed `.heic` but not `.heif`; this locks that drift
        // shut at the production entry point, which asserting on the constant
        // alone cannot do.
        //
        // Deliberately says nothing about decoding or thumbnails, so it holds
        // whether or not ffmpeg is installed.
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create temp root");
        // Uppercase on purpose: proves the lowercasing in `lowercase_extension`
        // is on the indexing path too.
        fs::write(temp_root.join("photo.HEIF"), b"fake heif").expect("write heif");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "heif-scan-test".to_string(),
            })
            .expect("add root");
        let root = database
            .list_roots()
            .expect("list roots")
            .into_iter()
            .find(|item| item.id == root_id)
            .expect("find root");

        let summary = scan_root(&mut database, &root).expect("scan root");
        // Dropping `.heif` from IMAGE_EXTENSIONS flips these to 0 / 1.
        assert_eq!(summary.media_files_seen, 1);
        assert_eq!(summary.skipped_files, 0);

        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].name, "photo.HEIF");
        assert_eq!(page.items[0].kind.as_deref(), Some("image"));

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_with_tiny_write_batch_preserves_summary_and_media_rows() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("photos").join("raw")).expect("create photos dir");
        fs::write(temp_root.join("photos").join("image.JPG"), b"fake jpg").expect("write image");
        fs::write(
            temp_root.join("photos").join("raw").join("scan.png"),
            b"fake png",
        )
        .expect("write png");
        fs::write(temp_root.join("clip.mp4"), b"fake mp4").expect("write video");
        fs::write(temp_root.join("notes.txt"), b"not media").expect("write notes");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "tiny-batch-scan-test".to_string(),
            })
            .expect("add root");
        let root = database
            .list_roots()
            .expect("list roots")
            .into_iter()
            .find(|item| item.id == root_id)
            .expect("find root");

        let summary = scan_root_with_options(
            &mut database,
            &root,
            ScanOptions {
                write_batch_size: 2,
                progress_callback: None,
                cancellation_callback: None,
                task_attempt_guard: None,
                root_binding_guard: None,
                writer: None,
                batch_observer: None,
            },
        )
        .expect("scan root");
        assert_eq!(summary.folders_seen, 3);
        assert_eq!(summary.media_files_seen, 3);
        assert_eq!(summary.skipped_files, 1);

        let root = database
            .get_root(root_id)
            .expect("get root")
            .expect("root exists");
        assert!(root.last_scan_at.is_some());

        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(page.items.len(), 3);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_places_nested_files_under_nested_folder_ids() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("photos").join("raw")).expect("create raw dir");
        fs::write(
            temp_root.join("photos").join("raw").join("scan.png"),
            b"fake png",
        )
        .expect("write png");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "nested-folder-scan-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("scan root");

        let root = database
            .get_root(root_id)
            .expect("get root")
            .expect("root exists");
        let root_folder_id = root.root_folder_id.expect("root folder id");
        let photos_folder = only_child_named(&database, root_folder_id, "photos");
        let raw_folder = only_child_named(&database, photos_folder.id, "raw");
        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(raw_folder.id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");

        assert_eq!(media.items.len(), 1);
        assert_eq!(media.items[0].name, "scan.png");
        assert_eq!(media.items[0].folder_id, raw_folder.id);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn repeated_scan_root_does_not_duplicate_folder_file_or_media_rows() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("photos").join("raw")).expect("create raw dir");
        fs::write(temp_root.join("clip.mp4"), b"fake mp4").expect("write mp4");
        fs::write(
            temp_root.join("photos").join("raw").join("scan.png"),
            b"fake png",
        )
        .expect("write png");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "repeat-scan-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("first scan");
        scan_root(&mut database, &root).expect("second scan");

        let root = database
            .get_root(root_id)
            .expect("get root")
            .expect("root exists");
        let root_folder_id = root.root_folder_id.expect("root folder id");
        let root_children = database
            .list_folder_children(root_folder_id)
            .expect("list root children");
        assert_eq!(root_children.len(), 1);
        let raw_children = database
            .list_folder_children(root_children[0].id)
            .expect("list photos children");
        assert_eq!(raw_children.len(), 1);

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(media.items.len(), 2);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn full_rescan_fingerprint_rename_reuses_asset_without_outbox_echo() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).unwrap();
        let old_path = temp_root.join("old.jpg");
        let new_path = temp_root.join("new.jpg");
        fs::write(&old_path, b"stable full scan fingerprint").unwrap();
        let mut database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let root_id = add_test_root(&database, &temp_root, "scan-fingerprint-rename");
        let root = test_root(&database, root_id);
        scan_root(&mut database, &root).unwrap();
        let old_asset: Vec<u8> = database
            .connection_for_fsops()
            .query_row(
                "SELECT asset_uuid FROM files WHERE name='old.jpg'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        database.connection_for_fsops().execute(
            "UPDATE sync_local_file_identities SET file_identity=randomblob(16) WHERE asset_uuid=?1",
            [&old_asset],
        ).unwrap();
        fs::rename(&old_path, &new_path).unwrap();
        scan_root(&mut database, &root).unwrap();
        let (asset, active, total, outbox): (Vec<u8>, i64, i64, i64) = database.connection_for_fsops().query_row(
            "SELECT (SELECT asset_uuid FROM files WHERE name='new.jpg'),(SELECT COUNT(*) FROM files WHERE status='active'),(SELECT COUNT(*) FROM files),(SELECT COUNT(*) FROM sync_outbox)", [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).unwrap();
        assert_eq!(asset, old_asset);
        assert_eq!((active, total, outbox), (1, 1, 0));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn completed_scan_marks_deleted_media_missing() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("keep.jpg"), b"keep").expect("write keep image");
        fs::write(temp_root.join("delete.jpg"), b"delete").expect("write delete image");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "scan-reconcile-delete-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("first scan");
        fs::remove_file(temp_root.join("delete.jpg")).expect("delete image");
        scan_root(&mut database, &root).expect("second scan");

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        let names: Vec<&str> = media.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["keep.jpg"]);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn completed_scan_marks_moved_out_media_missing() {
        let temp_root = unique_temp_dir();
        let outside_dir = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::create_dir_all(&outside_dir).expect("create outside dir");
        fs::write(temp_root.join("move-out.jpg"), b"move").expect("write image");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "scan-reconcile-move-out-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("first scan");
        fs::rename(
            temp_root.join("move-out.jpg"),
            outside_dir.join("move-out.jpg"),
        )
        .expect("move image outside root");
        scan_root(&mut database, &root).expect("second scan");

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert!(media.items.is_empty());

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
        fs::remove_dir_all(outside_dir).expect("cleanup outside dir");
    }

    #[test]
    fn completed_scan_removes_thumb_blob_for_missing_media() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let delete_path = temp_root.join("delete.jpg");
        write_test_image(&delete_path, 800, 400);

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "scan-reconcile-delete-blob-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("first scan");
        let media = single_media(&database, root_id);
        let source_fingerprint = database
            .get_thumbnail_source(media.id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                file_id: media.id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: None,
                width: Some(320),
                height: Some(160),
                byte_size: Some(42),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("seed ready thumbnail");
        database
            .upsert_thumb_blob(crate::db::ThumbBlobRecord {
                file_id: media.id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                data: vec![1, 2, 3, 4],
                width: 320,
                height: 160,
                byte_size: 4,
                output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .expect("seed thumb blob");

        fs::remove_file(delete_path).expect("delete image");
        scan_root(&mut database, &root).expect("second scan");

        assert!(database
            .get_thumb_blob(media.id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumb blob")
            .is_none());

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn folder_boundaries_do_not_flush_pending_files_before_batch_threshold() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("after-file")).expect("create after-file dir");
        fs::create_dir_all(temp_root.join("after-file-2")).expect("create after-file-2 dir");
        fs::write(temp_root.join("a.mp4"), b"fake mp4").expect("write first video");
        fs::write(temp_root.join("b.mp4"), b"fake mp4").expect("write second video");
        let observed = Rc::new(RefCell::new(Vec::new()));

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "batch-boundary-scan-test");
        let root = test_root(&database, root_id);

        let observed_batches = Rc::clone(&observed);
        scan_root_with_options(
            &mut database,
            &root,
            ScanOptions {
                write_batch_size: 10,
                progress_callback: None,
                cancellation_callback: None,
                task_attempt_guard: None,
                root_binding_guard: None,
                writer: None,
                batch_observer: Some(Rc::new(move |stats| {
                    observed_batches.borrow_mut().push(stats);
                })),
            },
        )
        .expect("scan root");

        let file_commit_sizes: Vec<usize> = observed
            .borrow()
            .iter()
            .filter_map(|stats| (stats.files > 0).then_some(stats.files))
            .collect();
        assert_eq!(file_commit_sizes, vec![2]);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_stops_before_writing_next_batch_when_root_is_disabled() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("a.jpg"), b"fake jpg").expect("write first image");
        fs::write(temp_root.join("b.jpg"), b"fake jpg").expect("write second image");
        fs::write(temp_root.join("c.jpg"), b"fake jpg").expect("write third image");

        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("scan-cancel.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "disable-during-scan-test");
        let root = test_root(&database, root_id);

        let disable_db_path = db_path.clone();
        let mut disabled = false;
        let mut progress_callback = move |progress: TaskScanProgress| {
            if !disabled && progress.media_files_seen >= 1 {
                let disable_database = Database::open(&disable_db_path).expect("open disable db");
                disable_database
                    .disable_root(root_id)
                    .expect("disable root");
                disabled = true;
            }
        };

        let error = scan_root_with_options(
            &mut database,
            &root,
            ScanOptions {
                write_batch_size: 2,
                progress_callback: Some(&mut progress_callback),
                cancellation_callback: None,
                task_attempt_guard: None,
                root_binding_guard: None,
                writer: None,
                batch_observer: None,
            },
        )
        .expect_err("disabled root should stop scan");
        assert!(error.to_string().contains("disabled"));

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert!(media.items.is_empty());

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn guarded_scan_completion_does_not_reconcile_when_task_attempt_is_superseded() {
        // Regression for Blocker 2: a guarded scan whose task attempt has
        // been superseded must not finish reconciliation. It must error out
        // (the existing batch guard rejects writes against a stale attempt,
        // and the new task-attempt-guarded reconciliation refuses to run
        // against a stale attempt), and it must leave the new (retried)
        // attempt and root state untouched: the root is not marked scanned
        // and the retried attempt remains pending.
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("a.jpg"), b"fake jpg").expect("write image a");
        fs::write(temp_root.join("b.jpg"), b"fake jpg").expect("write image b");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "stale-attempt-scan-test");
        let root = test_root(&database, root_id);

        // Persist a task and mark its first attempt running. The scan code
        // expects the task to already be running before a guarded scan.
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_task_running_for_attempt(task_id, old_attempt)
            .expect("mark running");

        // Supersede the attempt before the scan starts: cancel + retry
        // bumps the attempt generation, so the in-flight scan is now stale.
        database.cancel_task(task_id).expect("cancel task");
        database.retry_task(task_id).expect("retry task");

        let error = scan_root_with_options(
            &mut database,
            &root,
            ScanOptions {
                write_batch_size: DEFAULT_SCAN_WRITE_BATCH_SIZE,
                progress_callback: None,
                cancellation_callback: None,
                task_attempt_guard: Some(TaskAttemptGuard {
                    task_id,
                    attempt_generation: old_attempt,
                }),
                root_binding_guard: None,
                writer: None,
                batch_observer: None,
            },
        )
        .expect_err("stale attempt scan must not succeed");
        let message = error.to_string();
        assert!(
            message.contains("attempt") || message.contains("superseded"),
            "unexpected error message: {message}"
        );

        // The retried (current) attempt remains pending and unmodified.
        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "pending");
        assert!(task.attempt_generation > old_attempt);

        // The root must not have been marked scanned by the stale path.
        let root = database.get_root(root_id).expect("get root").expect("root");
        assert_eq!(root.last_scan_at, None);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_checks_cancellation_before_flushing_pending_files() {
        use std::cell::Cell;
        use std::rc::Rc;

        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("a.jpg"), b"fake jpg").expect("write first image");
        fs::write(temp_root.join("b.jpg"), b"fake jpg").expect("write second image");
        fs::write(temp_root.join("c.jpg"), b"fake jpg").expect("write third image");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "cancel-callback-scan-test");
        let root = test_root(&database, root_id);

        let cancel_requested = Rc::new(Cell::new(false));
        let progress_cancel_requested = Rc::clone(&cancel_requested);
        let mut progress_callback = |progress: TaskScanProgress| {
            if progress.media_files_seen >= 1 {
                progress_cancel_requested.set(true);
            }
        };
        let callback_cancel_requested = Rc::clone(&cancel_requested);
        let mut cancellation_callback = || {
            if callback_cancel_requested.get() {
                return Err(anyhow::anyhow!("task cancelled: test"));
            }
            Ok(())
        };

        let error = scan_root_with_options(
            &mut database,
            &root,
            ScanOptions {
                write_batch_size: 10,
                progress_callback: Some(&mut progress_callback),
                cancellation_callback: Some(&mut cancellation_callback),
                task_attempt_guard: None,
                root_binding_guard: None,
                writer: None,
                batch_observer: None,
            },
        )
        .expect_err("cancelled task should stop scan");
        assert!(error.to_string().contains("cancelled"));

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert!(media.items.is_empty());

        let root = database.get_root(root_id).expect("get root").expect("root");
        assert_eq!(root.last_scan_at, None);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_leaves_image_dimensions_pending_for_later_metadata_work() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let image_path = temp_root.join("photo.png");
        let buffer = image::ImageBuffer::from_fn(800u32, 600u32, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 0])
        });
        image::DynamicImage::ImageRgb8(buffer)
            .save(&image_path)
            .expect("write 800x600 png");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "scan-metadata-pending-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("scan root");

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(media.items.len(), 1);
        let entry = &media.items[0];
        assert_eq!(entry.name, "photo.png");
        assert_eq!(entry.width, None);
        assert_eq!(entry.height, None);

        let source = database
            .get_thumbnail_source(entry.id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(source.metadata_status.as_deref(), Some("pending"));

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_does_not_generate_preview_placeholder_inline() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("image.jpg"), 800, 400);

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "disclosure-scan");
        let root = database
            .get_root(root_id)
            .expect("get root")
            .expect("root exists");

        scan_root(&mut database, &root).expect("scan root");

        let media = single_media(&database, root_id);
        assert!(media.preview_placeholder.is_none());
        let source = database
            .get_thumbnail_source(media.id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(source.metadata_status.as_deref(), Some("pending"));

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_preserves_metadata_and_placeholder_for_unchanged_files() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("image.jpg"), 800, 400);

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "unchanged-rescan");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("initial scan");
        let media = single_media(&database, root_id);
        database
            .update_media_dimensions(media.id, 800, 400)
            .expect("seed dimensions");
        database
            .update_media_preview_placeholder(media.id, b"cached-preview", "image/webp")
            .expect("seed placeholder");

        scan_root(&mut database, &root).expect("unchanged rescan");

        let media = single_media(&database, root_id);
        assert_eq!(media.width, Some(800));
        assert_eq!(media.height, Some(400));
        assert_eq!(
            media.preview_placeholder.as_deref(),
            Some(&b"cached-preview"[..])
        );
        assert_eq!(
            media.preview_placeholder_format.as_deref(),
            Some("image/webp")
        );
        let source = database
            .get_thumbnail_source(media.id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(source.metadata_status.as_deref(), Some("ready"));

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_folder_preserves_ready_thumbnail_for_unchanged_files() {
        let temp_root = unique_temp_dir();
        let folder_path = temp_root.join("selected");
        fs::create_dir_all(&folder_path).expect("create selected folder");
        let image_path = folder_path.join("image.jpg");
        write_test_image(&image_path, 800, 400);

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "unchanged-folder-thumb");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("initial scan");
        let root_record = database
            .get_root(root_id)
            .expect("get root")
            .expect("root exists");
        let root_folder_id = root_record.root_folder_id.expect("root folder id");
        let selected_folder = only_child_named(&database, root_folder_id, "selected");
        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(selected_folder.id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list selected media");
        let file_id = media.items[0].id;
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: None,
                width: Some(320),
                height: Some(160),
                byte_size: Some(42),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("seed ready thumbnail");

        scan_folder_with_options(
            &mut database,
            &root,
            selected_folder.id,
            &folder_path,
            ScanOptions::interactive(),
        )
        .expect("interactive folder rescan");

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "ready");

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_resets_metadata_and_placeholder_when_file_identity_changes() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create media root");
        let image_path = temp_root.join("image.jpg");
        write_test_image(&image_path, 800, 400);

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "changed-rescan");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("initial scan");
        let media = single_media(&database, root_id);
        database
            .update_media_dimensions(media.id, 800, 400)
            .expect("seed dimensions");
        database
            .update_media_preview_placeholder(media.id, b"cached-preview", "image/webp")
            .expect("seed placeholder");

        fs::write(&image_path, b"changed image bytes").expect("change file identity");
        scan_root(&mut database, &root).expect("changed rescan");

        let media = single_media(&database, root_id);
        assert_eq!(media.width, None);
        assert_eq!(media.height, None);
        assert!(media.preview_placeholder.is_none());
        let source = database
            .get_thumbnail_source(media.id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(source.metadata_status.as_deref(), Some("pending"));

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[test]
    fn scan_root_keeps_broken_image_metadata_pending() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("broken.png"), b"").expect("write broken png");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_test_root(&database, &temp_root, "scan-dimension-probe-error-test");
        let root = test_root(&database, root_id);

        scan_root(&mut database, &root).expect("scan root succeeds despite probe failure");

        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(media.items.len(), 1);
        let entry = &media.items[0];
        assert_eq!(entry.width, None);
        assert_eq!(entry.height, None);
        let source = database
            .get_thumbnail_source(entry.id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(source.metadata_status.as_deref(), Some("pending"));

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    /// Drive the real scan leaf with the writer path (`Some(writer)`), so
    /// batch commits are routed through the single writer actor exactly as
    /// production does. Runs on a blocking thread (as the real worker does),
    /// because `write_blocking` cannot block a runtime worker thread. Returns
    /// the owned worker connection so the caller can read it back.
    async fn scan_root_via_writer_for_test(
        mut worker_database: Database,
        writer: crate::db::DbWriter,
        root: RootRecord,
    ) -> (Database, anyhow::Result<ScanSummary>) {
        tokio::task::spawn_blocking(move || {
            let result = scan_root_with_options(
                &mut worker_database,
                &root,
                ScanOptions {
                    write_batch_size: 2,
                    progress_callback: None,
                    cancellation_callback: None,
                    task_attempt_guard: None,
                    root_binding_guard: None,
                    writer: Some(writer),
                    batch_observer: None,
                },
            );
            (worker_database, result)
        })
        .await
        .expect("join scan blocking task")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn scan_commits_through_writer_keep_wal_bounded() {
        use crate::db::DbWriter;

        let media_root = unique_temp_dir();
        fs::create_dir_all(&media_root).expect("create media root");
        for index in 0..40 {
            fs::write(media_root.join(format!("image_{index}.jpg")), b"fake jpg")
                .expect("write image");
        }

        let dir = unique_temp_dir();
        fs::create_dir_all(&dir).expect("create db dir");
        let db_path = dir.join("scan-writer.sqlite");
        let seed = Database::open(&db_path).expect("open");
        seed.apply_migrations().expect("migrate");
        let root_id = add_test_root(&seed, &media_root, "scan-writer-test");
        let root = test_root(&seed, root_id);

        let worker_database = seed.reopen().expect("reopen").expect("worker conn");
        let writer = DbWriter::spawn(seed);

        // Scan routed through the writer (the code path under test).
        let (worker_database, result) =
            scan_root_via_writer_for_test(worker_database, writer, root).await;
        result.expect("scan via writer");

        // The worker connection's WAL is bounded (writer checkpointed it).
        assert!(
            worker_database.wal_size_bytes() < 64 * 1024 * 1024,
            "scan WAL must stay bounded, got {}",
            worker_database.wal_size_bytes()
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&media_root);
    }

    /// Drive `scan_folder_with_options` (the interactive-folder-scan leaf)
    /// with `Some(writer)`, mirroring `scan_root_via_writer_for_test` above.
    /// Before this task, `scan_folder_with_options` called
    /// `ensure_folder_chain_for_path` directly on the caller's connection
    /// regardless of `options.writer`; this proves the writer-routed branch
    /// (which production's `run_task` exercises via `interactive_scan_options`
    /// feeding a real `DbWriter`) resolves the folder and completes the scan.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn scan_folder_routes_folder_chain_through_writer() {
        use crate::db::DbWriter;

        let temp_root = unique_temp_dir();
        let folder_path = temp_root.join("selected");
        fs::create_dir_all(&folder_path).expect("create selected folder");
        fs::write(folder_path.join("image.jpg"), b"fake jpg").expect("write image");

        let dir = unique_temp_dir();
        fs::create_dir_all(&dir).expect("create db dir");
        let db_path = dir.join("scan-folder-writer.sqlite");
        let seed = Database::open(&db_path).expect("open");
        seed.apply_migrations().expect("migrate");
        let root_id = add_test_root(&seed, &temp_root, "scan-folder-writer-test");
        let root = test_root(&seed, root_id);

        // Resolve the selected folder id up front on a throwaway connection,
        // mirroring how the API resolves `folder_id` before enqueuing an
        // interactive folder scan task.
        let folder_id = seed
            .ensure_folder_chain_for_path(root_id, &temp_root, &folder_path)
            .expect("resolve selected folder id");

        let worker_database = seed.reopen().expect("reopen").expect("worker conn");
        let writer = DbWriter::spawn(seed);

        let writer_for_task = writer.clone();
        let (worker_database, result) = tokio::task::spawn_blocking(move || {
            let mut worker_database = worker_database;
            let result = scan_folder_with_options(
                &mut worker_database,
                &root,
                folder_id,
                &folder_path,
                ScanOptions {
                    writer: Some(writer_for_task),
                    ..ScanOptions::interactive()
                },
            );
            (worker_database, result)
        })
        .await
        .expect("join scan_folder blocking task");
        result.expect("interactive folder scan via writer");

        assert!(
            worker_database.wal_size_bytes() < 64 * 1024 * 1024,
            "scan_folder WAL must stay bounded, got {}",
            worker_database.wal_size_bytes()
        );

        let media = single_media(&worker_database, root_id);
        assert_eq!(media.name, "image.jpg");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&temp_root);
    }

    fn single_media(database: &Database, root_id: i64) -> crate::db::MediaRecord {
        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(media.items.len(), 1);
        media.items.into_iter().next().expect("media exists")
    }

    fn add_test_root(database: &Database, temp_root: &Path, display_name: &str) -> i64 {
        database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: display_name.to_string(),
            })
            .expect("add root")
    }

    fn test_root(database: &Database, root_id: i64) -> RootRecord {
        database
            .list_roots()
            .expect("list roots")
            .into_iter()
            .find(|item| item.id == root_id)
            .expect("find root")
    }

    fn only_child_named(
        database: &Database,
        folder_id: i64,
        name: &str,
    ) -> crate::db::FolderRecord {
        let children = database
            .list_folder_children(folder_id)
            .expect("list folder children");
        children
            .into_iter()
            .find(|folder| folder.name == name)
            .unwrap_or_else(|| panic!("missing child folder {name} under {folder_id}"))
    }

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "megle_scan_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn write_test_image(path: &Path, width: u32, height: u32) {
        let buffer = image::ImageBuffer::from_fn(width, height, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 32])
        });
        image::DynamicImage::ImageRgb8(buffer)
            .save(path)
            .expect("write test image");
    }
}

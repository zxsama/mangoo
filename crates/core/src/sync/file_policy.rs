//! Device-local collection policy. Content eligibility is independent of decoding.
use std::path::{Component, Path};

#[derive(Clone, Default)]
pub struct StorageSnapshotSource(
    std::sync::Arc<std::sync::RwLock<(u64, std::sync::Arc<Vec<std::path::PathBuf>>)>>,
);

impl StorageSnapshotSource {
    pub fn publish(&self, paths: Vec<std::path::PathBuf>) {
        let mut snapshot = self.0.write().unwrap_or_else(|p| p.into_inner());
        *snapshot = (snapshot.0.wrapping_add(1), std::sync::Arc::new(paths));
    }
    pub fn invalidate(&self) {
        let mut snapshot = self.0.write().unwrap_or_else(|p| p.into_inner());
        snapshot.0 = snapshot.0.wrapping_add(1);
    }
}

#[derive(Default)]
pub struct StoragePathPolicy {
    pub source: StorageSnapshotSource,
    cache: std::cell::RefCell<(
        u64,
        std::collections::HashMap<std::path::PathBuf, (String, bool)>,
    )>,
}

impl StoragePathPolicy {
    pub fn reset(&self) {
        self.cache.borrow_mut().1.clear();
    }
    pub fn excludes(&self, root: &Path, path: &Path) -> anyhow::Result<bool> {
        let (epoch, known) = self
            .source
            .0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if known
            .iter()
            .any(|storage| path == storage || path.starts_with(storage))
        {
            return Ok(true);
        }
        let mut cache = self.cache.borrow_mut();
        if cache.0 != epoch {
            cache.0 = epoch;
            cache.1.clear();
        }
        // Inspect ancestors only; never enumerate content or query the Host.
        for directory in path
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(root))
        {
            let metadata = match std::fs::symlink_metadata(directory) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !metadata.is_dir() {
                continue;
            }
            if is_link(&metadata) {
                return Ok(true);
            }
            let identity = crate::library_path::local_file_identity(directory)?;
            if let Some((previous, owned)) = cache.1.get(directory) {
                if *previous == identity {
                    if *owned {
                        return Ok(true);
                    } else {
                        continue;
                    }
                }
            }
            let marker = read_control_json::<crate::library_path::LibraryMarker>(
                &directory.join(crate::library_path::MARKER_FILE_NAME),
            )?;
            let owned = marker.is_some_and(|marker| {
                marker.format_version == crate::library_path::STORAGE_FORMAT_VERSION
                    && [
                        &marker.library_uuid,
                        &marker.root_uuid,
                        &marker.creation_txn_uuid,
                    ]
                    .iter()
                    .all(|value| {
                        uuid::Uuid::parse_str(value)
                            .is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == **value)
                    })
            });
            if cache.1.len() >= 4096 {
                cache.1.clear();
            }
            cache.1.insert(directory.to_path_buf(), (identity, owned));
            if owned {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub fn is_owned_path(
    connection: &rusqlite::Connection,
    root: &Path,
    path: &Path,
) -> anyhow::Result<bool> {
    use rusqlite::OptionalExtension;
    let relative = path.strip_prefix(root)?;
    let mut current = root.to_path_buf();
    for (depth, component) in relative.components().enumerate() {
        let Component::Normal(value) = component else {
            anyhow::bail!("unsupported relative file path");
        };
        current.push(value);
        let Some(name) = value.to_str() else { continue };
        #[cfg(windows)]
        let lowercase = name.to_ascii_lowercase();
        #[cfg(windows)]
        let name = lowercase.as_str();
        if depth == 0
            && (name == ".mangoo-sync-root"
                || name
                    .strip_prefix(".mangoo-sync-root.")
                    .and_then(|s| s.strip_suffix(".tmp"))
                    .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()))
        {
            if let Some(marker) =
                read_control_json::<crate::sync_folders::SyncRootMarker>(&current)?
            {
                let owned: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM roots r JOIN sync_root_local_bindings b ON b.root_id=r.id WHERE r.path=?1 AND b.library_uuid=?2 AND b.root_uuid=?3 AND r.root_folder_uuid=?4 AND (b.local_binding_uuid=?5 OR ?7) AND b.binding_generation>=?6)",
                    rusqlite::params![root.to_string_lossy(), uuid_bytes(&marker.library_uuid), uuid_bytes(&marker.root_uuid), uuid_bytes(&marker.root_folder_uuid), uuid_bytes(&marker.local_binding_uuid), marker.binding_generation, name != ".mangoo-sync-root"],
                    |row| row.get(0))?;
                if marker.format_version == 1
                    && marker.binding_generation > 0
                    && uuid::Uuid::parse_str(&marker.local_binding_uuid).is_ok()
                    && owned
                {
                    return Ok(true);
                }
            }
        }
        if depth == 0 && name == ".mangoo-library" {
            if let Some(marker) = read_control_json::<crate::library_path::LibraryMarker>(&current)?
            {
                let owned: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM roots r, sync_local_state s WHERE r.path=?1 AND r.root_uuid=?2 AND s.library_uuid=?3)",
                    rusqlite::params![root.to_string_lossy(), uuid_bytes(&marker.root_uuid), uuid_bytes(&marker.library_uuid)], |row| row.get(0))?;
                if marker.format_version == 1
                    && owned
                    && uuid::Uuid::parse_str(&marker.creation_txn_uuid).is_ok()
                {
                    return Ok(true);
                }
            }
        }
        if depth == 0 {
            if let Some(revision) = name
                .strip_prefix(".mangoo-work-")
                .and_then(|s| s.get(..32))
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
            {
                let staging: Option<(String, Option<String>, Option<String>)> = connection.query_row(
                    "SELECT j.staging_relpath,j.staging_directory_identity,j.staging_file_identity FROM media_materialization_jobs j JOIN files f ON f.asset_uuid=j.asset_uuid JOIN roots r ON r.id=f.root_id WHERE j.content_revision_uuid=?1 AND r.path=?2 AND j.staging_relpath IS NOT NULL",
                    rusqlite::params![revision.as_bytes().as_slice(),root.to_string_lossy()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
                if let Some((staging, expected, file_identity)) = staging {
                    let partial = root.join(staging);
                    if partial.parent() == Some(current.as_path()) {
                        if std::fs::symlink_metadata(&current).is_ok() {
                            let Some(expected) = expected else {
                                anyhow::bail!("materialization staging ownership is pending; retry observation");
                            };
                            anyhow::ensure!(
                                crate::library_path::local_file_identity(&current)? == expected,
                                "materialization staging identity changed; retry observation"
                            );
                            // A private empty/payload-only container must not emit
                            // portable folder events. At most two entries establish
                            // whether user content requires normal traversal.
                            let only_payload = if path == current {
                                let mut entries = std::fs::read_dir(&current)?;
                                let first = entries.next().transpose()?;
                                match first {
                                    None => true,
                                    Some(entry) if entry.path() == partial => {
                                        entries.next().transpose()?.is_none()
                                    }
                                    Some(_) => false,
                                }
                            } else {
                                false
                            };
                            if path == partial || only_payload {
                                let file_identity = file_identity.ok_or_else(|| anyhow::anyhow!("materialization payload ownership pending; retry observation"))?;
                                match crate::library_path::local_file_identity(&partial) {
                                    Ok(actual) => anyhow::ensure!(actual == file_identity, "materialization payload identity changed; retry observation"),
                                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(only_payload),
                                    Err(error) => return Err(error.into()),
                                }
                                return Ok(true);
                            }
                            return Ok(false);
                        }
                    }
                }
            }
        }
        if name
            .strip_prefix(".mangoo-recycle-")
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
        {
            let expected: Option<String> = connection.query_row(
                "SELECT o.staging_identity FROM file_operations o JOIN roots r ON r.id=o.root_id WHERE r.path=?1 AND o.target_path=?2 AND o.operation='delete_recycle' AND o.staging_identity IS NOT NULL ORDER BY o.id DESC LIMIT 1",
                rusqlite::params![root.to_string_lossy(),current.to_string_lossy()], |row| row.get(0)).optional()?;
            if let Some(expected) = expected {
                if crate::library_path::local_file_identity(&current)
                    .is_ok_and(|actual| actual == expected)
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn uuid_bytes(value: &str) -> Option<Vec<u8>> {
    uuid::Uuid::parse_str(value)
        .ok()
        .map(|uuid| uuid.as_bytes().to_vec())
}

fn read_control_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > 16 * 1024 {
        return Ok(None);
    }
    let (file, _guard) = open_source(path)?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Ok(None);
    }
    Ok(serde_json::from_slice(&bytes).ok())
}

pub fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

/// Recheck every existing component at source/effect admission. Never follow a
/// directory link introduced after enumeration. Missing descendants are allowed
/// for destination admission; the nearest existing parent is still checked.
pub fn require_no_links(root: &Path, path: &Path) -> anyhow::Result<()> {
    let relative = path.strip_prefix(root)?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            let Component::Normal(name) = component else {
                anyhow::bail!("unsupported relative file path")
            };
            current.push(name);
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => anyhow::ensure!(
                !is_link(&metadata),
                "file path contains an unsupported link"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn require_regular_source(path: &Path) -> anyhow::Result<()> {
    let root = path
        .ancestors()
        .last()
        .ok_or_else(|| anyhow::anyhow!("file has no path root"))?;
    require_no_links(root, path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file() && !is_link(&metadata),
        "source is not an ordinary file"
    );
    Ok(())
}

/// Windows parent handles deny renames while the source handle denies competing
/// writes/deletes. The same existing no-follow primitive protects root markers.
pub fn open_source(
    path: &Path,
) -> anyhow::Result<(std::fs::File, crate::library_path::MarkerDirectoryGuard)> {
    require_regular_source(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("file parent missing"))?;
    let directory = crate::library_path::MarkerDirectoryGuard::open_readonly(parent)?;
    let file = directory.open_marker(
        path.file_name()
            .ok_or_else(|| anyhow::anyhow!("file name missing"))?,
        false,
    )?;
    Ok((file, directory))
}

pub fn pin_destination_parents(
    root: &Path,
    target: &Path,
) -> anyhow::Result<Vec<crate::library_path::MarkerDirectoryGuard>> {
    let relative = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("destination parent missing"))?
        .strip_prefix(root)?;
    let mut guards = vec![crate::library_path::MarkerDirectoryGuard::open(root)?];
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("invalid destination component")
        };
        path.push(name);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        guards.push(crate::library_path::MarkerDirectoryGuard::open(&path)?);
    }
    Ok(guards)
}

pub fn unicode_name(path: &Path) -> anyhow::Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            anyhow::anyhow!("file name is not representable by the Unicode sync protocol")
        })
}

#[cfg(test)]
pub(crate) fn test_root_marker(
    connection: &rusqlite::Connection,
    root: &Path,
) -> crate::sync_folders::SyncRootMarker {
    connection.query_row(
        "SELECT b.library_uuid,b.root_uuid,r.root_folder_uuid,b.local_binding_uuid,b.binding_generation FROM roots r JOIN sync_root_local_bindings b ON b.root_id=r.id WHERE r.path=?1",
        [root.to_string_lossy()], |row| {
            let id = |index| -> rusqlite::Result<String> {
                Ok(uuid::Uuid::from_slice(&row.get::<_,Vec<u8>>(index)?).unwrap().to_string())
            };
            Ok(crate::sync_folders::SyncRootMarker { format_version:1, library_uuid:id(0)?, root_uuid:id(1)?, root_folder_uuid:id(2)?, local_binding_uuid:id(3)?, binding_generation:row.get(4)? })
        }).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_controls_require_matching_marker_journal_and_staging_evidence() {
        let root =
            std::env::temp_dir().join(format!("mangoo-owned-policy-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap();
        let db = crate::db::Database::open_in_memory().unwrap();
        db.apply_migrations().unwrap();
        db.ensure_selected_sync_root(
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            &root.to_string_lossy(),
            "Files",
            "Files",
            [7; 32],
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            1,
            "active",
            1,
            false,
        )
        .unwrap();
        let connection = db.connection_for_fsops();
        let marker_path = root.join(".mangoo-sync-root");
        let marker = test_root_marker(connection, &root);
        std::fs::write(&marker_path, b"ordinary user file").unwrap();
        assert!(!is_owned_path(connection, &root, &marker_path).unwrap());
        std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
        assert!(is_owned_path(connection, &root, &marker_path).unwrap());
        let mut old_marker = marker.clone();
        old_marker.local_binding_uuid = uuid::Uuid::now_v7().to_string();
        std::fs::write(&marker_path, serde_json::to_vec(&old_marker).unwrap()).unwrap();
        assert!(!is_owned_path(connection, &root, &marker_path).unwrap());
        let temporary = root.join(format!(".mangoo-sync-root.{}.tmp", uuid::Uuid::now_v7()));
        std::fs::write(&temporary, serde_json::to_vec(&old_marker).unwrap()).unwrap();
        assert!(is_owned_path(connection, &root, &temporary).unwrap());
        old_marker.library_uuid = uuid::Uuid::now_v7().to_string();
        std::fs::write(&temporary, serde_json::to_vec(&old_marker).unwrap()).unwrap();
        assert!(!is_owned_path(connection, &root, &temporary).unwrap());
        std::fs::write(&marker_path, vec![b'x'; 16 * 1024 + 1]).unwrap();
        assert!(!is_owned_path(connection, &root, &marker_path).unwrap());

        let root_id = db.list_roots().unwrap()[0].id;
        let recycle = root.join(format!(".mangoo-recycle-{}", uuid::Uuid::now_v7()));
        std::fs::write(&recycle, b"user file").unwrap();
        assert!(!is_owned_path(connection, &root, &recycle).unwrap());
        let identity = crate::library_path::local_file_identity(&recycle).unwrap();
        connection.execute("INSERT INTO file_operations(operation,source_path,target_path,status,created_at,root_id,staging_identity) VALUES ('delete_recycle','source',?1,'running',1,?2,?3)",rusqlite::params![recycle.to_string_lossy(),root_id,identity]).unwrap();
        assert!(is_owned_path(connection, &root, &recycle).unwrap());
        std::fs::rename(&recycle, root.join("original-recycle")).unwrap();
        std::fs::write(&recycle, b"replacement user file").unwrap();
        assert!(!is_owned_path(connection, &root, &recycle).unwrap());

        let folder = db
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root".into(),
                mtime: None,
            })
            .unwrap();
        let file = db
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id: folder,
                name: "data.bin".into(),
                ext: "bin".into(),
                size: 0,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .unwrap();
        let asset: Vec<u8> = connection
            .query_row("SELECT asset_uuid FROM files WHERE id=?1", [file], |r| {
                r.get(0)
            })
            .unwrap();
        let revision = uuid::Uuid::now_v7();
        connection.execute("INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,byte_size,local_mutation_generation,created_at) VALUES (?1,?2,0,1,1)",rusqlite::params![revision.as_bytes().as_slice(),asset]).unwrap();
        let relative = format!(
            ".mangoo-work-{}-{}/payload.partial",
            revision.simple(),
            uuid::Uuid::now_v7().simple()
        );
        connection.execute("INSERT INTO media_materialization_jobs(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,staging_relpath,created_at,updated_at) VALUES (?1,?2,'data.bin','assembling',0,?3,1,1)",rusqlite::params![revision.as_bytes().as_slice(),asset,relative]).unwrap();
        let partial = root.join(&relative);
        let directory = partial.parent().unwrap();
        assert!(!is_owned_path(connection, &root, &partial).unwrap());
        std::fs::create_dir(directory).unwrap();
        assert!(is_owned_path(connection, &root, directory)
            .unwrap_err()
            .to_string()
            .contains("pending"));
        std::fs::write(&partial, b"partial").unwrap();
        let identity = crate::library_path::local_file_identity(directory).unwrap();
        let file_identity = crate::library_path::local_file_identity(&partial).unwrap();
        connection
            .execute(
                "UPDATE media_materialization_jobs SET staging_directory_identity=?1,staging_file_identity=?2",
                rusqlite::params![identity,file_identity],
            )
            .unwrap();
        assert!(is_owned_path(connection, &root, &partial).unwrap());
        assert!(is_owned_path(connection, &root, directory).unwrap());
        let addition = directory.join("user.bin");
        std::fs::write(&addition, b"user addition").unwrap();
        assert!(!is_owned_path(connection, &root, directory).unwrap());
        assert!(!is_owned_path(connection, &root, &addition).unwrap());
        std::fs::rename(directory, root.join("previous-work")).unwrap();
        std::fs::create_dir(directory).unwrap();
        std::fs::write(&partial, b"replacement").unwrap();
        assert!(is_owned_path(connection, &root, &partial)
            .unwrap_err()
            .to_string()
            .contains("identity changed"));
        assert_eq!(std::fs::read(partial).unwrap(), b"replacement");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn owned_paths_require_persisted_identity_evidence() {
        let root = Path::new("root");
        let db = crate::db::Database::open_in_memory().unwrap();
        db.apply_migrations().unwrap();
        for name in [
            ".mangoo-sync-root",
            ".mangoo-library",
            ".mangoo-staging/data",
            ".mangoo-sync-root.00000000-0000-0000-0000-000000000001.tmp",
            "sub/.mangoo-recycle-00000000-0000-0000-0000-000000000001/data",
            "Thumbs.db",
            "user.tmp",
            ".mangoo-user",
            ".mangoo-sync-root.user.tmp",
            "empty",
        ] {
            assert!(
                !is_owned_path(db.connection_for_fsops(), root, &root.join(name)).unwrap(),
                "{name}"
            );
        }
    }
}

#[cfg(all(test, windows))]
mod pinned_path_tests {
    use super::*;
    #[test]
    fn all_file_source_and_destination_parent_pins_block_path_replacement() {
        let root =
            std::env::temp_dir().join(format!("mangoo-pinned-file-{}", uuid::Uuid::now_v7()));
        let parent = root.join("source");
        std::fs::create_dir_all(&parent).unwrap();
        let source = parent.join("source.bin");
        std::fs::write(&source, b"content").unwrap();
        let (file, guard) = open_source(&source).unwrap();
        assert!(std::fs::rename(&parent, root.join("moved")).is_err());
        assert!(std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .is_err());
        drop(file);
        drop(guard);
        let destination = root.join("target/nested/data.bin");
        std::fs::create_dir_all(root.join("target")).unwrap();
        let guards = pin_destination_parents(&root.join("target"), &destination).unwrap();
        assert!(std::fs::rename(root.join("target/nested"), root.join("moved")).is_err());
        drop(guards);
        std::fs::rename(root.join("target/nested"), root.join("moved")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

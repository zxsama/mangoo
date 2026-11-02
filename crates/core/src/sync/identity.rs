use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

pub fn stable_content_fingerprint(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    stable_content_fingerprint_with_cancel(path, || Ok(()))
}

/// High-resolution local cache stamp. Change time also detects same-size writes
/// whose caller restores the last-write timestamp. Never sent to another device.
pub fn source_stamp(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let (file, _guard) = super::file_policy::open_source(path).map_err(std::io::Error::other)?;
    let metadata = file.metadata()?;
    let mut stamp = metadata.len().to_le_bytes().to_vec();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileBasicInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO,
        };
        let mut info = FILE_BASIC_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&mut info as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        stamp.extend_from_slice(&info.LastWriteTime.to_le_bytes());
        stamp.extend_from_slice(&info.ChangeTime.to_le_bytes());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for value in [
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ] {
            stamp.extend_from_slice(&value.to_le_bytes());
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        stamp.extend_from_slice(
            &metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_le_bytes(),
        );
    }
    Ok(stamp)
}

pub fn observed_fingerprint(
    database: &crate::db::Database,
    path: &std::path::Path,
    volume: &[u8],
    identity: &[u8],
    mut check_cancelled: impl FnMut() -> std::io::Result<()>,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    check_cancelled()?;
    let before = source_stamp(path)?;
    if let Some(fingerprint) =
        database.cached_external_fingerprint(path, volume, identity, &before)?
    {
        return Ok((fingerprint, before));
    }
    let fingerprint = stable_content_fingerprint_with_cancel(path, &mut check_cancelled)?;
    let after = source_stamp(path)?;
    anyhow::ensure!(
        before == after,
        "file changed while observing content fingerprint"
    );
    Ok((fingerprint, after))
}

pub fn stable_content_fingerprint_with_cancel(
    path: &std::path::Path,
    mut check_cancelled: impl FnMut() -> std::io::Result<()>,
) -> std::io::Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    super::file_policy::require_regular_source(path).map_err(std::io::Error::other)?;
    let (mut file, _directory_guard) =
        super::file_policy::open_source(path).map_err(std::io::Error::other)?;
    let before = file.metadata()?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancelled()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || current.len() != before.len()
        || current.modified()? != before.modified()?
        || super::file_policy::is_link(&current)
    {
        return Err(std::io::Error::other(
            "file changed while computing content fingerprint",
        ));
    }
    Ok(hash.finalize().to_vec())
}

use super::capture::{match_external_identity, ExternalIdentityMatch, LocalFileIdentity};

pub fn record_identity(
    transaction: &Transaction<'_>,
    identity: &LocalFileIdentity,
    path: &str,
) -> anyhow::Result<()> {
    transaction.execute(
        "DELETE FROM sync_local_file_identities WHERE asset_uuid=?1 AND (volume_identity!=?2 OR file_identity!=?3)",
        params![identity.asset_uuid.as_bytes().as_slice(),identity.volume_identity,identity.file_identity],
    )?;
    transaction.execute("INSERT INTO sync_local_file_identities(volume_identity,file_identity,asset_uuid,last_path,content_fingerprint,last_seen_at) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(asset_uuid) DO UPDATE SET volume_identity=excluded.volume_identity,file_identity=excluded.file_identity,last_path=excluded.last_path,content_fingerprint=COALESCE(excluded.content_fingerprint,sync_local_file_identities.content_fingerprint),last_seen_at=excluded.last_seen_at,source_stamp=NULL", params![identity.volume_identity,identity.file_identity,identity.asset_uuid.as_bytes().as_slice(),path,identity.content_fingerprint,now_ms()])?;
    Ok(())
}

pub fn resolve_external_rename(
    transaction: &Transaction<'_>,
    volume: &[u8],
    file: &[u8],
    fingerprint: Option<&[u8]>,
    new_asset: Uuid,
    path: &str,
) -> anyhow::Result<ExternalIdentityMatch> {
    let exact: Option<Vec<u8>> = transaction.query_row("SELECT asset_uuid FROM sync_local_file_identities WHERE volume_identity=?1 AND file_identity=?2 AND asset_uuid<>?3", params![volume,file,new_asset.as_bytes().as_slice()], |r| r.get(0)).optional()?;
    if let Some(asset) = exact {
        return Ok(ExternalIdentityMatch::Exact(Uuid::from_slice(&asset)?));
    }
    let mut statement = transaction.prepare("SELECT i.asset_uuid,i.volume_identity,i.file_identity,i.content_fingerprint FROM sync_local_file_identities i LEFT JOIN files f ON f.asset_uuid=i.asset_uuid WHERE i.content_fingerprint=?1 AND i.asset_uuid<>?2 AND (f.id IS NULL OR f.status='missing') ORDER BY i.asset_uuid")?;
    let candidates = if let Some(fingerprint) = fingerprint {
        statement
            .query_map(params![fingerprint, new_asset.as_bytes().as_slice()], |r| {
                Ok(LocalFileIdentity {
                    asset_uuid: Uuid::from_slice(&r.get::<_, Vec<u8>>(0)?)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
                    volume_identity: r.get(1)?,
                    file_identity: r.get(2)?,
                    content_fingerprint: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let matched = match_external_identity(volume, file, fingerprint, &candidates);
    match &matched {
        ExternalIdentityMatch::Exact(asset) | ExternalIdentityMatch::UniqueFingerprint(asset) => {
            record_identity(
                transaction,
                &LocalFileIdentity {
                    asset_uuid: *asset,
                    volume_identity: volume.to_vec(),
                    file_identity: file.to_vec(),
                    content_fingerprint: fingerprint.map(ToOwned::to_owned),
                },
                path,
            )?
        }
        ExternalIdentityMatch::Ambiguous(old) => {
            for old_asset in old {
                transaction.execute("INSERT INTO sync_identity_conflicts(conflict_uuid,old_asset_uuid,new_asset_uuid,reason,state,created_at) VALUES (?1,?2,?3,'ambiguous-fingerprint','open',?4)", params![Uuid::now_v7().as_bytes().as_slice(),old_asset.as_bytes().as_slice(),new_asset.as_bytes().as_slice(),now_ms()])?;
            }
        }
        ExternalIdentityMatch::NewAsset => record_identity(
            transaction,
            &LocalFileIdentity {
                asset_uuid: new_asset,
                volume_identity: volume.to_vec(),
                file_identity: file.to_vec(),
                content_fingerprint: fingerprint.map(ToOwned::to_owned),
            },
            path,
        )?,
    }
    Ok(matched)
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn ambiguous_fingerprint_is_persisted_without_guessing_identity() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let connection = database.connection_for_fsops();
        let transaction = connection.unchecked_transaction().unwrap();
        for asset in [Uuid::now_v7(), Uuid::now_v7()] {
            record_identity(
                &transaction,
                &LocalFileIdentity {
                    asset_uuid: asset,
                    volume_identity: Uuid::now_v7().as_bytes().to_vec(),
                    file_identity: Uuid::now_v7().as_bytes().to_vec(),
                    content_fingerprint: Some(b"same".to_vec()),
                },
                "old",
            )
            .unwrap();
        }
        let result = resolve_external_rename(
            &transaction,
            b"new-volume",
            b"new-file",
            Some(b"same"),
            Uuid::now_v7(),
            "new",
        )
        .unwrap();
        assert!(matches!(result, ExternalIdentityMatch::Ambiguous(_)));
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM sync_identity_conflicts WHERE state='open'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }
}

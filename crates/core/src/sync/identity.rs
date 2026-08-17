use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

pub fn stable_content_fingerprint(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
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
    transaction.execute("INSERT INTO sync_local_file_identities(volume_identity,file_identity,asset_uuid,last_path,content_fingerprint,last_seen_at) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(volume_identity,file_identity) DO UPDATE SET asset_uuid=excluded.asset_uuid,last_path=excluded.last_path,content_fingerprint=excluded.content_fingerprint,last_seen_at=excluded.last_seen_at", params![identity.volume_identity,identity.file_identity,identity.asset_uuid.as_bytes().as_slice(),path,identity.content_fingerprint,now_ms()])?;
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

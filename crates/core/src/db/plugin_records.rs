//! Generic plugin-owned records in the active Library Catalog.
//!
//! Any Host-engine plugin that is granted `library-data` may keep small JSON
//! records here, grouped by `collection`. Records belong to the Library, so
//! they synchronize with it through the ordinary Catalog stream — no plugin
//! owns a transport, a schema or a second database:
//!
//! - one record is one synchronized entity (its UUID is the entity UUID);
//! - the immutable header field [`HEADER_FIELD`] names the owner plugin and
//!   collection;
//! - every top-level field is its own last-writer-wins field
//!   [`FIELD_PREFIX`]`<name>`, so concurrent edits of different fields on two
//!   devices both survive;
//! - delete / restore are the canonical tombstone operations, so an undo on one
//!   device and a delete on another converge like any other entity.
//!
//! `plugin_records` is only a projection of that truth. Remote apply and
//! checkpoint replay call [`rebuild_plugin_record`] from
//! `sync::inbox::materialize_truth`; local writes update the row directly and,
//! while a sync binding has capture enabled, emit the same events in the same
//! transaction. Rows written while capture is off are backfilled by
//! [`capture_uncaptured_plugin_records`] once capture turns on.
//!
//! Older builds receive these events as unknown fields: they keep them in
//! `sync_field_versions` and project nothing, which is forward compatible.

use std::sync::OnceLock;

use rusqlite::params;
use serde::Deserialize;
use serde_json::{Map, Value as JsonValue};

use super::*;
use crate::sync::capture::{capture_catalog_mutation, MutationOrigin};
use crate::sync::types::{EntityUuid, EventUuid, Operation};

/// Immutable header: `{ "pluginId", "collection", "createdAt" }`.
pub const HEADER_FIELD: &str = "plugin-record.v1";
/// Per-field LWW prefix: `plugin-field.v1:<name>`.
pub const FIELD_PREFIX: &str = "plugin-field.v1:";
pub const MAX_FIELDS_PER_RECORD: usize = 64;
/// One field value travels in one sync event (`MAX_EVENT_BYTES` = 256 KiB,
/// envelope included), so a serialized value is capped well below it.
pub const MAX_FIELD_VALUE_BYTES: usize = 160 * 1024;
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_BATCH_OPERATIONS: usize = 200;
pub const MAX_LIST_LIMIT: i64 = 1000;
pub const DEFAULT_LIST_LIMIT: i64 = 500;

/// Process-wide wake for `/api/plugin-data/{pluginId}/changes` long-polls.
/// Local writes notify after commit; projection rebuilds (remote apply) notify
/// from inside their transaction, and the waiter re-reads the committed
/// revision, so an early wake is harmless.
pub fn plugin_record_wake() -> &'static tokio::sync::Notify {
    static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
    WAKE.get_or_init(tokio::sync::Notify::new)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub id: String,
    pub collection: String,
    pub fields: Map<String, JsonValue>,
    pub deleted: bool,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum PluginRecordOperation {
    /// Create, or patch the listed top-level fields of, one record. A `null`
    /// field value removes that field, like a name listed in `unset`.
    Put {
        id: String,
        collection: String,
        #[serde(default)]
        fields: Map<String, JsonValue>,
        #[serde(default)]
        unset: Vec<String>,
    },
    Delete {
        id: String,
    },
    Restore {
        id: String,
    },
}

#[derive(Debug)]
pub enum PluginRecordError {
    Invalid(String),
    NotFound(String),
    Conflict(String),
    Other(anyhow::Error),
}

impl std::fmt::Display for PluginRecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::NotFound(message) | Self::Conflict(message) => {
                f.write_str(message)
            }
            Self::Other(error) => write!(f, "{error:#}"),
        }
    }
}

impl From<anyhow::Error> for PluginRecordError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}

impl From<rusqlite::Error> for PluginRecordError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Other(value.into())
    }
}

impl From<serde_json::Error> for PluginRecordError {
    fn from(value: serde_json::Error) -> Self {
        Self::Other(value.into())
    }
}

type RecordResult<T> = Result<T, PluginRecordError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecordPage {
    pub items: Vec<PluginRecord>,
    pub next_cursor: Option<String>,
    pub revision: i64,
}

// ---------------------------------------------------------------- validation

pub fn valid_collection(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

pub fn valid_field_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphabetic()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

pub fn parse_record_id(value: &str) -> RecordResult<uuid::Uuid> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| PluginRecordError::Invalid(format!("invalid record id: {value}")))?;
    if parsed.is_nil() {
        return Err(PluginRecordError::Invalid("record id cannot be nil".into()));
    }
    Ok(parsed)
}

fn validate_fields(fields: &Map<String, JsonValue>) -> RecordResult<()> {
    if fields.len() > MAX_FIELDS_PER_RECORD {
        return Err(PluginRecordError::Invalid(format!(
            "a record holds at most {MAX_FIELDS_PER_RECORD} fields"
        )));
    }
    let mut total = 0usize;
    for (name, value) in fields {
        if !valid_field_name(name) {
            return Err(PluginRecordError::Invalid(format!(
                "invalid field name: {name}"
            )));
        }
        let size = serde_json::to_vec(value)?.len();
        if size > MAX_FIELD_VALUE_BYTES {
            return Err(PluginRecordError::Invalid(format!(
                "field {name} exceeds {MAX_FIELD_VALUE_BYTES} bytes"
            )));
        }
        total += size + name.len();
    }
    if total > MAX_RECORD_BYTES {
        return Err(PluginRecordError::Invalid(format!(
            "record exceeds {MAX_RECORD_BYTES} bytes"
        )));
    }
    Ok(())
}

// ------------------------------------------------------------------ helpers

fn now_ms() -> i64 {
    unix_timestamp_millis()
}

fn bump_revision(connection: &Connection, plugin_id: &str) -> rusqlite::Result<i64> {
    connection.query_row(
        "INSERT INTO plugin_record_revisions(plugin_id,revision) VALUES (?1,1) ON CONFLICT(plugin_id) DO UPDATE SET revision=revision+1 RETURNING revision",
        [plugin_id],
        |row| row.get(0),
    )
}

pub fn plugin_revision(connection: &Connection, plugin_id: &str) -> rusqlite::Result<i64> {
    Ok(connection
        .query_row(
            "SELECT revision FROM plugin_record_revisions WHERE plugin_id=?1",
            [plugin_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

struct StoredRecord {
    plugin_id: String,
    collection: String,
    fields: Map<String, JsonValue>,
    deleted: bool,
    sync_captured: bool,
    created_at: i64,
}

fn load_stored(connection: &Connection, id: &uuid::Uuid) -> RecordResult<Option<StoredRecord>> {
    let row = connection
        .query_row(
            "SELECT plugin_id,collection,fields_json,deleted,sync_captured,created_at FROM plugin_records WHERE record_uuid=?1",
            [id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((plugin_id, collection, fields, deleted, sync_captured, created_at)) = row else {
        return Ok(None);
    };
    let fields = match serde_json::from_str::<JsonValue>(&fields)? {
        JsonValue::Object(map) => map,
        _ => Map::new(),
    };
    Ok(Some(StoredRecord {
        plugin_id,
        collection,
        fields,
        deleted,
        sync_captured,
        created_at,
    }))
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginRecord> {
    let id: Vec<u8> = row.get(0)?;
    let fields: String = row.get(2)?;
    let fields = match serde_json::from_str::<JsonValue>(&fields) {
        Ok(JsonValue::Object(map)) => map,
        _ => Map::new(),
    };
    let id = uuid::Uuid::from_slice(&id)
        .map(|value| value.to_string())
        .unwrap_or_default();
    Ok(PluginRecord {
        id,
        collection: row.get(1)?,
        fields,
        deleted: row.get(3)?,
        revision: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

const RECORD_COLUMNS: &str =
    "record_uuid,collection,fields_json,deleted,revision,created_at,updated_at";

fn read_record(connection: &Connection, id: &uuid::Uuid) -> RecordResult<Option<PluginRecord>> {
    Ok(connection
        .query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM plugin_records WHERE record_uuid=?1"),
            [id.as_bytes().as_slice()],
            record_from_row,
        )
        .optional()?)
}

#[allow(clippy::too_many_arguments)]
fn write_row(
    connection: &Connection,
    id: &uuid::Uuid,
    plugin_id: &str,
    collection: &str,
    fields: &Map<String, JsonValue>,
    deleted: bool,
    sync_captured: bool,
    created_at: i64,
) -> RecordResult<()> {
    let revision = bump_revision(connection, plugin_id)?;
    connection.execute(
        "INSERT INTO plugin_records(record_uuid,plugin_id,collection,fields_json,deleted,revision,sync_captured,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(record_uuid) DO UPDATE SET fields_json=excluded.fields_json,deleted=excluded.deleted,revision=excluded.revision,sync_captured=MAX(plugin_records.sync_captured,excluded.sync_captured),updated_at=excluded.updated_at",
        params![
            id.as_bytes().as_slice(),
            plugin_id,
            collection,
            serde_json::to_string(fields)?,
            deleted,
            revision,
            sync_captured,
            created_at,
            now_ms()
        ],
    )?;
    Ok(())
}

/// Whether local mutations currently become canonical events.
fn capture_active(connection: &Connection) -> rusqlite::Result<bool> {
    Ok(connection
        .query_row(
            "SELECT capture_enabled=1 FROM sync_local_state WHERE singleton=1",
            [],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

fn capture(
    transaction: &Transaction<'_>,
    id: &uuid::Uuid,
    operation: Operation,
) -> RecordResult<()> {
    capture_catalog_mutation(
        transaction,
        MutationOrigin::LocalApi,
        EntityUuid(*id),
        operation,
    )?;
    Ok(())
}

fn capture_field(
    transaction: &Transaction<'_>,
    id: &uuid::Uuid,
    name: &str,
    value: JsonValue,
) -> RecordResult<()> {
    capture(
        transaction,
        id,
        Operation::SetField {
            field: format!("{FIELD_PREFIX}{name}"),
            value,
        },
    )
}

/// Emit a record's complete current state (header + every field, plus a
/// tombstone when deleted) as canonical events.
fn capture_snapshot(
    transaction: &Transaction<'_>,
    id: &uuid::Uuid,
    plugin_id: &str,
    collection: &str,
    fields: &Map<String, JsonValue>,
    deleted: bool,
    created_at: i64,
) -> RecordResult<()> {
    capture(
        transaction,
        id,
        Operation::SetField {
            field: HEADER_FIELD.into(),
            value: serde_json::json!({
                "pluginId": plugin_id,
                "collection": collection,
                "createdAt": created_at,
            }),
        },
    )?;
    for (name, value) in fields {
        capture_field(transaction, id, name, value.clone())?;
    }
    if deleted {
        capture(
            transaction,
            id,
            Operation::Delete {
                token: EventUuid::new(),
            },
        )?;
    }
    Ok(())
}

fn live_tombstones(
    connection: &Connection,
    entity: &[u8],
) -> rusqlite::Result<std::collections::BTreeSet<EventUuid>> {
    let mut statement = connection.prepare(
        "SELECT t.token_uuid FROM sync_tombstone_tokens t WHERE t.entity_uuid=?1 AND NOT EXISTS (SELECT 1 FROM sync_tombstone_removals r WHERE r.observed_token_uuid=t.token_uuid)",
    )?;
    let rows = statement
        .query_map([entity], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|bytes| uuid::Uuid::from_slice(&bytes).ok().map(EventUuid))
        .collect())
}

// ------------------------------------------------------------- local writes

fn apply_operation(
    transaction: &Transaction<'_>,
    plugin_id: &str,
    operation: PluginRecordOperation,
    allow_mirror: bool,
) -> RecordResult<uuid::Uuid> {
    let capturing = capture_active(transaction)?;
    match operation {
        PluginRecordOperation::Put {
            id,
            collection,
            fields,
            unset,
        } => {
            let id = parse_record_id(&id)?;
            if !valid_collection(&collection)
                || (!allow_mirror && crate::plugin_shared::is_mirror_collection(&collection))
            {
                return Err(PluginRecordError::Invalid(format!(
                    "invalid collection: {collection}"
                )));
            }
            for name in fields.keys().chain(unset.iter()) {
                if !valid_field_name(name) {
                    return Err(PluginRecordError::Invalid(format!(
                        "invalid field name: {name}"
                    )));
                }
            }
            let existing = load_stored(transaction, &id)?;
            if let Some(stored) = &existing {
                if stored.plugin_id != plugin_id {
                    return Err(PluginRecordError::Conflict(
                        "record belongs to another plugin".into(),
                    ));
                }
                if stored.collection != collection {
                    return Err(PluginRecordError::Conflict(
                        "record belongs to another collection".into(),
                    ));
                }
                if stored.deleted {
                    return Err(PluginRecordError::Conflict("record-deleted".into()));
                }
            }
            let mut next = existing
                .as_ref()
                .map(|stored| stored.fields.clone())
                .unwrap_or_default();
            let mut changed: Vec<String> = Vec::new();
            for (name, value) in fields {
                let previous = next.get(&name).cloned();
                if value.is_null() {
                    if next.remove(&name).is_some() {
                        changed.push(name);
                    }
                } else if previous.as_ref() != Some(&value) {
                    next.insert(name.clone(), value);
                    changed.push(name);
                }
            }
            for name in unset {
                if next.remove(&name).is_some() && !changed.contains(&name) {
                    changed.push(name);
                }
            }
            if existing.is_some() && changed.is_empty() {
                return Ok(id);
            }
            validate_fields(&next)?;
            let created_at = existing
                .as_ref()
                .map(|stored| stored.created_at)
                .unwrap_or_else(now_ms);
            let already_captured = existing.as_ref().is_some_and(|stored| stored.sync_captured);
            if capturing {
                if already_captured {
                    for name in &changed {
                        let value = next.get(name).cloned().unwrap_or(JsonValue::Null);
                        capture_field(transaction, &id, name, value)?;
                    }
                } else {
                    capture_snapshot(
                        transaction,
                        &id,
                        plugin_id,
                        &collection,
                        &next,
                        false,
                        created_at,
                    )?;
                }
            }
            write_row(
                transaction,
                &id,
                plugin_id,
                &collection,
                &next,
                false,
                capturing || already_captured,
                created_at,
            )?;
            Ok(id)
        }
        PluginRecordOperation::Delete { id } => {
            let id = parse_record_id(&id)?;
            let stored = owned_record(transaction, plugin_id, &id)?;
            if stored.deleted {
                return Ok(id);
            }
            if capturing {
                if !stored.sync_captured {
                    capture_snapshot(
                        transaction,
                        &id,
                        plugin_id,
                        &stored.collection,
                        &stored.fields,
                        true,
                        stored.created_at,
                    )?;
                } else {
                    capture(
                        transaction,
                        &id,
                        Operation::Delete {
                            token: EventUuid::new(),
                        },
                    )?;
                }
            }
            write_row(
                transaction,
                &id,
                plugin_id,
                &stored.collection,
                &stored.fields,
                true,
                capturing || stored.sync_captured,
                stored.created_at,
            )?;
            Ok(id)
        }
        PluginRecordOperation::Restore { id } => {
            let id = parse_record_id(&id)?;
            // Restoring what does not exist is a no-op (like restoring what is
            // not deleted), so a client can "restore then put" idempotently.
            if load_stored(transaction, &id)?.is_none() {
                return Ok(id);
            }
            let stored = owned_record(transaction, plugin_id, &id)?;
            if !stored.deleted {
                return Ok(id);
            }
            if capturing {
                if !stored.sync_captured {
                    capture_snapshot(
                        transaction,
                        &id,
                        plugin_id,
                        &stored.collection,
                        &stored.fields,
                        false,
                        stored.created_at,
                    )?;
                } else {
                    let observed_tokens = live_tombstones(transaction, id.as_bytes())?;
                    if !observed_tokens.is_empty() {
                        capture(transaction, &id, Operation::Restore { observed_tokens })?;
                    }
                }
            }
            write_row(
                transaction,
                &id,
                plugin_id,
                &stored.collection,
                &stored.fields,
                false,
                capturing || stored.sync_captured,
                stored.created_at,
            )?;
            Ok(id)
        }
    }
}

fn owned_record(
    connection: &Connection,
    plugin_id: &str,
    id: &uuid::Uuid,
) -> RecordResult<StoredRecord> {
    match load_stored(connection, id)? {
        Some(stored) if stored.plugin_id == plugin_id => Ok(stored),
        _ => Err(PluginRecordError::NotFound(format!(
            "record not found: {id}"
        ))),
    }
}

// ------------------------------------------------------ projection rebuild

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordHeader {
    plugin_id: String,
    collection: String,
    #[serde(default)]
    created_at: Option<i64>,
}

/// Rebuild one `plugin_records` row from canonical truth. Called by
/// `materialize_truth` for every header / field / tombstone event of an
/// entity; a no-op for entities that are not plugin records. Returns whether
/// a row was written.
pub(crate) fn rebuild_plugin_record(
    transaction: &Transaction<'_>,
    entity: &[u8],
) -> anyhow::Result<bool> {
    let header: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT value FROM sync_field_versions WHERE entity_uuid=?1 AND field_name=?2 AND is_winner=1",
            params![entity, HEADER_FIELD],
            |row| row.get(0),
        )
        .optional()?;
    let Some(header) = header else {
        return Ok(false);
    };
    let Ok(header) = serde_json::from_slice::<RecordHeader>(&header) else {
        return Ok(false);
    };
    if header.plugin_id.is_empty()
        || header.plugin_id.len() > crate::plugins::manifest::MAX_PLUGIN_ID_LEN
        || !valid_collection(&header.collection)
    {
        return Ok(false);
    }
    let Ok(id) = uuid::Uuid::from_slice(entity) else {
        return Ok(false);
    };
    if let Some(stored) =
        load_stored(transaction, &id).map_err(|error| anyhow::anyhow!("{error}"))?
    {
        if stored.plugin_id != header.plugin_id {
            // A UUID can only ever name one owner; never let a remote event
            // move a record between plugins.
            return Ok(false);
        }
    }
    let mut statement = transaction.prepare(
        "SELECT field_name,value FROM sync_field_versions WHERE entity_uuid=?1 AND is_winner=1 AND substr(field_name,1,?2)=?3",
    )?;
    let rows = statement
        .query_map(
            params![entity, FIELD_PREFIX.len() as i64, FIELD_PREFIX],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let mut fields = Map::new();
    for (field_name, value) in rows {
        let name = &field_name[FIELD_PREFIX.len()..];
        if !valid_field_name(name) {
            continue;
        }
        let Some(value) = value else { continue };
        let Ok(value) = serde_json::from_slice::<JsonValue>(&value) else {
            continue;
        };
        if !value.is_null() {
            fields.insert(name.to_owned(), value);
        }
    }
    let deleted = !live_tombstones(transaction, entity)?.is_empty();
    write_row(
        transaction,
        &id,
        &header.plugin_id,
        &header.collection,
        &fields,
        deleted,
        true,
        header.created_at.unwrap_or_else(now_ms),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    plugin_record_wake().notify_waiters();
    Ok(true)
}

/// Whether a field name belongs to the plugin-record projection.
pub(crate) fn is_plugin_record_field(field: &str) -> bool {
    field == HEADER_FIELD || field.starts_with(FIELD_PREFIX)
}

/// Backfill records written while capture was disabled, in bounded batches.
/// Runs inside the caller's transaction on every sync turn.
pub(crate) fn capture_uncaptured_plugin_records(
    transaction: &Transaction<'_>,
) -> anyhow::Result<usize> {
    if !capture_active(transaction)? {
        return Ok(0);
    }
    let mut statement = transaction.prepare(
        "SELECT record_uuid FROM plugin_records WHERE sync_captured=0 ORDER BY id LIMIT 100",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let mut captured = 0;
    for bytes in ids {
        let id = uuid::Uuid::from_slice(&bytes)?;
        let Some(stored) =
            load_stored(transaction, &id).map_err(|error| anyhow::anyhow!("{error}"))?
        else {
            continue;
        };
        capture_snapshot(
            transaction,
            &id,
            &stored.plugin_id,
            &stored.collection,
            &stored.fields,
            stored.deleted,
            stored.created_at,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        transaction.execute(
            "UPDATE plugin_records SET sync_captured=1 WHERE record_uuid=?1",
            [bytes.as_slice()],
        )?;
        captured += 1;
    }
    Ok(captured)
}

// ---------------------------------------------------------------- Database

impl Database {
    pub fn plugin_record_revision(&self, plugin_id: &str) -> anyhow::Result<i64> {
        Ok(plugin_revision(&self.connection, plugin_id)?)
    }

    /// Keyset page over one collection, ordered by record UUID (UUIDv7 ids
    /// are creation ordered). `cursor` is the last id of the previous page.
    pub fn list_plugin_records(
        &self,
        plugin_id: &str,
        collection: &str,
        cursor: Option<&str>,
        limit: i64,
        include_deleted: bool,
    ) -> RecordResult<PluginRecordPage> {
        if !valid_collection(collection) || crate::plugin_shared::is_mirror_collection(collection) {
            return Err(PluginRecordError::Invalid(format!(
                "invalid collection: {collection}"
            )));
        }
        let limit = limit.clamp(1, MAX_LIST_LIMIT);
        let after = cursor.map(parse_record_id).transpose()?;
        // One read transaction: the revision must describe exactly this page.
        let transaction = self.connection.unchecked_transaction()?;
        let revision = plugin_revision(&transaction, plugin_id)?;
        let mut statement = transaction.prepare(&format!(
            "SELECT {RECORD_COLUMNS} FROM plugin_records WHERE plugin_id=?1 AND collection=?2 AND (?3 OR deleted=0) AND (?4 IS NULL OR record_uuid>?4) ORDER BY record_uuid LIMIT ?5"
        ))?;
        let mut items = statement
            .query_map(
                params![
                    plugin_id,
                    collection,
                    include_deleted,
                    after.map(|id| id.as_bytes().to_vec()),
                    limit + 1
                ],
                record_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        transaction.commit()?;
        let next_cursor = if items.len() as i64 > limit {
            items.truncate(limit as usize);
            items.last().map(|item| item.id.clone())
        } else {
            None
        };
        Ok(PluginRecordPage {
            items,
            next_cursor,
            revision,
        })
    }

    pub fn get_plugin_record(&self, plugin_id: &str, id: &str) -> RecordResult<PluginRecord> {
        let id = parse_record_id(id)?;
        let owner: Option<String> = self
            .connection
            .query_row(
                "SELECT plugin_id FROM plugin_records WHERE record_uuid=?1",
                [id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        if owner.as_deref() != Some(plugin_id) {
            return Err(PluginRecordError::NotFound(format!(
                "record not found: {id}"
            )));
        }
        read_record(&self.connection, &id)?
            .ok_or_else(|| PluginRecordError::NotFound(format!("record not found: {id}")))
    }

    /// Apply every operation in one immediate transaction (all or nothing) and
    /// return the resulting records in operation order.
    pub fn apply_plugin_record_batch(
        &self,
        plugin_id: &str,
        operations: Vec<PluginRecordOperation>,
    ) -> RecordResult<(i64, Vec<PluginRecord>)> {
        self.apply_plugin_record_batch_scoped(plugin_id, operations, false)
    }

    /// Every shared-scope mirror record of a plugin in this Library:
    /// `(id, mirror collection, wrapped fields)`. Mirrors are never tombstoned.
    pub fn list_plugin_mirror_records(
        &self,
        plugin_id: &str,
    ) -> anyhow::Result<Vec<(uuid::Uuid, String, Map<String, JsonValue>)>> {
        let mut statement = self.connection.prepare(
            "SELECT record_uuid,collection,fields_json FROM plugin_records WHERE plugin_id=?1 AND substr(collection,1,?2)=?3 AND deleted=0 ORDER BY record_uuid",
        )?;
        let prefix = crate::plugin_shared::MIRROR_PREFIX;
        let rows = statement
            .query_map(params![plugin_id, prefix.len() as i64, prefix], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, collection, fields)| {
                let fields = match serde_json::from_str::<JsonValue>(&fields)? {
                    JsonValue::Object(map) => map,
                    _ => Map::new(),
                };
                Ok((uuid::Uuid::from_slice(&id)?, collection, fields))
            })
            .collect()
    }

    /// Write reconciled shared values into this Library's mirror collections
    /// (internal: the public batch refuses `mirror--` collections).
    pub fn apply_plugin_mirror_puts(
        &self,
        plugin_id: &str,
        puts: Vec<(uuid::Uuid, String, Map<String, JsonValue>)>,
    ) -> RecordResult<i64> {
        let mut revision = self.plugin_record_revision(plugin_id)?;
        for chunk in puts.chunks(MAX_BATCH_OPERATIONS) {
            let operations = chunk
                .iter()
                .map(|(id, collection, fields)| PluginRecordOperation::Put {
                    id: id.to_string(),
                    collection: collection.clone(),
                    fields: fields.clone(),
                    unset: Vec::new(),
                })
                .collect();
            revision = self
                .apply_plugin_record_batch_scoped(plugin_id, operations, true)?
                .0;
        }
        Ok(revision)
    }

    fn apply_plugin_record_batch_scoped(
        &self,
        plugin_id: &str,
        operations: Vec<PluginRecordOperation>,
        allow_mirror: bool,
    ) -> RecordResult<(i64, Vec<PluginRecord>)> {
        if operations.is_empty() || operations.len() > MAX_BATCH_OPERATIONS {
            return Err(PluginRecordError::Invalid(format!(
                "a batch holds 1 to {MAX_BATCH_OPERATIONS} operations"
            )));
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let mut ids = Vec::with_capacity(operations.len());
        for operation in operations {
            ids.push(apply_operation(
                &transaction,
                plugin_id,
                operation,
                allow_mirror,
            )?);
        }
        let mut records = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(record) = read_record(&transaction, id)? {
                records.push(record);
            }
        }
        let revision = plugin_revision(&transaction, plugin_id)?;
        transaction.commit()?;
        plugin_record_wake().notify_waiters();
        Ok((revision, records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        database
    }

    fn put(id: &str, fields: JsonValue) -> PluginRecordOperation {
        PluginRecordOperation::Put {
            id: id.into(),
            collection: "items".into(),
            fields: fields.as_object().cloned().unwrap_or_default(),
            unset: Vec::new(),
        }
    }

    const PLUGIN: &str = "com.mangoo.todo-notes";

    #[test]
    fn plugin_records_put_patch_delete_restore_and_list() {
        let db = database();
        let id = uuid::Uuid::now_v7().to_string();
        let (revision, records) = db
            .apply_plugin_record_batch(
                PLUGIN,
                vec![put(&id, serde_json::json!({"title": "a", "done": false}))],
            )
            .unwrap();
        assert!(revision >= 1);
        assert_eq!(records[0].fields["title"], "a");
        // Patch one field, remove another via null.
        let (_, records) = db
            .apply_plugin_record_batch(
                PLUGIN,
                vec![put(&id, serde_json::json!({"title": "b", "done": null}))],
            )
            .unwrap();
        assert_eq!(records[0].fields.get("done"), None);
        assert_eq!(records[0].fields["title"], "b");
        // No-op patch does not bump the revision.
        let before = db.plugin_record_revision(PLUGIN).unwrap();
        db.apply_plugin_record_batch(PLUGIN, vec![put(&id, serde_json::json!({"title": "b"}))])
            .unwrap();
        assert_eq!(db.plugin_record_revision(PLUGIN).unwrap(), before);

        db.apply_plugin_record_batch(
            PLUGIN,
            vec![PluginRecordOperation::Delete { id: id.clone() }],
        )
        .unwrap();
        let page = db
            .list_plugin_records(PLUGIN, "items", None, 10, false)
            .unwrap();
        assert!(page.items.is_empty());
        let page = db
            .list_plugin_records(PLUGIN, "items", None, 10, true)
            .unwrap();
        assert!(page.items[0].deleted);
        // A put on a deleted record is refused until it is restored.
        assert!(matches!(
            db.apply_plugin_record_batch(PLUGIN, vec![put(&id, serde_json::json!({"title": "c"}))]),
            Err(PluginRecordError::Conflict(_))
        ));
        db.apply_plugin_record_batch(
            PLUGIN,
            vec![PluginRecordOperation::Restore { id: id.clone() }],
        )
        .unwrap();
        assert_eq!(
            db.get_plugin_record(PLUGIN, &id).unwrap().fields["title"],
            "b"
        );
    }

    #[test]
    fn plugin_records_are_isolated_per_plugin_and_validated() {
        let db = database();
        let id = uuid::Uuid::now_v7().to_string();
        db.apply_plugin_record_batch(PLUGIN, vec![put(&id, serde_json::json!({"title": "a"}))])
            .unwrap();
        assert!(matches!(
            db.get_plugin_record("com.example.other", &id),
            Err(PluginRecordError::NotFound(_))
        ));
        assert!(matches!(
            db.apply_plugin_record_batch(
                "com.example.other",
                vec![put(&id, serde_json::json!({"x": 1}))]
            ),
            Err(PluginRecordError::Conflict(_))
        ));
        assert!(matches!(
            db.apply_plugin_record_batch(
                PLUGIN,
                vec![put(&id, serde_json::json!({"bad-name": 1}))]
            ),
            Err(PluginRecordError::Invalid(_))
        ));
        let huge = "x".repeat(MAX_FIELD_VALUE_BYTES + 1);
        assert!(matches!(
            db.apply_plugin_record_batch(
                PLUGIN,
                vec![put(
                    &uuid::Uuid::now_v7().to_string(),
                    serde_json::json!({"data": huge})
                )]
            ),
            Err(PluginRecordError::Invalid(_))
        ));
        // A failing batch commits nothing.
        let fresh = uuid::Uuid::now_v7().to_string();
        assert!(db
            .apply_plugin_record_batch(
                PLUGIN,
                vec![
                    put(&fresh, serde_json::json!({"title": "x"})),
                    put("not-a-uuid", serde_json::json!({}))
                ]
            )
            .is_err());
        assert!(db.get_plugin_record(PLUGIN, &fresh).is_err());
    }

    #[test]
    fn plugin_records_keyset_pagination() {
        let db = database();
        let operations = (0..5)
            .map(|index| {
                put(
                    &uuid::Uuid::now_v7().to_string(),
                    serde_json::json!({ "n": index }),
                )
            })
            .collect();
        db.apply_plugin_record_batch(PLUGIN, operations).unwrap();
        let first = db
            .list_plugin_records(PLUGIN, "items", None, 2, false)
            .unwrap();
        assert_eq!(first.items.len(), 2);
        let second = db
            .list_plugin_records(PLUGIN, "items", first.next_cursor.as_deref(), 10, false)
            .unwrap();
        assert_eq!(second.items.len(), 3);
        assert!(second.next_cursor.is_none());
        assert_eq!(second.items[0].fields["n"], 2);
    }

    /// Captured events replayed into a fresh Catalog (the remote-apply and
    /// checkpoint-restore path) rebuild the same projection, including field
    /// level merges, deletes and backfilled pre-capture records.
    #[test]
    fn plugin_records_sync_capture_replays_into_a_fresh_catalog() {
        let db = database();
        db.initialize_sync_identity(
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
        )
        .expect("sync identity");
        // Written while capture is disabled: no events yet.
        let early = uuid::Uuid::now_v7().to_string();
        db.apply_plugin_record_batch(
            PLUGIN,
            vec![put(&early, serde_json::json!({"title": "early"}))],
        )
        .unwrap();
        assert_eq!(db.sync_capture_counts_for_test().unwrap().0, 0);

        db.enable_fake_sync_capture_for_test().unwrap();
        let live = uuid::Uuid::now_v7().to_string();
        let gone = uuid::Uuid::now_v7().to_string();
        db.apply_plugin_record_batch(
            PLUGIN,
            vec![
                put(&live, serde_json::json!({"title": "one", "done": false})),
                put(&gone, serde_json::json!({"title": "bye"})),
            ],
        )
        .unwrap();
        db.apply_plugin_record_batch(PLUGIN, vec![put(&live, serde_json::json!({"done": true}))])
            .unwrap();
        db.apply_plugin_record_batch(
            PLUGIN,
            vec![PluginRecordOperation::Delete { id: gone.clone() }],
        )
        .unwrap();
        {
            let transaction = db.connection.unchecked_transaction().unwrap();
            assert_eq!(capture_uncaptured_plugin_records(&transaction).unwrap(), 1);
            transaction.commit().unwrap();
        }
        // The live projection is unchanged by its own rebuilds.
        assert_eq!(
            db.get_plugin_record(PLUGIN, &live).unwrap().fields["done"],
            true
        );

        let events = {
            let mut statement = db
                .connection
                .prepare("SELECT payload FROM sync_events ORDER BY actor_seq")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .unwrap()
                .map(|row| crate::sync::codec::decode_event(&row.unwrap()).unwrap())
                .collect::<Vec<_>>()
        };
        assert!(!events.is_empty());

        let replica = database();
        replica
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let transaction = replica.connection.unchecked_transaction().unwrap();
        for event in &events {
            crate::sync::inbox::apply_remote_batch(&transaction, std::slice::from_ref(event))
                .unwrap();
        }
        transaction.commit().unwrap();
        let live_copy = replica.get_plugin_record(PLUGIN, &live).unwrap();
        assert_eq!(live_copy.fields["title"], "one");
        assert_eq!(live_copy.fields["done"], true);
        assert_eq!(live_copy.collection, "items");
        assert_eq!(
            replica.get_plugin_record(PLUGIN, &early).unwrap().fields["title"],
            "early"
        );
        assert!(replica.get_plugin_record(PLUGIN, &gone).unwrap().deleted);
        assert!(replica.plugin_record_revision(PLUGIN).unwrap() > 0);
    }
}

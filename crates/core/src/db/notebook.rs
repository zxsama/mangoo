//! Notebook documents and collected files travel through ordinary Library
//! plugin records. A single LWW field publishes each document version. Small
//! sources fit inline; large sources and files reference immutable chunks.
//! A remote reader can therefore never assemble two different edits together.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use uuid::Uuid;

use super::plugin_records::{
    apply_operation, parse_record_id, plugin_record_wake, PluginRecordError as Error,
    PluginRecordOperation, MAX_FIELD_VALUE_BYTES,
};
use super::{unix_timestamp_millis, Database};

#[path = "notebook_tree.rs"]
pub mod tree;

pub use crate::plugins::NOTEBOOK_PLUGIN_ID as PLUGIN;
pub(super) const DOCUMENTS: &str = "documents";
const ATTACHMENTS: &str = "attachments";
const CHUNKS: &str = "chunks";
const CHUNK_BYTES: usize = 96 * 1024;
// One page fits in a plugin field and its corresponding sync event. Only a
// logarithmic stack of pages is retained while ingesting a file.
const INDEX_FANOUT: usize = 1024;
type Result<T> = std::result::Result<T, Error>;

#[derive(Default, Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteFormat {
    #[default]
    Markdown,
    Html,
    Document,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteInput {
    pub title: String,
    pub markdown: String,
    pub styles: Map<String, Value>,
    #[serde(default)]
    pub format: NoteFormat,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub id: Uuid,
    pub title: String,
    pub format: NoteFormat,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct NotebookNote {
    #[serde(flatten)]
    pub summary: NoteSummary,
    pub markdown: String,
    pub styles: Map<String, Value>,
    pub attachments: Vec<NoteAttachment>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteAttachment {
    pub id: Uuid,
    pub name: String,
    pub mime_type: String,
    pub size: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChunkList {
    size: usize,
    chunks: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "is_zero")]
    level: u8,
}

fn is_zero(level: &u8) -> bool {
    *level == 0
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Source {
    Text(String),
    Chunks(ChunkList),
}

impl Source {
    fn text(self, conn: &Connection) -> Result<String> {
        match self {
            Self::Text(text) => Ok(text),
            Self::Chunks(parts) => {
                let mut bytes = Vec::new();
                visit_chunks(conn, parts, &mut |chunk| {
                    bytes.extend_from_slice(chunk);
                    Ok(())
                })?;
                String::from_utf8(bytes)
                    .map_err(|_| Error::Invalid("Note source is not valid UTF-8".into()))
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentHead {
    title: String,
    format: NoteFormat,
    styles: Map<String, Value>,
    source: Source,
    updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentHead {
    note_id: Uuid,
    name: String,
    mime_type: String,
    contents: ChunkList,
}

fn pending() -> Error {
    Error::Conflict("note_content_pending".into())
}

fn missing() -> Error {
    Error::NotFound("Note or attachment was not found".into())
}

fn put(tx: &Transaction<'_>, id: Uuid, collection: &str, field: &str, value: Value) -> Result<()> {
    apply_operation(
        tx,
        PLUGIN,
        PluginRecordOperation::Put {
            id: id.to_string(),
            collection: collection.into(),
            fields: Map::from_iter([(field.into(), value)]),
            unset: Vec::new(),
        },
        false,
    )?;
    Ok(())
}

fn record_head<T: DeserializeOwned>(
    conn: &Connection,
    id: Uuid,
    collection: &str,
) -> Result<(T, i64, i64)> {
    let (json, revision, created_at): (Option<String>, i64, i64) = conn
        .query_row(
            "SELECT json_extract(fields_json,'$.head'),revision,created_at FROM plugin_records
         WHERE record_uuid=?1 AND plugin_id=?2 AND collection=?3 AND deleted=0",
            params![id.as_bytes(), PLUGIN, collection],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or_else(missing)?;
    Ok((
        serde_json::from_str(&json.ok_or_else(pending)?)?,
        revision,
        created_at,
    ))
}

fn content_id(domain: &[u8], bytes: &[u8]) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash.finalize()[..16]);
    uuid::Builder::from_custom_bytes(id).into_uuid()
}

fn chunk_id(bytes: &[u8]) -> Uuid {
    content_id(b"mangoo.notebook.chunk.v1\0", bytes)
}

fn index_id(children: &[Uuid]) -> Uuid {
    let bytes: Vec<_> = children.iter().flat_map(|id| *id.as_bytes()).collect();
    content_id(b"mangoo.notebook.chunk-index.v1\0", &bytes)
}

fn store_page(tx: &Transaction<'_>, page: &[Uuid]) -> Result<Uuid> {
    let id = index_id(page);
    put(
        tx,
        id,
        "chunk-index",
        "children",
        serde_json::to_value(page)?,
    )?;
    Ok(id)
}

fn push_chunk_id(
    tx: &Transaction<'_>,
    levels: &mut Vec<Vec<Uuid>>,
    level: usize,
    id: Uuid,
) -> Result<()> {
    if levels.len() == level {
        levels.push(Vec::new());
    }
    if levels[level].len() == INDEX_FANOUT {
        let parent = store_page(tx, &levels[level])?;
        levels[level].clear();
        push_chunk_id(tx, levels, level + 1, parent)?;
    }
    levels[level].push(id);
    Ok(())
}

fn collect_chunks(tx: &Transaction<'_>, mut reader: impl Read) -> Result<ChunkList> {
    let mut buffer = vec![0; CHUNK_BYTES];
    let mut levels = vec![Vec::new()];
    let mut size = 0usize;
    loop {
        let mut used = 0;
        while used < buffer.len() {
            match reader.read(&mut buffer[used..]) {
                Ok(0) => break,
                Ok(n) => used += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Other(e.into())),
            }
        }
        if used == 0 {
            break;
        }
        let chunk = &buffer[..used];
        let id = chunk_id(chunk);
        put(
            tx,
            id,
            CHUNKS,
            "data",
            Value::String(STANDARD.encode(chunk)),
        )?;
        push_chunk_id(tx, &mut levels, 0, id)?;
        size = size
            .checked_add(used)
            .ok_or_else(|| Error::Invalid("Content exceeds platform address space".into()))?;
        if used < CHUNK_BYTES {
            break;
        }
    }
    let mut level = 0;
    while level + 1 < levels.len() {
        let id = store_page(tx, &levels[level])?;
        push_chunk_id(tx, &mut levels, level + 1, id)?;
        level += 1;
    }
    Ok(ChunkList {
        size,
        chunks: levels.pop().unwrap(),
        level: level as u8,
    })
}

fn visit_chunks(
    conn: &Connection,
    list: ChunkList,
    consume: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let invalid = || Error::Invalid("Invalid synchronized content length".into());
    // A valid minimal tree cannot have more levels than a platform-sized file.
    if u32::from(list.level) > usize::BITS.div_ceil(INDEX_FANOUT.ilog2()) {
        return Err(invalid());
    }
    let capacity = CHUNK_BYTES.saturating_mul(INDEX_FANOUT.saturating_pow(list.level.into()));
    if list.chunks.len() != list.size.div_ceil(capacity) {
        return Err(invalid());
    }
    let mut remaining = list.size;
    for id in list.chunks {
        let size = remaining.min(capacity);
        remaining -= size;
        if list.level == 0 {
            let data: Option<String> = conn.prepare_cached(
                "SELECT json_extract(fields_json,'$.data') FROM plugin_records WHERE record_uuid=?1 AND plugin_id=?2 AND collection=?3 AND deleted=0",
            )?.query_row(params![id.as_bytes(), PLUGIN, CHUNKS], |row| row.get(0)).optional()?.flatten();
            let chunk = STANDARD
                .decode(data.ok_or_else(pending)?)
                .map_err(|_| pending())?;
            if chunk.len() != size || chunk_id(&chunk) != id {
                return Err(pending());
            }
            consume(&chunk)?;
        } else {
            let json: Option<String> = conn.prepare_cached(
                "SELECT json_extract(fields_json,'$.children') FROM plugin_records WHERE record_uuid=?1 AND plugin_id=?2 AND collection='chunk-index' AND deleted=0",
            )?.query_row(params![id.as_bytes(), PLUGIN], |row| row.get(0)).optional()?.flatten();
            let children: Vec<Uuid> =
                serde_json::from_str(&json.ok_or_else(pending)?).map_err(|_| pending())?;
            if children.len() > INDEX_FANOUT || index_id(&children) != id {
                return Err(pending());
            }
            visit_chunks(
                conn,
                ChunkList {
                    size,
                    chunks: children,
                    level: list.level - 1,
                },
                consume,
            )?;
        }
    }
    Ok(())
}

fn parse_uuid(bytes: Vec<u8>) -> rusqlite::Result<Uuid> {
    Uuid::from_slice(&bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(e))
    })
}

fn get_document(conn: &Connection, id: Uuid) -> Result<NotebookNote> {
    let (head, revision, created_at) = record_head::<DocumentHead>(conn, id, DOCUMENTS)?;
    let markdown = head.source.text(conn)?;
    let mut query = conn.prepare(
        "SELECT record_uuid,json_extract(fields_json,'$.head.name'),json_extract(fields_json,'$.head.mimeType'),
                json_extract(fields_json,'$.head.contents.size') FROM plugin_records
         WHERE plugin_id=?1 AND collection=?2 AND deleted=0 AND json_extract(fields_json,'$.head.noteId')=?3
         ORDER BY created_at,record_uuid",
    )?;
    let attachments = query
        .query_map(params![PLUGIN, ATTACHMENTS, id.to_string()], |row| {
            Ok(NoteAttachment {
                id: parse_uuid(row.get(0)?)?,
                name: row.get(1)?,
                mime_type: row.get(2)?,
                size: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(NotebookNote {
        summary: NoteSummary {
            id,
            title: head.title,
            format: head.format,
            revision,
            created_at,
            updated_at: head.updated_at,
        },
        markdown,
        styles: head.styles,
        attachments,
    })
}

fn list_summaries(conn: &Connection, query: Option<&str>) -> Result<Vec<NoteSummary>> {
    let (predicate, needle) = match query {
        Some(query) if query.chars().count() >= 3 => (
            "AND id IN (SELECT p.record_id FROM notebook_fts JOIN notebook_search_parts p ON p.id=notebook_fts.rowid WHERE notebook_fts MATCH ?3)",
            format!("\"{}\"", query.replace('"', "\"\"")),
        ),
        Some(query) => (
            "AND id IN (SELECT p.record_id FROM notebook_fts JOIN notebook_search_parts p ON p.id=notebook_fts.rowid WHERE instr(search_text,?3)>0)",
            query.to_owned(),
        ),
        None => ("AND ?3 IS NULL", String::new()),
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT record_uuid,json_extract(fields_json,'$.head.title'),json_extract(fields_json,'$.head.format'),
         revision,created_at,json_extract(fields_json,'$.head.updatedAt') AS edited FROM plugin_records
         WHERE plugin_id=?1 AND collection=?2 AND deleted=0 AND json_type(fields_json,'$.head')='object'
         {predicate} ORDER BY edited DESC,record_uuid"
    ))?;
    let rows = stmt.query_map(params![PLUGIN, DOCUMENTS, query.map(|_| needle)], |row| {
        let format: String = row.get(2)?;
        Ok(NoteSummary {
            id: parse_uuid(row.get(0)?)?,
            title: row.get(1)?,
            format: match format.as_str() {
                "markdown" => NoteFormat::Markdown,
                "html" => NoteFormat::Html,
                "document" => NoteFormat::Document,
                _ => return Err(rusqlite::Error::InvalidQuery),
            },
            revision: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn project_search_entry(
    conn: &Connection,
    id: &Uuid,
    head: Option<&Value>,
) -> Result<()> {
    let rowid = conn.query_row(
        "SELECT id FROM plugin_records WHERE record_uuid=?1",
        [id.as_bytes()],
        |row| row.get::<_, i64>(0),
    )?;
    index_head(
        conn,
        rowid,
        head.and_then(|value| serde_json::from_value(value.clone()).ok()),
    )
}

const MAX_QUERY_CHARS: usize = 512;
const SEARCH_PART_BYTES: usize = 64 * 1024;

fn clear_search_parts(conn: &Connection, rowid: i64) -> Result<()> {
    conn.execute("DELETE FROM notebook_fts WHERE rowid IN (SELECT id FROM notebook_search_parts WHERE record_id=?1)", [rowid])?;
    conn.execute(
        "DELETE FROM notebook_search_parts WHERE record_id=?1",
        [rowid],
    )?;
    Ok(())
}

fn insert_search_part(conn: &Connection, rowid: i64, text: &str) -> Result<()> {
    let part = conn
        .prepare_cached("INSERT INTO notebook_search_parts(record_id) VALUES (?1)")?
        .insert([rowid])?;
    conn.prepare_cached("INSERT INTO notebook_fts(rowid,search_text) VALUES (?1,?2)")?
        .execute(params![part, text])?;
    Ok(())
}

fn index_source(conn: &Connection, rowid: i64, head: DocumentHead) -> Result<()> {
    let invalid = || Error::Invalid("Note source is not valid UTF-8".into());
    let mut part = String::new();
    let mut append = |text: &str| -> Result<()> {
        for ch in text.chars().flat_map(char::to_lowercase) {
            part.push(ch);
            if part.len() >= SEARCH_PART_BYTES {
                insert_search_part(conn, rowid, &part)?;
                let overlap = part
                    .char_indices()
                    .rev()
                    .nth(MAX_QUERY_CHARS - 2)
                    .unwrap()
                    .0;
                part.drain(..overlap);
            }
        }
        Ok(())
    };
    append(&head.title)?;
    append("\n")?;
    match head.source {
        Source::Text(text) => append(&text)?,
        Source::Chunks(list) => {
            let mut buffer = Vec::with_capacity(CHUNK_BYTES + 3);
            visit_chunks(conn, list, &mut |chunk| {
                buffer.extend_from_slice(chunk);
                match std::str::from_utf8(&buffer) {
                    Ok(text) => {
                        append(text)?;
                        buffer.clear();
                    }
                    Err(e) if e.error_len().is_none() => {
                        let valid = e.valid_up_to();
                        append(std::str::from_utf8(&buffer[..valid]).map_err(|_| invalid())?)?;
                        buffer.drain(..valid);
                    }
                    Err(_) => return Err(invalid()),
                }
                Ok(())
            })?;
            if !buffer.is_empty() {
                return Err(invalid());
            }
        }
    }
    insert_search_part(conn, rowid, &part)
}

fn index_head(conn: &Connection, rowid: i64, head: Option<DocumentHead>) -> Result<()> {
    clear_search_parts(conn, rowid)?;
    if let Some(head) = head {
        match index_source(conn, rowid, head) {
            Ok(()) => {}
            Err(Error::Conflict(_)) => {
                clear_search_parts(conn, rowid)?;
                conn.execute(
                    "INSERT OR IGNORE INTO notebook_search_pending(record_id) VALUES(?1)",
                    [rowid],
                )?;
                return Ok(());
            }
            Err(Error::Invalid(_)) => clear_search_parts(conn, rowid)?,
            Err(other) => return Err(other),
        }
    }
    conn.execute(
        "DELETE FROM notebook_search_pending WHERE record_id=?1",
        [rowid],
    )?;
    Ok(())
}

fn require_revision(conn: &Connection, id: Uuid, expected: i64) -> Result<()> {
    let revision: i64 = conn.query_row(
        "SELECT revision FROM plugin_records WHERE record_uuid=?1 AND plugin_id=?2 AND collection=?3 AND deleted=0",
        params![id.as_bytes(), PLUGIN, DOCUMENTS], |row| row.get(0),
    ).optional()?.ok_or_else(missing)?;
    if expected != revision {
        return Err(Error::Conflict("note_revision_conflict".into()));
    }
    Ok(())
}

impl Database {
    pub fn list_notes(&self) -> Result<Vec<NoteSummary>> {
        list_summaries(&self.connection, None)
    }

    pub fn search_notes(&self, query: &str) -> Result<Vec<NoteSummary>> {
        let query = query
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if query.is_empty() {
            return self.list_notes();
        }
        if query.chars().count() > MAX_QUERY_CHARS || query.contains('\0') {
            return Err(Error::Invalid(
                "Search text must contain at most 512 characters and no NUL".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        // Older catalogs and remote heads delivered before their chunks may
        // lack an index row. Catch up only those documents, inside the same
        // snapshot as the query. Ordinary saves and complete remote heads
        // update the index as part of the projection write.
        let mut missing = tx.prepare("SELECT record_id FROM notebook_search_pending")?;
        // Keep only keys in memory; a large library may need a first-time rebuild.
        let mut stale = Vec::new();
        for row in missing.query_map([], |row| row.get::<_, i64>(0))? {
            stale.push(row?);
        }
        drop(missing);
        for rowid in stale {
            let head: Option<String> = tx.query_row(
                "SELECT json_extract(fields_json,'$.head') FROM plugin_records WHERE id=?1 AND deleted=0",
                [rowid], |row| row.get(0),
            ).optional()?.flatten();
            index_head(
                &tx,
                rowid,
                head.and_then(|json| serde_json::from_str(&json).ok()),
            )?;
        }
        let result = list_summaries(&tx, Some(&query))?;
        tx.commit()?;
        Ok(result)
    }

    pub fn notebook_note(&self, id: &str) -> Result<NotebookNote> {
        let tx = self.connection.unchecked_transaction()?;
        get_document(&tx, parse_record_id(id)?)
    }

    pub fn save_note(
        &self,
        existing: Option<(&str, i64)>,
        input: NoteInput,
    ) -> Result<NotebookNote> {
        self.write_note(existing, input, None)
    }

    pub fn create_note_in_folder(
        &self,
        input: NoteInput,
        folder: Option<Uuid>,
    ) -> Result<NotebookNote> {
        self.write_note(None, input, folder)
    }

    fn write_note(
        &self,
        existing: Option<(&str, i64)>,
        input: NoteInput,
        folder: Option<Uuid>,
    ) -> Result<NotebookNote> {
        let title = input.title.trim();
        if title.is_empty()
            || title.chars().count() > 512
            || serde_json::to_vec(&input.styles)?.len() > 16 * 1024
        {
            return Err(Error::Invalid(
                "A title of 1–512 characters and styles up to 16 KiB are required".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let id = if let Some((id, revision)) = existing {
            let id = parse_record_id(id)?;
            require_revision(&tx, id, revision)?;
            id
        } else {
            Uuid::now_v7()
        };
        if let Some(folder) = folder {
            tree::require_folder(&tx, folder)?;
            tree::put_placement(&tx, id, Some(folder))?;
        }
        let mut head = DocumentHead {
            title: title.into(),
            format: input.format,
            source: if input.markdown.len() > MAX_FIELD_VALUE_BYTES {
                Source::Chunks(collect_chunks(&tx, input.markdown.as_bytes())?)
            } else {
                Source::Text(input.markdown)
            },
            styles: input.styles,
            updated_at: unix_timestamp_millis(),
        };
        let mut value = serde_json::to_value(&head)?;
        if serde_json::to_vec(&value)?.len() > MAX_FIELD_VALUE_BYTES {
            if let Source::Text(text) = &head.source {
                head.source = Source::Chunks(collect_chunks(&tx, text.as_bytes())?);
                value = serde_json::to_value(&head)?;
            }
        }
        put(&tx, id, DOCUMENTS, "head", value)?;
        let result = get_document(&tx, id)?;
        tx.commit()?;
        plugin_record_wake().notify_waiters();
        Ok(result)
    }

    pub fn delete_note(&self, id: &str, revision: i64) -> Result<()> {
        let id = parse_record_id(id)?;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        require_revision(&tx, id, revision)?;
        apply_operation(
            &tx,
            PLUGIN,
            PluginRecordOperation::Delete { id: id.to_string() },
            false,
        )?;
        tx.commit()?;
        plugin_record_wake().notify_waiters();
        Ok(())
    }

    pub fn collect_note_attachment(
        &self,
        note_id: &str,
        name: String,
        mime_type: String,
        reader: impl Read,
    ) -> Result<NoteAttachment> {
        let note_id = parse_record_id(note_id)?;
        if name.trim().is_empty()
            || name.chars().count() > 255
            || name.contains(['/', '\\'])
            || name.chars().any(char::is_control)
        {
            return Err(Error::Invalid("Invalid attachment filename".into()));
        }
        // File.type may include MIME parameters (for example a text charset).
        let mime_type = mime_type.split(';').next().unwrap_or_default().trim();
        let mime_type = if mime_type.is_empty() {
            "application/octet-stream".into()
        } else {
            mime_type.to_ascii_lowercase()
        };
        let valid_token = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
        };
        if mime_type.len() > 127
            || !mime_type
                .split_once('/')
                .is_some_and(|(a, b)| valid_token(a) && valid_token(b))
        {
            return Err(Error::Invalid("Invalid attachment media type".into()));
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        record_head::<DocumentHead>(&tx, note_id, DOCUMENTS)?;
        let contents = collect_chunks(&tx, reader)?;
        let size = contents.size;
        let head = AttachmentHead {
            note_id,
            name: name.clone(),
            mime_type: mime_type.clone(),
            contents,
        };
        // Retrying the same upload, or collecting the same original twice in
        // this note, returns one stable attachment. Ownership is part of it.
        let id = content_id(
            b"mangoo.notebook.attachment.v1\0",
            &serde_json::to_vec(&head)?,
        );
        put(&tx, id, ATTACHMENTS, "head", serde_json::to_value(head)?)?;
        tx.commit()?;
        plugin_record_wake().notify_waiters();
        Ok(NoteAttachment {
            id,
            name,
            mime_type,
            size,
        })
    }

    pub fn write_note_attachment(
        &self,
        note_id: &str,
        id: &str,
        writer: &mut impl Write,
    ) -> Result<(String, usize)> {
        let tx = self.connection.unchecked_transaction()?;
        let note_id = parse_record_id(note_id)?;
        record_head::<DocumentHead>(&tx, note_id, DOCUMENTS)?;
        let (head, _, _) = record_head::<AttachmentHead>(&tx, parse_record_id(id)?, ATTACHMENTS)?;
        if note_id != head.note_id {
            return Err(missing());
        }
        let size = head.contents.size;
        visit_chunks(&tx, head.contents, &mut |chunk| {
            writer.write_all(chunk).map_err(|e| Error::Other(e.into()))
        })?;
        Ok((head.mime_type, size))
    }

    #[cfg(test)]
    pub(crate) fn read_note_attachment(&self, note: &str, id: &str) -> Result<(String, Vec<u8>)> {
        let mut bytes = Vec::new();
        let (mime, _) = self.write_note_attachment(note, id, &mut bytes)?;
        Ok((mime, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.apply_migrations().unwrap();
        db.initialize_sync_identity(Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7())
            .unwrap();
        db
    }

    fn input(source: &str) -> NoteInput {
        NoteInput {
            title: "草稿".into(),
            format: NoteFormat::Markdown,
            markdown: source.into(),
            styles: Map::from_iter([("h1".into(), serde_json::json!({"color":"#096"}))]),
        }
    }

    #[test]
    fn notebook_capture_replays_source_styles_and_original_attachment_bytes() {
        let local = catalog();
        let source = "正文✨ \r\n".repeat(24000);
        let folder = local
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "项目".into(),
                    parent_id: None,
                },
            )
            .unwrap();
        let child = local
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "研究".into(),
                    parent_id: Some(folder.id),
                },
            )
            .unwrap();
        let note = local
            .create_note_in_folder(input(&source), Some(child.id))
            .unwrap();
        let id = note.summary.id.to_string();
        // Initial source was created offline. Backfill it on binding.
        local.enable_fake_sync_capture_for_test().unwrap();
        let tx = local.connection.unchecked_transaction().unwrap();
        assert!(super::super::plugin_records::capture_uncaptured_plugin_records(&tx).unwrap() > 0);
        tx.commit().unwrap();
        let file = (0..CHUNK_BYTES * 3 + 29)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        let attachment = local
            .collect_note_attachment(
                &id,
                "文档.pdf".into(),
                "application/pdf".into(),
                file.as_slice(),
            )
            .unwrap();
        let copy = catalog();
        let mut stmt = local
            .connection
            .prepare("SELECT payload FROM sync_events ORDER BY actor_seq")
            .unwrap();
        let events = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0)).unwrap();
        let tx = copy.connection.unchecked_transaction().unwrap();
        for bytes in events {
            let event = crate::sync::codec::decode_event(&bytes.unwrap()).unwrap();
            crate::sync::inbox::apply_remote_batch(&tx, std::slice::from_ref(&event)).unwrap();
        }
        tx.commit().unwrap();
        let restored = copy.notebook_note(&id).unwrap();
        let tree = copy.notebook_tree().unwrap();
        assert_eq!(tree.note_parents[&restored.summary.id], child.id);
        assert_eq!(
            tree.folders
                .iter()
                .find(|item| item.id == child.id)
                .unwrap()
                .parent_id,
            Some(folder.id)
        );
        assert_eq!(restored.markdown, source);
        assert_eq!(
            copy.search_notes("正文✨").unwrap()[0].id,
            restored.summary.id
        );
        assert_eq!(restored.styles["h1"]["color"], "#096");
        assert_eq!(restored.attachments[0].name, "文档.pdf");
        let (mime, data) = copy
            .read_note_attachment(&id, &attachment.id.to_string())
            .unwrap();
        assert_eq!(mime, "application/pdf");
        assert_eq!(data, file);
    }

    #[test]
    fn notebook_folder_moves_are_atomic_and_independent_of_document_revisions() {
        let db = catalog();
        let root = db
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "作品".into(),
                    parent_id: None,
                },
            )
            .unwrap();
        let child = db
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "参考".into(),
                    parent_id: Some(root.id),
                },
            )
            .unwrap();
        let legacy = db.save_note(None, input("legacy")).unwrap().summary;
        assert!(db.notebook_tree().unwrap().note_parents.is_empty());
        assert!(matches!(
            db.save_notebook_folder(
                Some((root.id, root.revision)),
                tree::FolderInput {
                    name: "作品".into(),
                    parent_id: Some(child.id)
                }
            ),
            Err(Error::Invalid(_))
        ));
        db.move_notebook_note(legacy.id, None, Some(child.id))
            .unwrap();
        assert!(matches!(
            db.move_notebook_note(legacy.id, None, Some(root.id)),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            db.delete_notebook_folder(child.id, child.revision),
            Err(Error::Conflict(_))
        ));
        db.save_note(
            Some((&legacy.id.to_string(), legacy.revision)),
            input("edited"),
        )
        .unwrap();
        assert_eq!(
            db.notebook_tree().unwrap().note_parents[&legacy.id],
            child.id
        );
        db.move_notebook_note(legacy.id, Some(child.id), None)
            .unwrap();
        let renamed = db
            .save_notebook_folder(
                Some((child.id, child.revision)),
                tree::FolderInput {
                    name: "归档".into(),
                    parent_id: None,
                },
            )
            .unwrap();
        assert!(matches!(
            db.delete_notebook_folder(child.id, child.revision),
            Err(Error::Conflict(_))
        ));
        db.delete_notebook_folder(renamed.id, renamed.revision)
            .unwrap();
        assert!(matches!(
            db.create_note_in_folder(input("partial"), Some(child.id)),
            Err(Error::NotFound(_))
        ));
        assert_eq!(db.list_notes().unwrap().len(), 1);
        assert_eq!(db.notebook_tree().unwrap().folders.len(), 1);
    }

    #[test]
    fn notebook_sibling_names_are_unique_for_local_writes_and_legacy_ids_remain_distinct() {
        let db = catalog();
        let folder = |name: &str, parent_id| tree::FolderInput {
            name: name.to_owned(),
            parent_id,
        };
        let parent = db.save_notebook_folder(None, folder("项目", None)).unwrap();
        let original = db
            .save_notebook_folder(None, folder("Résumé", None))
            .unwrap();
        let child = db
            .save_notebook_folder(None, folder("résumé", Some(parent.id)))
            .unwrap();
        let equivalent = " RE\u{301}SUME\u{301} ";
        for attempt in [
            db.save_notebook_folder(None, folder(equivalent, None)),
            db.save_notebook_folder(Some((parent.id, parent.revision)), folder(equivalent, None)),
            db.save_notebook_folder(Some((child.id, child.revision)), folder(equivalent, None)),
        ] {
            assert!(
                matches!(attempt, Err(Error::Conflict(code)) if code == tree::DUPLICATE_FOLDER_NAME)
            );
        }
        // Independent offline creation can replay the same sibling name. Both
        // UUIDs survive, and either may subsequently be renamed or moved.
        let legacy = Uuid::now_v7();
        let tx = db.connection.unchecked_transaction().unwrap();
        put(&tx, legacy, "folders", "name", serde_json::json!("résumé")).unwrap();
        tx.commit().unwrap();
        let restored = db.notebook_tree().unwrap();
        assert!(restored.folders.iter().any(|f| f.id == original.id));
        let duplicate = restored.folders.iter().find(|f| f.id == legacy).unwrap();
        db.save_notebook_folder(Some((legacy, duplicate.revision)), folder("归档", None))
            .unwrap();
        assert_eq!(db.notebook_tree().unwrap().folders.len(), 4);
    }

    #[test]
    fn notebook_tree_remains_traversable_after_cross_device_cycle_or_deleted_parent() {
        let db = catalog();
        let a = db
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "A".into(),
                    parent_id: None,
                },
            )
            .unwrap();
        let b = db
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "B".into(),
                    parent_id: Some(a.id),
                },
            )
            .unwrap();
        let c = db
            .save_notebook_folder(
                None,
                tree::FolderInput {
                    name: "C".into(),
                    parent_id: Some(b.id),
                },
            )
            .unwrap();
        let tx = db.connection.unchecked_transaction().unwrap();
        put(&tx, a.id, "folders", "parentId", serde_json::json!(b.id)).unwrap();
        tx.commit().unwrap();
        let tree = db.notebook_tree().unwrap();
        assert_eq!(
            tree.folders
                .iter()
                .find(|f| f.parent_id.is_none())
                .unwrap()
                .id,
            a.id.min(b.id)
        );
        assert_eq!(
            tree.folders
                .iter()
                .find(|f| f.id == c.id)
                .unwrap()
                .parent_id,
            Some(b.id)
        );
        let tx = db.connection.unchecked_transaction().unwrap();
        apply_operation(
            &tx,
            PLUGIN,
            PluginRecordOperation::Delete {
                id: b.id.to_string(),
            },
            false,
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(db
            .notebook_tree()
            .unwrap()
            .folders
            .iter()
            .all(|f| f.parent_id.is_none()));
    }

    #[test]
    fn notebook_compare_and_save_is_atomic_and_deleted_note_hides_attachments() {
        let db = catalog();
        let first = db.save_note(None, input("first")).unwrap();
        let id = first.summary.id.to_string();
        let other = db.save_note(None, input("other")).unwrap();
        let updated = db
            .save_note(Some((&id, first.summary.revision)), input("revised"))
            .unwrap();
        assert!(matches!(
            db.save_note(Some((&id, first.summary.revision)), input("stale")),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            db.delete_note(&id, first.summary.revision),
            Err(Error::Conflict(_))
        ));
        assert_eq!(db.notebook_note(&id).unwrap().markdown, "revised");
        let file = db
            .collect_note_attachment(
                &id,
                "image.png".into(),
                "image/png".into(),
                &[0, 255, 8][..],
            )
            .unwrap()
            .id
            .to_string();
        let repeat = db
            .collect_note_attachment(
                &id,
                "image.png".into(),
                "image/png".into(),
                &[0, 255, 8][..],
            )
            .unwrap();
        assert_eq!(file, repeat.id.to_string());
        let note = db.notebook_note(&id).unwrap();
        assert_eq!(note.summary.revision, updated.summary.revision);
        assert_eq!(note.attachments.len(), 1);
        assert!(matches!(
            db.read_note_attachment(&other.summary.id.to_string(), &file),
            Err(Error::NotFound(_))
        ));
        db.delete_note(&id, updated.summary.revision).unwrap();
        assert_eq!(db.list_notes().unwrap().len(), 1);
        assert!(db.search_notes("revised").unwrap().is_empty());
        assert!(matches!(
            db.read_note_attachment(&id, &file),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            db.save_note(Some((&id, updated.summary.revision)), input("late")),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn notebook_search_tracks_edits_literals_and_late_synchronized_chunks() {
        let db = catalog();
        let created = db
            .save_note(None, input("中文正文 – quote \"hi\" & 100%"))
            .unwrap();
        let id = created.summary.id;
        for literal in ["中文", "中文正", "QUOTE", "\"hi\"", "100%", "草稿"] {
            assert_eq!(db.search_notes(literal).unwrap()[0].id, id, "{literal}");
        }
        let empty_library = catalog();
        assert!(empty_library.search_notes("中文正").unwrap().is_empty());
        let source = "abcdef0123456789".repeat(15000) + "尾部命中";
        let updated = db
            .save_note(
                Some((&id.to_string(), created.summary.revision)),
                input(&source),
            )
            .unwrap();
        assert!(db.search_notes("中文").unwrap().is_empty());
        // A note head can precede one of its chunk fields during replica replay.
        let first_chunk = chunk_id(&source.as_bytes()[..CHUNK_BYTES]);
        let fields: String = db
            .connection
            .query_row(
                "SELECT fields_json FROM plugin_records WHERE record_uuid=?1",
                [first_chunk.as_bytes()],
                |r| r.get(0),
            )
            .unwrap();
        db.connection
            .execute(
                "UPDATE plugin_records SET fields_json='{}' WHERE record_uuid=?1",
                [first_chunk.as_bytes()],
            )
            .unwrap();
        let head = db
            .get_plugin_record(PLUGIN, &id.to_string())
            .unwrap()
            .fields;
        project_search_entry(&db.connection, &id, head.get("head")).unwrap();
        assert!(db.search_notes("尾部命中").unwrap().is_empty());
        db.connection
            .execute(
                "UPDATE plugin_records SET fields_json=?2 WHERE record_uuid=?1",
                params![first_chunk.as_bytes(), fields],
            )
            .unwrap();
        assert_eq!(
            db.search_notes("尾部命中").unwrap()[0].revision,
            updated.summary.revision
        );
        assert_eq!(
            db.connection
                .query_row("SELECT count(*) FROM notebook_search_pending", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn notebook_imported_document_keeps_its_primary_file_and_markdown_annotations() {
        let db = catalog();
        let mut initial = input("");
        initial.format = NoteFormat::Document;
        initial.title = "report.xlsx".into();
        let created = db.save_note(None, initial).unwrap();
        let id = created.summary.id.to_string();
        let attachment = db
            .collect_note_attachment(
                &id,
                created.summary.title.clone(),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
                &b"original bytes"[..],
            )
            .unwrap();
        let source = format!(
            "[report.xlsx](mangoo-attachment:{})\n\nAnnotation",
            attachment.id
        );
        let mut edited = input(&source);
        edited.format = NoteFormat::Document;
        edited.title = created.summary.title;
        db.save_note(Some((&id, created.summary.revision)), edited)
            .unwrap();
        let reopened = db.notebook_note(&id).unwrap();
        assert!(matches!(reopened.summary.format, NoteFormat::Document));
        assert!(matches!(
            db.list_notes().unwrap()[0].format,
            NoteFormat::Document
        ));
        assert_eq!(reopened.markdown, source);
        assert_eq!(reopened.attachments[0].id, attachment.id);
    }
}

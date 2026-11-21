use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

const FOLDERS: &str = "folders";
const PLACEMENTS: &str = "placements";
pub const DUPLICATE_FOLDER_NAME: &str = "notebook_duplicate_folder_name";

// Match the directory importer's ECMAScript trim, NFC and default lowercase.
fn trim_folder_name(name: &str) -> &str {
    name.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{0085}'))
}

fn sibling_name_key(name: &str) -> String {
    trim_folder_name(name)
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInput {
    pub name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookFolder {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub revision: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookTree {
    pub folders: Vec<NotebookFolder>,
    pub note_parents: BTreeMap<Uuid, Uuid>,
}

fn folder_missing() -> Error {
    Error::NotFound("文件夹不存在，可能已在其他设备上删除。".into())
}

pub(super) fn require_folder(conn: &Connection, id: Uuid) -> Result<()> {
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM plugin_records WHERE record_uuid=?1 AND plugin_id=?2 AND collection=?3 AND deleted=0 AND json_type(fields_json,'$.name')='text')",
        params![id.as_bytes(), PLUGIN, FOLDERS], |row| row.get(0))?;
    if !exists {
        return Err(folder_missing());
    }
    Ok(())
}

fn placement_id(note: Uuid) -> Uuid {
    content_id(b"mangoo.notebook.placement.v1\0", note.as_bytes())
}

pub(super) fn put_placement(tx: &Transaction<'_>, note: Uuid, parent: Option<Uuid>) -> Result<()> {
    put(
        tx,
        placement_id(note),
        PLACEMENTS,
        "head",
        serde_json::json!({ "noteId": note, "parentId": parent }),
    )
}

fn load_folders(conn: &Connection) -> Result<BTreeMap<Uuid, NotebookFolder>> {
    let mut stmt = conn.prepare("SELECT record_uuid, json_extract(fields_json,'$.name'), unhex(json_extract(fields_json,'$.parentId'),'-'), revision FROM plugin_records
        WHERE plugin_id=?1 AND collection=?2 AND deleted=0 AND json_type(fields_json,'$.name')='text'")?;
    let folders = stmt
        .query_map(params![PLUGIN, FOLDERS], |row| {
            let id = parse_uuid(row.get(0)?)?;
            Ok((
                id,
                NotebookFolder {
                    id,
                    name: row.get(1)?,
                    parent_id: row
                        .get::<_, Option<Vec<u8>>>(2)?
                        .map(parse_uuid)
                        .transpose()?,
                    revision: row.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    Ok(folders)
}

// A delete on one device and a move on another can leave a missing parent.
// Concurrent moves in opposite directions can form a cycle. Every replica
// presents the same traversable tree, even while events arrive out of order.
fn normalize_parents(folders: &mut BTreeMap<Uuid, NotebookFolder>) {
    let ids = folders.keys().copied().collect::<HashSet<_>>();
    for folder in folders.values_mut() {
        folder.parent_id = folder.parent_id.filter(|parent| ids.contains(parent));
    }
    let mut done = HashSet::new();
    for id in ids {
        if done.contains(&id) {
            continue;
        }
        let mut path = Vec::new();
        let mut positions = HashMap::new();
        let mut cursor = Some(id);
        while let Some(node) = cursor {
            if done.contains(&node) {
                break;
            }
            if let Some(start) = positions.insert(node, path.len()) {
                let root = path[start..].iter().min().unwrap();
                folders.get_mut(root).unwrap().parent_id = None;
                break;
            }
            path.push(node);
            cursor = folders[&node].parent_id;
        }
        done.extend(path);
    }
}

fn read_tree(conn: &Connection) -> Result<NotebookTree> {
    let mut folders = load_folders(conn)?;
    normalize_parents(&mut folders);
    // Look up each live document by UUID in SQLite's unique index. A stale
    // placement from a remote delete cannot make an empty folder undeletable.
    let mut stmt = conn.prepare("SELECT d.record_uuid, unhex(json_extract(p.fields_json,'$.head.parentId'),'-')
        FROM plugin_records p CROSS JOIN plugin_records d ON d.record_uuid=unhex(json_extract(p.fields_json,'$.head.noteId'),'-')
        WHERE p.plugin_id=?1 AND p.collection=?2 AND p.deleted=0 AND d.plugin_id=?1 AND d.collection=?3 AND d.deleted=0
        AND json_type(p.fields_json,'$.head.parentId')='text'")?;
    let mut note_parents = BTreeMap::new();
    for result in stmt.query_map(params![PLUGIN, PLACEMENTS, DOCUMENTS], |row| {
        Ok((parse_uuid(row.get(0)?)?, parse_uuid(row.get(1)?)?))
    })? {
        let (id, parent) = result?;
        if folders.contains_key(&parent) {
            note_parents.insert(id, parent);
        }
    }
    Ok(NotebookTree {
        folders: folders.into_values().collect(),
        note_parents,
    })
}

fn check_folder_revision(folder: &NotebookFolder, revision: i64) -> Result<()> {
    if folder.revision != revision {
        return Err(Error::Conflict("文件夹已更改，请刷新后重试。".into()));
    }
    Ok(())
}

impl Database {
    pub fn notebook_tree(&self) -> Result<NotebookTree> {
        let tx = self.connection.unchecked_transaction()?;
        read_tree(&tx)
    }

    pub fn save_notebook_folder(
        &self,
        existing: Option<(Uuid, i64)>,
        input: FolderInput,
    ) -> Result<NotebookFolder> {
        let name = trim_folder_name(&input.name);
        if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "文件夹名称须为 1 到 200 个可显示字符。".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let mut folders = load_folders(&tx)?;
        normalize_parents(&mut folders);
        let id = if let Some((id, revision)) = existing {
            check_folder_revision(folders.get(&id).ok_or_else(folder_missing)?, revision)?;
            let mut cursor = input.parent_id;
            while let Some(parent) = cursor {
                if parent == id {
                    return Err(Error::Invalid("无法移动到自身或下级文件夹。".into()));
                }
                cursor = folders.get(&parent).ok_or_else(folder_missing)?.parent_id;
            }
            id
        } else {
            if let Some(parent) = input.parent_id {
                folders.get(&parent).ok_or_else(folder_missing)?;
            }
            Uuid::now_v7()
        };
        let name_key = sibling_name_key(name);
        let same_location_and_name = folders.get(&id).is_some_and(|current| {
            current.parent_id == input.parent_id && sibling_name_key(&current.name) == name_key
        });
        // Existing equal names (e.g. two offline creations) remain addressable
        // by UUID. Check only when a local write introduces a new sibling key.
        if !same_location_and_name
            && folders.values().any(|other| {
                other.id != id
                    && other.parent_id == input.parent_id
                    && sibling_name_key(&other.name) == name_key
            })
        {
            return Err(Error::Conflict(DUPLICATE_FOLDER_NAME.into()));
        }
        apply_operation(
            &tx,
            PLUGIN,
            PluginRecordOperation::Put {
                id: id.to_string(),
                collection: FOLDERS.into(),
                fields: Map::from_iter([
                    ("name".into(), Value::String(name.into())),
                    ("parentId".into(), serde_json::to_value(input.parent_id)?),
                ]),
                unset: Vec::new(),
            },
            false,
        )?;
        let revision = tx.query_row(
            "SELECT revision FROM plugin_records WHERE record_uuid=?1",
            [id.as_bytes()],
            |row| row.get(0),
        )?;
        tx.commit()?;
        plugin_record_wake().notify_waiters();
        Ok(NotebookFolder {
            id,
            name: name.into(),
            parent_id: input.parent_id,
            revision,
        })
    }

    pub fn delete_notebook_folder(&self, id: Uuid, revision: i64) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let tree = read_tree(&tx)?;
        let folder = tree
            .folders
            .iter()
            .find(|folder| folder.id == id)
            .ok_or_else(folder_missing)?;
        check_folder_revision(folder, revision)?;
        if tree
            .folders
            .iter()
            .any(|folder| folder.parent_id == Some(id))
            || tree.note_parents.values().any(|parent| *parent == id)
        {
            return Err(Error::Conflict("请先移出文件夹中的笔记和子文件夹。".into()));
        }
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

    pub fn move_notebook_note(
        &self,
        note: Uuid,
        expected: Option<Uuid>,
        parent: Option<Uuid>,
    ) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM plugin_records WHERE record_uuid=?1 AND plugin_id=?2 AND collection=?3 AND deleted=0)",
            params![note.as_bytes(), PLUGIN, DOCUMENTS], |row| row.get(0))?;
        if !exists {
            return Err(missing());
        }
        if let Some(folder) = parent {
            require_folder(&tx, folder)?;
        }
        let current = tx.query_row(
            "SELECT f.record_uuid FROM plugin_records p JOIN plugin_records f ON f.record_uuid=unhex(json_extract(p.fields_json,'$.head.parentId'),'-')
             WHERE p.record_uuid=?1 AND p.plugin_id=?2 AND p.collection=?3 AND p.deleted=0 AND f.plugin_id=?2 AND f.collection=?4 AND f.deleted=0 AND json_type(f.fields_json,'$.name')='text'",
            params![placement_id(note).as_bytes(), PLUGIN, PLACEMENTS, FOLDERS], |row| parse_uuid(row.get(0)?),
        ).optional()?;
        if current != expected {
            return Err(Error::Conflict(
                "笔记所在文件夹已更改，请刷新后重试。".into(),
            ));
        }
        if current != parent {
            put_placement(&tx, note, parent)?;
        }
        tx.commit()?;
        plugin_record_wake().notify_waiters();
        Ok(())
    }
}

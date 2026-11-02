use super::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_DIRECTORY_DEPTH: i64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorySortOrder {
    NameAsc,
    NameDesc,
}

impl DirectorySortOrder {
    fn as_str(self) -> &'static str {
        match self {
            Self::NameAsc => "name_asc",
            Self::NameDesc => "name_desc",
        }
    }

    fn from_db(value: &str) -> anyhow::Result<Self> {
        match value {
            "name_asc" => Ok(Self::NameAsc),
            "name_desc" => Ok(Self::NameDesc),
            _ => anyhow::bail!("invalid stored directory sort order"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectorySortRule {
    pub order: DirectorySortOrder,
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySortSource {
    pub root_id: i64,
    pub folder_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySortState {
    pub root_id: i64,
    pub folder_id: Option<i64>,
    pub explicit_rule: Option<DirectorySortRule>,
    pub effective_order: DirectorySortOrder,
    pub source: Option<DirectorySortSource>,
    pub revision: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySortMutationResult {
    pub state: DirectorySortState,
    pub invalidate_descendants: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderChildrenPage {
    pub items: Vec<FolderRecord>,
    pub next_cursor: Option<String>,
    pub total_count: Option<i64>,
    pub sort_state: DirectorySortState,
}

#[derive(Clone, Copy)]
pub enum DirectoryTarget {
    Root(i64),
    Folder(i64),
}

fn rule_from_columns(
    order: Option<String>,
    recursive: Option<bool>,
) -> anyhow::Result<Option<DirectorySortRule>> {
    order
        .map(|order| {
            Ok(DirectorySortRule {
                order: DirectorySortOrder::from_db(&order)?,
                recursive: recursive.unwrap_or(false),
            })
        })
        .transpose()
}

impl Database {
    pub fn read_directory_sort_state(
        &self,
        target: DirectoryTarget,
    ) -> anyhow::Result<Option<DirectorySortState>> {
        let transaction = self.connection.unchecked_transaction()?;
        let state = self.directory_sort_state(target)?;
        transaction.commit()?;
        Ok(state)
    }
    pub fn update_root_appearance(
        &self,
        root_id: i64,
        icon: &str,
        color: &str,
    ) -> anyhow::Result<Option<RootRecord>> {
        let transaction = self.connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE roots SET icon=?1, color=?2 WHERE id=?3 AND enabled=1 AND pending_delete_at IS NULL",
            (icon, color, root_id),
        )?;
        let record = if changed == 0 {
            None
        } else {
            self.get_root(root_id)?
        };
        transaction.commit()?;
        Ok(record)
    }

    /// Resolve only the target's bounded ancestry; no counts or catalog-wide rule load.
    pub fn directory_sort_state(
        &self,
        target: DirectoryTarget,
    ) -> anyhow::Result<Option<DirectorySortState>> {
        let mut lineage = Vec::new();
        let mut folder_id = None;
        let root_id = match target {
            DirectoryTarget::Root(id) => id,
            DirectoryTarget::Folder(id) => {
                let mut statement = self.connection.prepare_cached(
                    "WITH RECURSIVE ancestry(id,parent_id,root_id,depth,own_revision,subtree_revision) AS (
                        SELECT id,parent_id,root_id,0,directory_sort_revision,directory_sort_subtree_revision
                        FROM folders WHERE id=?1 AND status='active'
                        UNION ALL
                        SELECT f.id,f.parent_id,f.root_id,a.depth+1,f.directory_sort_revision,f.directory_sort_subtree_revision
                        FROM ancestry a JOIN folders f ON f.id=a.parent_id AND f.root_id=a.root_id
                        WHERE f.status='active' AND a.depth < ?2
                    )
                    SELECT a.id,a.parent_id,a.root_id,a.own_revision,a.subtree_revision,r.sort_order,r.recursive
                    FROM ancestry a LEFT JOIN directory_sort_rules r ON r.folder_id=a.id ORDER BY a.depth"
                )?;
                let rows = statement.query_map((id, MAX_DIRECTORY_DEPTH), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<bool>>(6)?,
                    ))
                })?;
                for row in rows {
                    lineage.push(row?);
                }
                let Some(first) = lineage.first() else {
                    return Ok(None);
                };
                if lineage.last().is_some_and(|entry| entry.1.is_some()) {
                    anyhow::bail!("directory ancestry is incomplete or exceeds depth limit");
                }
                if first.1.is_some() {
                    folder_id = Some(id);
                }
                first.2
            }
        };
        let root = self.connection.query_row(
            "SELECT roots.directory_sort_revision,roots.directory_sort_subtree_revision,r.sort_order,r.recursive
             FROM roots LEFT JOIN directory_sort_rules r ON r.root_id=roots.id
             WHERE roots.id=?1 AND roots.enabled=1 AND roots.pending_delete_at IS NULL",
            [root_id], |row| Ok((row.get::<_, i64>(0)?,row.get::<_, i64>(1)?,row.get::<_, Option<String>>(2)?,row.get::<_, Option<bool>>(3)?)),
        ).optional()?;
        let Some((root_revision, root_subtree_revision, order, recursive)) = root else {
            return Ok(None);
        };
        let root_rule = rule_from_columns(order, recursive)?;
        let mut fingerprint = Sha256::new();
        fingerprint.update(format!(
            "v1:root:{root_id}:{};",
            if folder_id.is_none() {
                root_revision
            } else {
                root_subtree_revision
            }
        ));
        let mut explicit_rule = None;
        let mut effective = None;
        for (depth, (id, parent, _, own_revision, subtree_revision, order, recursive)) in
            lineage.into_iter().enumerate()
        {
            // Sentinel preferences belong to Root, not the sentinel row.
            if parent.is_none() {
                continue;
            }
            fingerprint.update(format!(
                "folder:{id}:{};",
                if depth == 0 {
                    own_revision
                } else {
                    subtree_revision
                }
            ));
            let rule = rule_from_columns(order, recursive)?;
            if depth == 0 {
                explicit_rule = rule.clone();
            }
            if effective.is_none() {
                if let Some(rule) = rule.filter(|rule| depth == 0 || rule.recursive) {
                    effective = Some((
                        rule.order,
                        DirectorySortSource {
                            root_id,
                            folder_id: Some(id),
                        },
                    ));
                }
            }
        }
        if folder_id.is_none() {
            explicit_rule = root_rule.clone();
        }
        if effective.is_none() {
            if let Some(rule) = root_rule.filter(|rule| folder_id.is_none() || rule.recursive) {
                effective = Some((
                    rule.order,
                    DirectorySortSource {
                        root_id,
                        folder_id: None,
                    },
                ));
            }
        }
        let (effective_order, source) = effective
            .map(|(order, source)| (order, Some(source)))
            .unwrap_or((DirectorySortOrder::NameAsc, None));
        Ok(Some(DirectorySortState {
            root_id,
            folder_id,
            explicit_rule,
            effective_order,
            source,
            revision: hex_encode(&fingerprint.finalize()),
        }))
    }

    /// One rule write plus target counters, independent of descendant count.
    pub fn update_directory_sort(
        &self,
        target: DirectoryTarget,
        rule: Option<DirectorySortRule>,
    ) -> anyhow::Result<Option<DirectorySortMutationResult>> {
        // Reserve the writer before reading the previous rule. A deferred WAL
        // snapshot cannot safely upgrade after a concurrent startup/fsops write.
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let Some(old) = self.directory_sort_state(target)? else {
            return Ok(None);
        };
        if matches!(target, DirectoryTarget::Folder(_)) && old.folder_id.is_none() {
            return Ok(None);
        }
        if old.explicit_rule == rule {
            transaction.commit()?;
            return Ok(Some(DirectorySortMutationResult {
                state: old,
                invalidate_descendants: false,
            }));
        }
        let invalidate_descendants = old.explicit_rule.as_ref().is_some_and(|r| r.recursive)
            || rule.as_ref().is_some_and(|r| r.recursive);
        let (table, column, id) = match target {
            DirectoryTarget::Root(id) => ("roots", "root_id", id),
            DirectoryTarget::Folder(id) => ("folders", "folder_id", id),
        };
        if let Some(rule) = rule {
            transaction.execute(&format!(
                "INSERT INTO directory_sort_rules({column},sort_order,recursive) VALUES(?1,?2,?3)
                 ON CONFLICT({column}) DO UPDATE SET sort_order=excluded.sort_order,recursive=excluded.recursive"
            ), (id, rule.order.as_str(), rule.recursive))?;
        } else {
            transaction.execute(
                &format!("DELETE FROM directory_sort_rules WHERE {column}=?1"),
                [id],
            )?;
        }
        transaction.execute(
            &format!(
                "UPDATE {table} SET directory_sort_revision=directory_sort_revision+1,
             directory_sort_subtree_revision=directory_sort_subtree_revision+?2 WHERE id=?1"
            ),
            (id, i64::from(invalidate_descendants)),
        )?;
        let state = self
            .directory_sort_state(target)?
            .context("directory vanished during preference write")?;
        transaction.commit()?;
        Ok(Some(DirectorySortMutationResult {
            state,
            invalidate_descendants,
        }))
    }

    /// One page-seeded bulk traversal. Unrelated siblings are never counted.
    fn page_folder_counts(&self, ids: &[i64]) -> anyhow::Result<HashMap<i64, (i64, i64)>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let sql = page_folder_counts_sql(ids.len());
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(ids), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let counts = rows
            .map(|row| row.map(|(id, media, files)| (id, (media, files))))
            .collect::<Result<_, _>>()
            .map_err(anyhow::Error::from)?;
        #[cfg(test)]
        trace_directory_statement(&statement, "page-counts");
        Ok(counts)
    }

    pub fn list_folder_children_page(
        &self,
        folder_id: i64,
        limit: i64,
        cursor: Option<String>,
        include_descendants: bool,
    ) -> anyhow::Result<FolderChildrenPage> {
        // Keep the effective rule, page and exact counts in one WAL read snapshot.
        let transaction = self.connection.unchecked_transaction()?;
        let sort_state = self
            .directory_sort_state(DirectoryTarget::Folder(folder_id))?
            .context("folder not found")?;
        let limit = limit.clamp(1, 500);
        let cursor_prefix = format!(
            "v2:folder:{folder_id}:{}:{}:{}:",
            u8::from(include_descendants),
            sort_state.effective_order.as_str(),
            sort_state.revision
        );
        let cursor = cursor
            .as_deref()
            .map(|cursor| decode_directory_cursor(cursor, &cursor_prefix))
            .transpose()?;
        let sql = folder_children_sql(
            sort_state.effective_order,
            cursor.is_some(),
            include_descendants,
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            rusqlite::params![
                folder_id,
                cursor.as_ref().map(|c| c.name.as_str()),
                cursor.as_ref().map(|c| c.id),
                limit + 1
            ],
            |row| {
                Ok(FolderRecord {
                    id: row.get(0)?,
                    root_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    name: row.get(3)?,
                    status: row.get(4)?,
                    has_children: row.get(5)?,
                    icon: row.get(6)?,
                    color: row.get(7)?,
                    media_count: 0,
                    file_count: 0,
                })
            },
        )?;
        let mut folders = rows.collect::<Result<Vec<_>, _>>()?;
        #[cfg(test)]
        trace_directory_statement(&statement, "page-items");
        drop(statement);
        let next_cursor = if folders.len() > limit as usize {
            folders.pop();
            folders.last().map(|folder| {
                format!(
                    "{cursor_prefix}{}:{}",
                    folder.id,
                    hex_encode(folder.name.as_bytes())
                )
            })
        } else {
            None
        };
        let counts = self.page_folder_counts(&folders.iter().map(|f| f.id).collect::<Vec<_>>())?;
        for folder in &mut folders {
            (folder.media_count, folder.file_count) =
                counts.get(&folder.id).copied().unwrap_or((0, 0));
        }
        transaction.commit()?;
        Ok(FolderChildrenPage {
            items: folders,
            next_cursor,
            total_count: None,
            sort_state,
        })
    }
}

pub(super) fn page_folder_counts_sql(count: usize) -> String {
    let seeds = std::iter::repeat_n("(?)", count)
        .collect::<Vec<_>>()
        .join(",");
    format!("WITH RECURSIVE page_roots(id) AS (VALUES {seeds}), scope(owner,id) AS (
        SELECT id,id FROM page_roots
        UNION ALL SELECT scope.owner, child.id FROM scope JOIN folders child ON child.parent_id=scope.id AND child.status='active'
    )
    SELECT scope.owner,COALESCE(SUM(files.browse_visible),0),COUNT(files.id)
    FROM scope LEFT JOIN files ON files.folder_id=scope.id AND files.status='active'
    GROUP BY scope.owner")
}

pub(super) fn folder_children_sql(
    order: DirectorySortOrder,
    has_cursor: bool,
    descendants: bool,
) -> String {
    let direction = if order == DirectorySortOrder::NameDesc {
        "DESC"
    } else {
        "ASC"
    };
    let comparison = if order == DirectorySortOrder::NameDesc {
        "<"
    } else {
        ">"
    };
    let cursor_clause = if has_cursor {
        format!("AND (folders.name, folders.id) {comparison} (?2, ?3)")
    } else {
        String::new()
    };
    let (cte, join, parent) = if descendants {
        ("WITH RECURSIVE descendants(id) AS (SELECT id FROM folders WHERE parent_id=?1 AND status='active'
           UNION ALL SELECT f.id FROM descendants d JOIN folders f ON f.parent_id=d.id AND f.status='active')", "JOIN descendants d ON d.id=folders.id", "")
    } else {
        ("", "", "AND folders.parent_id=?1")
    };
    format!("{cte}
        SELECT folders.id,folders.root_id,folders.parent_id,folders.name,folders.status,
          EXISTS(SELECT 1 FROM folders child WHERE child.parent_id=folders.id AND child.status='active'),folders.icon,folders.color
        FROM folders {join} JOIN roots ON roots.id=folders.root_id AND roots.enabled=1 AND roots.pending_delete_at IS NULL
        WHERE folders.status='active' {parent} {cursor_clause}
        ORDER BY folders.name {direction}, folders.id {direction} LIMIT ?4")
}

fn decode_directory_cursor(cursor: &str, prefix: &str) -> anyhow::Result<FolderCursor> {
    let suffix = cursor
        .strip_prefix(prefix)
        .context("invalid folder cursor: parent or sort revision changed")?;
    let (id, name) = suffix.split_once(':').context("invalid folder cursor")?;
    let id = id.parse::<i64>().context("invalid folder cursor")?;
    let name = hex_decode_to_string(name).map_err(|_| anyhow::anyhow!("invalid folder cursor"))?;
    Ok(FolderCursor { name, id })
}

#[cfg(test)]
fn trace_directory_statement(statement: &rusqlite::Statement<'_>, phase: &str) {
    if std::env::var_os("MANGOO_DIRECTORY_BENCH_TRACE").is_some() {
        use rusqlite::StatementStatus;
        println!(
            "directory-statement phase={phase} vm_steps={} fullscan_steps={} sorts={}",
            statement.get_status(StatementStatus::VmStep),
            statement.get_status(StatementStatus::FullscanStep),
            statement.get_status(StatementStatus::Sort)
        );
    }
}

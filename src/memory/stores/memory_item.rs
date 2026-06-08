use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value as JsonValue;

use crate::core::types::{MemoryItem, MemoryType};
use crate::memory::stores::traits::MemoryStore;
use crate::prelude::{XueliError, XueliResult};

pub struct SqliteMemoryItemStore {
    conn: Arc<Mutex<Connection>>,
    _db_path: PathBuf,
}

const SCHEMA_VERSION: i32 = 2;

const INIT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS memory_items (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    content         TEXT NOT NULL,
    memory_type     TEXT NOT NULL,
    importance      REAL NOT NULL DEFAULT 0.0,
    created_at      TEXT NOT NULL,
    last_accessed_at TEXT NOT NULL,
    access_count    INTEGER NOT NULL DEFAULT 0,
    is_archived     INTEGER NOT NULL DEFAULT 0,
    half_life_hours REAL NOT NULL DEFAULT 720.0,
    archived_at     TEXT,
    consolidation_version INTEGER NOT NULL DEFAULT 0,
    consolidated_at TEXT,
    half_life_modifier REAL,
    suppressed_at   TEXT,
    merge_status    TEXT,
    merged_into_id  TEXT,
    metadata_json   TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_mi_user_id
    ON memory_items(user_id);

CREATE INDEX IF NOT EXISTS idx_mi_type
    ON memory_items(memory_type);

CREATE INDEX IF NOT EXISTS idx_mi_importance
    ON memory_items(importance DESC);

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

fn memory_type_to_str(mt: &MemoryType) -> &'static str {
    match mt {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Event => "event",
        MemoryType::Opinion => "opinion",
        MemoryType::Relationship => "relationship",
    }
}

fn str_to_memory_type(s: &str) -> MemoryType {
    match s {
        "fact" => MemoryType::Fact,
        "preference" => MemoryType::Preference,
        "event" => MemoryType::Event,
        "opinion" => MemoryType::Opinion,
        "relationship" => MemoryType::Relationship,
        _ => MemoryType::Fact,
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct MemoryItemRecord {
    id: String,
    user_id: String,
    content: String,
    memory_type_str: String,
    importance: f64,
    created_at_str: String,
    last_accessed_at_str: String,
    access_count: i64,
    is_archived: bool,
    half_life_hours: f64,
    archived_at: Option<String>,
    consolidation_version: i32,
    consolidated_at: Option<String>,
    half_life_modifier: Option<f64>,
    suppressed_at: Option<String>,
    merge_status: Option<String>,
    merged_into_id: Option<String>,
    metadata_json: String,
}

fn row_to_memory_item(row: &rusqlite::Row) -> rusqlite::Result<MemoryItem> {
    let created_at: String = row.get(5)?;
    let last_accessed: String = row.get(6)?;
    let mem_type: String = row.get(3)?;

    Ok(MemoryItem {
        id: row.get(0)?,
        user_id: row.get(1)?,
        content: row.get(2)?,
        memory_type: str_to_memory_type(&mem_type),
        importance: row.get(4)?,
        created_at: created_at.parse().unwrap_or_default(),
        last_accessed_at: last_accessed.parse().unwrap_or_default(),
        access_count: row.get(7)?,
    })
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<MemoryItemRecord> {
    Ok(MemoryItemRecord {
        id: row.get(0)?,
        user_id: row.get(1)?,
        content: row.get(2)?,
        memory_type_str: row.get(3)?,
        importance: row.get(4)?,
        created_at_str: row.get(5)?,
        last_accessed_at_str: row.get(6)?,
        access_count: row.get(7)?,
        is_archived: row.get::<_, i32>(8)? != 0,
        half_life_hours: row.get(9)?,
        archived_at: row.get(10)?,
        consolidation_version: row.get(11)?,
        consolidated_at: row.get(12)?,
        half_life_modifier: row.get(13)?,
        suppressed_at: row.get(14)?,
        merge_status: row.get(15)?,
        merged_into_id: row.get(16)?,
        metadata_json: row.get(17)?,
    })
}

fn parse_ts_opt(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_ref().and_then(|v| v.parse().ok())
}

fn calc_effective_importance(rec: &MemoryItemRecord, now: DateTime<Utc>) -> f64 {
    let base_half_life = rec.half_life_hours;
    let modifier = rec.half_life_modifier.unwrap_or(1.0);
    let effective_half_life = (base_half_life * modifier).max(0.1);

    let ref_dt = rec
        .last_accessed_at_str
        .parse::<DateTime<Utc>>()
        .ok()
        .or_else(|| rec.created_at_str.parse::<DateTime<Utc>>().ok())
        .unwrap_or(now);
    let elapsed_hours = (now - ref_dt).num_hours().max(0) as f64;

    let decay_factor = 0.5_f64.powf(elapsed_hours / effective_half_life);

    let effective = (rec.importance * decay_factor).max(0.0);

    if rec.is_archived {
        if let Some(archived_at) = parse_ts_opt(&rec.archived_at) {
            let archived_hours = (now - archived_at).num_hours().max(0) as f64;
            if archived_hours > 2160.0 {
                return effective * 0.5_f64.powf(archived_hours / (effective_half_life * 1.5));
            }
        }
    }

    effective
}

impl SqliteMemoryItemStore {
    pub fn new(db_dir: &std::path::Path) -> XueliResult<Self> {
        std::fs::create_dir_all(db_dir).map_err(|e| format!("无法创建目录: {e}"))?;
        let db_path = db_dir.join("memory.db");

        let conn =
            Connection::open(&db_path).map_err(|e| format!("无法打开数据库 {db_path:?}: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch(INIT_SCHEMA)
            .map_err(|e| format!("建表失败: {e}"))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            _db_path: db_path,
        };

        if let Err(e) = store.migrate_schema_if_needed_sync() {
            tracing::warn!("schema 迁移失败（可能已是最新版本）: {e}");
        }

        Ok(store)
    }

    fn migrate_schema_if_needed_sync(&self) -> XueliResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;

        let current_version: i32 = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key='schema_version'",
                [],
                |row| row.get::<_, String>(0).map(|s| s.parse().unwrap_or(0)),
            )
            .unwrap_or(1);

        if current_version >= SCHEMA_VERSION {
            return Ok(());
        }

        let migrations: Vec<&str> = vec![
            "ALTER TABLE memory_items ADD COLUMN is_archived INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE memory_items ADD COLUMN half_life_hours REAL NOT NULL DEFAULT 720.0",
            "ALTER TABLE memory_items ADD COLUMN archived_at TEXT",
            "ALTER TABLE memory_items ADD COLUMN consolidation_version INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE memory_items ADD COLUMN consolidated_at TEXT",
            "ALTER TABLE memory_items ADD COLUMN half_life_modifier REAL",
            "ALTER TABLE memory_items ADD COLUMN suppressed_at TEXT",
            "ALTER TABLE memory_items ADD COLUMN merge_status TEXT",
            "ALTER TABLE memory_items ADD COLUMN merged_into_id TEXT",
            "ALTER TABLE memory_items ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}'",
        ];

        for sql in migrations {
            if let Err(e) = conn.execute(sql, []) {
                if !e.to_string().contains("duplicate column") {
                    return Err(XueliError::Database(format!("迁移失败: {e}")));
                }
            }
        }

        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )
        .map_err(|e| XueliError::Database(format!("schema_meta 写入失败: {e}")))?;

        Ok(())
    }

    pub fn migrate_schema_if_needed(&self) -> XueliResult<()> {
        self.migrate_schema_if_needed_sync()
    }

    fn _read_record(&self, mem_id: &str) -> XueliResult<Option<MemoryItemRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at,
                        access_count, is_archived, half_life_hours, archived_at, consolidation_version,
                        consolidated_at, half_life_modifier, suppressed_at, merge_status, merged_into_id, metadata_json
                 FROM memory_items WHERE id = ?1",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        stmt.query_row(params![mem_id], row_to_record)
            .optional()
            .map_err(|e| XueliError::Database(format!("查询失败: {e}")))
    }

    pub async fn get_by_id(&self, id: &str) -> XueliResult<Option<MemoryItem>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE id = ?1",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        let result = stmt
            .query_row(params![id], row_to_memory_item)
            .optional()
            .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

        Ok(result)
    }

    pub async fn get_effective_importance(&self, mem_id: &str) -> XueliResult<f64> {
        let mem_id = mem_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<f64> {
            let conn = conn.lock().map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at,
                            access_count, is_archived, half_life_hours, archived_at, consolidation_version,
                            consolidated_at, half_life_modifier, suppressed_at, merge_status, merged_into_id, metadata_json
                     FROM memory_items WHERE id = ?1",
                )
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let rec = stmt
                .query_row(params![mem_id], row_to_record)
                .optional()
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

            match rec {
                Some(r) => Ok(calc_effective_importance(&r, Utc::now())),
                None => Err(XueliError::Database(format!("记忆不存在: {mem_id}"))),
            }
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn refresh_archive_status(&self, user_id: &str) -> XueliResult<usize> {
        let user_id = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<usize> {
            let conn = conn.lock().map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at,
                            access_count, is_archived, half_life_hours, archived_at, consolidation_version,
                            consolidated_at, half_life_modifier, suppressed_at, merge_status, merged_into_id, metadata_json
                     FROM memory_items WHERE user_id = ?1",
                )
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let records: Vec<MemoryItemRecord> = stmt
                .query_map(params![user_id], row_to_record)
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            let now = Utc::now();
            let threshold = 0.1;
            let mut archived_count = 0usize;

            for rec in &records {
                let eff = calc_effective_importance(rec, now);
                let should_archive = eff < threshold;
                let currently_archived = rec.is_archived;

                if should_archive && !currently_archived {
                    conn.execute(
                        "UPDATE memory_items SET is_archived=1, archived_at=?1 WHERE id=?2",
                        params![now.to_rfc3339(), rec.id],
                    )
                    .map_err(|e| XueliError::Database(format!("归档更新失败: {e}")))?;
                    archived_count += 1;
                } else if !should_archive && currently_archived {
                    conn.execute(
                        "UPDATE memory_items SET is_archived=0, archived_at=NULL WHERE id=?1",
                        params![rec.id],
                    )
                    .map_err(|e| XueliError::Database(format!("取消归档更新失败: {e}")))?;
                }
            }

            Ok(archived_count)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn should_forget(&self, mem_id: &str) -> XueliResult<bool> {
        let mem_id = mem_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<bool> {
            let conn = conn.lock().map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at,
                            access_count, is_archived, half_life_hours, archived_at, consolidation_version,
                            consolidated_at, half_life_modifier, suppressed_at, merge_status, merged_into_id, metadata_json
                     FROM memory_items WHERE id = ?1",
                )
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let rec = stmt
                .query_row(params![mem_id], row_to_record)
                .optional()
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

            match rec {
                Some(r) => {
                    let now = Utc::now();
                    let eff = calc_effective_importance(&r, now);
                    if eff < 0.05 {
                        return Ok(true);
                    }
                    if r.is_archived {
                        if let Some(archived_at) = parse_ts_opt(&r.archived_at) {
                            let days_archived = (now - archived_at).num_days();
                            if days_archived > 90 {
                                return Ok(true);
                            }
                        }
                    }
                    Ok(false)
                }
                None => Ok(false),
            }
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn update_metadata(&self, mem_id: &str, key: &str, value: &str) -> XueliResult<()> {
        let mem_id = mem_id.to_string();
        let key = key.to_string();
        let value = value.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<()> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare("SELECT id, metadata_json FROM memory_items WHERE id = ?1")
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let (mem_id_existing, meta_str): (String, String) = stmt
                .query_row(params![mem_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

            let mut meta: JsonValue =
                serde_json::from_str(&meta_str).unwrap_or(JsonValue::Object(Default::default()));
            if let JsonValue::Object(ref mut map) = meta {
                map.insert(key, JsonValue::String(value));
            }
            let new_meta = serde_json::to_string(&meta)
                .map_err(|e| XueliError::Serialization(format!("JSON 序列化失败: {e}")))?;

            conn.execute(
                "UPDATE memory_items SET metadata_json=?1 WHERE id=?2",
                params![new_meta, mem_id_existing],
            )
            .map_err(|e| XueliError::Database(format!("元数据更新失败: {e}")))?;

            Ok::<(), XueliError>(())
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))??;

        Ok(())
    }

    pub async fn get_all_user_ids(&self) -> XueliResult<Vec<String>> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Vec<String>> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare("SELECT DISTINCT user_id FROM memory_items")
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(ids)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn suppress_competitors(
        &self,
        user_id: &str,
        selected_ids: &[String],
        penalty: f64,
    ) -> XueliResult<usize> {
        let user_id = user_id.to_string();
        let selected_set: std::collections::HashSet<String> =
            selected_ids.iter().cloned().collect();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<usize> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare("SELECT id, importance, is_archived FROM memory_items WHERE user_id = ?1")
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let items: Vec<(String, f64, bool)> = stmt
                .query_map(params![user_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, i32>(2)? != 0,
                    ))
                })
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            let mut suppressed = 0usize;
            let penalty_clamped = penalty.clamp(0.0, 1.0);

            for (mem_id, importance, is_archived) in &items {
                if selected_set.contains(mem_id) || *is_archived {
                    continue;
                }
                let new_importance = (importance * (1.0 - penalty_clamped)).max(0.0);
                conn.execute(
                    "UPDATE memory_items SET importance=?1, suppressed_at=?2 WHERE id=?3",
                    params![new_importance, Utc::now().to_rfc3339(), mem_id],
                )
                .map_err(|e| XueliError::Database(format!("抑制更新失败: {e}")))?;
                suppressed += 1;
            }

            Ok(suppressed)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }
}

#[async_trait]
impl MemoryStore for SqliteMemoryItemStore {
    async fn store(&self, item: MemoryItem) -> XueliResult<String> {
        let id = item.id.clone();
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;

        conn.execute(
            "INSERT OR REPLACE INTO memory_items
             (id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item.id,
                item.user_id,
                item.content,
                memory_type_to_str(&item.memory_type),
                item.importance,
                item.created_at.to_rfc3339(),
                item.last_accessed_at.to_rfc3339(),
                item.access_count as i64,
            ],
        )
        .map_err(|e| XueliError::Database(format!("插入失败: {e}")))?;

        Ok(id)
    }

    async fn store_batch(&self, items: Vec<MemoryItem>) -> XueliResult<Vec<String>> {
        let mut ids = Vec::with_capacity(items.len());
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| XueliError::Database(format!("事务失败: {e}")))?;

        for item in &items {
            ids.push(item.id.clone());
            tx.execute(
                "INSERT OR REPLACE INTO memory_items
                 (id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    item.id,
                    item.user_id,
                    item.content,
                    memory_type_to_str(&item.memory_type),
                    item.importance,
                    item.created_at.to_rfc3339(),
                    item.last_accessed_at.to_rfc3339(),
                    item.access_count as i64,
                ],
            )
            .map_err(|e| XueliError::Database(format!("批量插入失败: {e}")))?;
        }

        tx.commit()
            .map_err(|e| XueliError::Database(format!("提交事务失败: {e}")))?;
        Ok(ids)
    }

    async fn get(&self, id: &str) -> XueliResult<Option<MemoryItem>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE id = ?1",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        let result = stmt
            .query_row(params![id], row_to_memory_item)
            .optional()
            .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

        Ok(result)
    }

    async fn get_by_user(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE user_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        let rows = stmt
            .query_map(params![user_id], row_to_memory_item)
            .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| XueliError::Database(format!("读取行失败: {e}")))?);
        }
        Ok(items)
    }

    async fn update(&self, item: MemoryItem) -> XueliResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let affected = conn
            .execute(
                "UPDATE memory_items SET
                    content = ?2,
                    memory_type = ?3,
                    importance = ?4,
                    last_accessed_at = ?5,
                    access_count = ?6
                 WHERE id = ?1",
                params![
                    item.id,
                    item.content,
                    memory_type_to_str(&item.memory_type),
                    item.importance,
                    item.last_accessed_at.to_rfc3339(),
                    item.access_count as i64,
                ],
            )
            .map_err(|e| XueliError::Database(format!("更新失败: {e}")))?;

        if affected == 0 {
            return Err(XueliError::Database(format!("未找到记忆: {}", item.id)));
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> XueliResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        conn.execute("DELETE FROM memory_items WHERE id = ?1", params![id])
            .map_err(|e| XueliError::Database(format!("删除失败: {e}")))?;
        Ok(())
    }

    async fn search(&self, query: &str, limit: usize) -> XueliResult<Vec<MemoryItem>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE content LIKE ?1 ORDER BY importance DESC LIMIT ?2",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        let rows = stmt
            .query_map(params![pattern, limit as i64], row_to_memory_item)
            .map_err(|e| XueliError::Database(format!("搜索失败: {e}")))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| XueliError::Database(format!("读取行失败: {e}")))?);
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_item(id: &str, user_id: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance: 0.5,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        let item = make_item("mem1", "user1", "用户喜欢喝咖啡");
        let id = store.store(item).await.unwrap();
        assert_eq!(id, "mem1");

        let fetched = store.get("mem1").await.unwrap().unwrap();
        assert_eq!(fetched.content, "用户喜欢喝咖啡");
        assert_eq!(fetched.user_id, "user1");
    }

    #[tokio::test]
    async fn test_get_by_user() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store.store(make_item("mem1", "u1", "事实A")).await.unwrap();
        store.store(make_item("mem2", "u2", "事实B")).await.unwrap();
        store.store(make_item("mem3", "u1", "事实C")).await.unwrap();

        let items = store.get_by_user("u1").await.unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn test_search() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(make_item("m1", "u1", "喜欢吃辣的食物"))
            .await
            .unwrap();
        store
            .store(make_item("m2", "u1", "喜欢看电影"))
            .await
            .unwrap();

        let results = store.search("食物", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("食物"));
    }

    #[tokio::test]
    async fn test_update_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(make_item("m1", "u1", "原始内容"))
            .await
            .unwrap();

        let mut updated = store.get("m1").await.unwrap().unwrap();
        updated.content = "更新后内容".to_string();
        store.update(updated).await.unwrap();

        let fetched = store.get("m1").await.unwrap().unwrap();
        assert_eq!(fetched.content, "更新后内容");

        store.delete("m1").await.unwrap();
        assert!(store.get("m1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_store_batch() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        let items = vec![make_item("b1", "u1", "A"), make_item("b2", "u1", "B")];
        let ids = store.store_batch(items).await.unwrap();
        assert_eq!(ids.len(), 2);

        let all = store.get_by_user("u1").await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_get_effective_importance_recent() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(MemoryItem {
                id: "d1".into(),
                user_id: "u1".into(),
                content: "测试衰减记忆".into(),
                memory_type: MemoryType::Fact,
                importance: 0.8,
                created_at: Utc::now(),
                last_accessed_at: Utc::now(),
                access_count: 1,
            })
            .await
            .unwrap();

        let eff = store.get_effective_importance("d1").await.unwrap();
        assert!(eff > 0.7, "新记忆有效重要度应接近原始值，实际: {eff}");
    }

    #[tokio::test]
    async fn test_should_forget_not_forget_recent() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(MemoryItem {
                id: "d2".into(),
                user_id: "u1".into(),
                content: "不应遗忘的记忆".into(),
                memory_type: MemoryType::Fact,
                importance: 0.9,
                created_at: Utc::now(),
                last_accessed_at: Utc::now(),
                access_count: 3,
            })
            .await
            .unwrap();

        assert!(!store.should_forget("d2").await.unwrap());
    }

    #[tokio::test]
    async fn test_should_forget_low_importance() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        let old_time = Utc::now() - chrono::TimeDelta::hours(2000);

        store
            .store(MemoryItem {
                id: "d3".into(),
                user_id: "u1".into(),
                content: "极低重要度的旧记忆".into(),
                memory_type: MemoryType::Event,
                importance: 0.01,
                created_at: old_time,
                last_accessed_at: old_time,
                access_count: 0,
            })
            .await
            .unwrap();

        assert!(store.should_forget("d3").await.unwrap());
    }

    #[tokio::test]
    async fn test_refresh_archive_status() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        let old_time = Utc::now() - chrono::TimeDelta::hours(1000);

        store
            .store(MemoryItem {
                id: "ar1".into(),
                user_id: "ua1".into(),
                content: "很久之前的记忆".into(),
                memory_type: MemoryType::Event,
                importance: 0.05,
                created_at: old_time,
                last_accessed_at: old_time,
                access_count: 0,
            })
            .await
            .unwrap();

        let count = store.refresh_archive_status("ua1").await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_suppress_competitors() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(MemoryItem {
                id: "sel1".into(),
                user_id: "us1".into(),
                content: "选中的记忆".into(),
                memory_type: MemoryType::Fact,
                importance: 0.8,
                created_at: Utc::now(),
                last_accessed_at: Utc::now(),
                access_count: 1,
            })
            .await
            .unwrap();
        store
            .store(MemoryItem {
                id: "unsel1".into(),
                user_id: "us1".into(),
                content: "未选中的记忆".into(),
                memory_type: MemoryType::Fact,
                importance: 0.6,
                created_at: Utc::now(),
                last_accessed_at: Utc::now(),
                access_count: 1,
            })
            .await
            .unwrap();

        let count = store
            .suppress_competitors("us1", &["sel1".to_string()], 0.3)
            .await
            .unwrap();
        assert_eq!(count, 1);

        let suppressed = store.get("unsel1").await.unwrap().unwrap();
        assert!(suppressed.importance < 0.5, "importance 应被抑制");
    }

    #[tokio::test]
    async fn test_update_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(make_item("meta1", "u1", "元数据测试"))
            .await
            .unwrap();

        store
            .update_metadata("meta1", "test_key", "test_value")
            .await
            .unwrap();

        let r = store._read_record("meta1").unwrap().unwrap();
        let meta: JsonValue = serde_json::from_str(&r.metadata_json).unwrap();
        assert_eq!(meta["test_key"], "test_value");
    }

    #[tokio::test]
    async fn test_migrate_schema() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();
        store.migrate_schema_if_needed().unwrap();

        store
            .store(make_item("mig1", "u1", "迁移测试"))
            .await
            .unwrap();

        let exists: bool = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT half_life_hours FROM memory_items WHERE id='mig1'",
                [],
                |row| row.get::<_, f64>(0),
            )
            .is_ok()
        };
        assert!(exists);
    }
}

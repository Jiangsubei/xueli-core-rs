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

/// 类别半衰期修饰符（对应 Python 版 category_half_life_modifiers）
const CATEGORY_HALF_LIFE_MODIFIERS: &[(&str, f64)] = &[
    ("core_fact", 3.0),
    ("important", 1.5),
    ("casual", 0.7),
];

/// 类别遗忘阈值（对应 Python 版 _get_forget_threshold）
const CATEGORY_FORGET_THRESHOLDS: &[(&str, f64)] = &[
    ("core_fact", 0.05),
    ("casual", 0.8),
];

/// 抑制因子（对应 Python 版 suppression_factor）
const SUPPRESSION_FACTOR: f64 = 0.05;

fn category_half_life_modifier(category: &str) -> f64 {
    CATEGORY_HALF_LIFE_MODIFIERS
        .iter()
        .find(|(k, _)| *k == category)
        .map(|(_, v)| *v)
        .unwrap_or(1.0)
}

fn category_forget_threshold(category: &str, default: f64) -> f64 {
    CATEGORY_FORGET_THRESHOLDS
        .iter()
        .find(|(k, _)| *k == category)
        .map(|(_, v)| *v)
        .unwrap_or(default)
}

/// 从 metadata_json 中提取字段值
fn meta_get_f64(meta: &serde_json::Value, key: &str) -> Option<f64> {
    meta.get(key).and_then(|v| v.as_f64())
}

fn meta_get_i64(meta: &serde_json::Value, key: &str) -> Option<i64> {
    meta.get(key).and_then(|v| v.as_i64())
}

fn meta_get_str<'a>(meta: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    meta.get(key).and_then(|v| v.as_str())
}

fn meta_get_bool(meta: &serde_json::Value, key: &str) -> bool {
    meta.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 解析 metadata_json
fn parse_metadata(json_str: &str) -> serde_json::Value {
    serde_json::from_str(json_str).unwrap_or(JsonValue::Object(Default::default()))
}

/// 计算保留奖励（对应 Python 版 _get_retention_bonus）
fn calc_retention_bonus(meta: &serde_json::Value, age_days: f64) -> f64 {
    let mention_count = meta_get_i64(meta, "mention_count").unwrap_or(1).max(1) as f64;
    let mention_bonus = ((mention_count - 1.0).max(0.0) * 0.35).min(1.2);

    let observation_count = meta
        .get("source_observations")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter(|item| item.is_object()).count() as f64)
        .unwrap_or(0.0);
    let observation_bonus = ((observation_count - 1.0).max(0.0) * 0.2).min(0.8);

    let recency_bonus = if age_days <= 7.0 {
        0.6
    } else if age_days <= 21.0 {
        0.35
    } else if age_days <= 45.0 {
        0.15
    } else {
        0.0
    };

    let emotional_bonus = if meta_get_str(meta, "emotional_tone")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        0.2
    } else {
        0.0
    };

    mention_bonus + observation_bonus + recency_bonus + emotional_bonus
}

fn calc_effective_importance(rec: &MemoryItemRecord, now: DateTime<Utc>) -> f64 {
    let meta = parse_metadata(&rec.metadata_json);

    // 衰减未启用或非 ordinary 类型时返回基础重要度
    let memory_kind = meta_get_str(&meta, "memory_type").unwrap_or("legacy").to_lowercase();
    if memory_kind != "ordinary" && memory_kind != "legacy" {
        // 对于 Rust 版本，所有记忆都走衰减逻辑（因为 Rust 版不区分 memory_type 列）
    }

    // decay_exempt 检查
    if meta_get_bool(&meta, "decay_exempt") {
        return rec.importance.max(0.0);
    }

    let base_half_life = rec.half_life_hours;
    let consolidation_modifier = meta_get_f64(&meta, "consolidated_half_life_modifier").unwrap_or(1.0);

    // 类别修饰符
    let category = meta_get_str(&meta, "memory_category")
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let category_mod = category_half_life_modifier(&category);

    let effective_half_life = (base_half_life * category_mod * consolidation_modifier).max(0.1);

    let ref_dt = rec
        .last_accessed_at_str
        .parse::<DateTime<Utc>>()
        .ok()
        .or_else(|| {
            meta_get_str(&meta, "last_reinforced_at")
                .and_then(|s| s.parse().ok())
                .or_else(|| {
                    meta_get_str(&meta, "last_recalled_at").and_then(|s| s.parse().ok())
                })
        })
        .or_else(|| rec.created_at_str.parse::<DateTime<Utc>>().ok())
        .unwrap_or(now);

    let age_days = (now - ref_dt).num_seconds() as f64 / 86400.0;
    let age_days = age_days.max(0.0);

    let base = rec.importance.max(0.0);

    // 巩固期内未巩固的记忆保持高重要度
    let consolidation_hours = rec.half_life_hours; // 使用 half_life_hours 作为近似
    if consolidation_hours > 0.0 && age_days <= consolidation_hours / 24.0 {
        if meta_get_i64(&meta, "consolidation_version").unwrap_or(0) < 1 {
            return base.min(5.0);
        }
    }

    let decay_factor = 0.5_f64.powf(age_days / effective_half_life);

    // 冷记忆额外衰减
    let cold_threshold_days = 90.0;
    let cold_multiplier = 1.5;
    let decay_factor = if age_days > cold_threshold_days {
        let cold_age = age_days - cold_threshold_days;
        decay_factor * 0.5_f64.powf(cold_age / (effective_half_life / cold_multiplier))
    } else {
        decay_factor
    };

    let retention_bonus = calc_retention_bonus(&meta, age_days);
    let mut effective = (base * decay_factor + retention_bonus).min(5.0);

    // 抑制计数衰减
    let suppression_count = meta_get_i64(&meta, "suppression_count").unwrap_or(0);
    if suppression_count > 0 {
        effective *= 1.0 - suppression_count as f64 * SUPPRESSION_FACTOR;
    }

    effective.max(0.0)
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
                .prepare("SELECT id, importance, is_archived, metadata_json FROM memory_items WHERE user_id = ?1")
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let items: Vec<(String, f64, bool, String)> = stmt
                .query_map(params![user_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, i32>(2)? != 0,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            let mut suppressed = 0usize;
            let penalty_clamped = penalty.clamp(0.0, 1.0);
            let now = Utc::now().to_rfc3339();

            for (mem_id, importance, is_archived, meta_json) in &items {
                if selected_set.contains(mem_id) || *is_archived {
                    continue;
                }
                let new_importance = (importance * (1.0 - penalty_clamped)).max(0.0);

                // 更新 suppression_count in metadata
                let mut meta = parse_metadata(meta_json);
                let current_count = meta_get_i64(&meta, "suppression_count").unwrap_or(0);
                meta["suppression_count"] = JsonValue::Number(
                    serde_json::Number::from((current_count + 1).min(5)),
                );
                meta["last_suppressed_at"] = JsonValue::String(now.clone());
                let new_meta_str = serde_json::to_string(&meta)
                    .map_err(|e| XueliError::Serialization(format!("JSON 序列化失败: {e}")))?;

                conn.execute(
                    "UPDATE memory_items SET importance=?1, suppressed_at=?2, metadata_json=?3 WHERE id=?4",
                    params![new_importance, now, new_meta_str, mem_id],
                )
                .map_err(|e| XueliError::Database(format!("抑制更新失败: {e}")))?;
                suppressed += 1;
            }

            Ok(suppressed)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 标记记忆被召回（对应 Python 版 mark_recalled）
    pub async fn mark_recalled(&self, user_id: &str, memory_ids: &[String]) -> XueliResult<usize> {
        let user_id = user_id.to_string();
        let ids: Vec<String> = memory_ids.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<usize> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare("SELECT id, metadata_json, is_archived FROM memory_items WHERE user_id = ?1")
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let rows: Vec<(String, String, bool)> = stmt
                .query_map(params![user_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i32>(2)? != 0,
                    ))
                })
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            let wanted: std::collections::HashSet<&str> =
                ids.iter().map(|s| s.as_str()).collect();
            let now_iso = Utc::now().to_rfc3339();
            let mut updated = 0usize;

            for (mem_id, meta_json, is_archived) in &rows {
                if !wanted.contains(mem_id.as_str()) {
                    continue;
                }
                let mut meta = parse_metadata(meta_json);
                let mention_count = meta_get_i64(&meta, "mention_count").unwrap_or(1).max(1) + 1;
                meta["mention_count"] = JsonValue::Number(serde_json::Number::from(mention_count));
                meta["last_recalled_at"] = JsonValue::String(now_iso.clone());

                // 减少抑制计数
                let suppression_count = meta_get_i64(&meta, "suppression_count").unwrap_or(0);
                if suppression_count > 0 {
                    meta["suppression_count"] =
                        JsonValue::Number(serde_json::Number::from((suppression_count - 1).max(0)));
                }

                let new_meta_str = serde_json::to_string(&meta)
                    .map_err(|e| XueliError::Serialization(format!("JSON 序列化失败: {e}")))?;

                if *is_archived {
                    // 重新激活已归档记忆
                    meta["archived"] = JsonValue::Bool(false);
                    let recall_count =
                        meta_get_i64(&meta, "recall_count_since_archive").unwrap_or(0) + 1;
                    meta["recall_count_since_archive"] =
                        JsonValue::Number(serde_json::Number::from(recall_count));
                    let new_meta_str2 = serde_json::to_string(&meta)
                        .map_err(|e| XueliError::Serialization(format!("JSON 序列化失败: {e}")))?;
                    conn.execute(
                        "UPDATE memory_items SET metadata_json=?1, last_accessed_at=?2, is_archived=0 WHERE id=?3",
                        params![new_meta_str2, now_iso, mem_id],
                    )
                    .map_err(|e| XueliError::Database(format!("召回更新失败: {e}")))?;
                } else {
                    conn.execute(
                        "UPDATE memory_items SET metadata_json=?1, last_accessed_at=?2 WHERE id=?3",
                        params![new_meta_str, now_iso, mem_id],
                    )
                    .map_err(|e| XueliError::Database(format!("召回更新失败: {e}")))?;
                }
                updated += 1;
            }

            Ok(updated)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 替换用户所有记忆（对应 Python 版 replace_user_memories）
    pub async fn replace_user_memories(&self, user_id: &str, memories: Vec<MemoryItem>) -> XueliResult<bool> {
        let user_id = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<bool> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| XueliError::Database(format!("事务失败: {e}")))?;

            tx.execute("DELETE FROM memory_items WHERE user_id = ?1", params![user_id])
                .map_err(|e| XueliError::Database(format!("清理失败: {e}")))?;

            for item in &memories {
                tx.execute(
                    "INSERT INTO memory_items
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
            }

            tx.commit()
                .map_err(|e| XueliError::Database(format!("提交事务失败: {e}")))?;
            Ok(true)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 获取已归档的用户记忆（对应 Python 版 get_archived_user_memories）
    pub async fn get_archived_user_memories(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        let user_id = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Vec<MemoryItem>> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                     FROM memory_items WHERE user_id = ?1 AND is_archived = 1
                     ORDER BY last_accessed_at DESC, created_at DESC",
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
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 关键词搜索（对应 Python 版 search_memories_by_keyword）
    pub async fn search_memories_by_keyword(&self, keyword: &str, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        let memories = self.get_by_user(user_id).await?;
        let key = keyword.to_lowercase();
        Ok(memories
            .into_iter()
            .filter(|m| m.content.to_lowercase().contains(&key))
            .collect())
    }

    /// 文本规范化（对应 Python 版 _normalize_text）
    pub fn normalize_text(text: &str) -> String {
        let normalized = text.to_lowercase();
        let s: String = normalized
            .chars()
            .filter(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
            .collect();
        s.trim().to_string()
    }

    /// 判断两条记忆是否相同（对应 Python 版 _is_same_memory）
    pub fn is_same_memory(left: &str, right: &str) -> bool {
        let nl = Self::normalize_text(left);
        let nr = Self::normalize_text(right);
        if nl.is_empty() || nr.is_empty() {
            return false;
        }
        if nl == nr {
            return true;
        }
        let (shorter, longer) = if nl.len() <= nr.len() {
            (&nl, &nr)
        } else {
            (&nr, &nl)
        };
        if shorter.len() < 4 {
            return false;
        }
        longer.contains(shorter.as_str()) && (shorter.len() as f64 / longer.len().max(1) as f64) >= 0.75
    }

    /// 带去重的添加记忆（对应 Python 版 add_memory）
    pub async fn add_memory_dedup(
        &self,
        content: &str,
        user_id: &str,
        memory_type: MemoryType,
        importance: f64,
    ) -> XueliResult<Option<MemoryItem>> {
        let normalized_content = content.trim().to_string();
        if normalized_content.is_empty() {
            return Ok(None);
        }
        let user_id = user_id.to_string();

        // 检查去重
        let existing = self.get_by_user(&user_id).await?;
        for item in &existing {
            if Self::is_same_memory(&item.content, &normalized_content) {
                // 合并：保留较长的内容
                let new_content = if normalized_content.len() > item.content.len() {
                    normalized_content.clone()
                } else {
                    item.content.clone()
                };
                let mut updated = item.clone();
                updated.content = new_content;
                updated.last_accessed_at = Utc::now();
                self.update(updated).await?;
                return Ok(Some(item.clone()));
            }
        }

        // 新建记忆
        let now = Utc::now();
        let mem_id = format!(
            "mem_{}_{:04x}",
            now.format("%Y%m%d%H%M%S"),
            (normalized_content.len() as u16)
        );
        let item = MemoryItem {
            id: mem_id,
            user_id: user_id.clone(),
            content: normalized_content,
            memory_type,
            importance: importance.clamp(0.0, 1.0),
            created_at: now,
            last_accessed_at: now,
            access_count: 0,
        };
        self.store(item.clone()).await?;
        Ok(Some(item))
    }

    /// 完整版元数据加载（对应 Python 版 _load_metadata）
    pub async fn load_metadata(&self, user_id: &str, mem_id: &str) -> XueliResult<Option<serde_json::Value>> {
        let mem_id = mem_id.trim().to_string();
        let user_id = user_id.to_string();
        if user_id.is_empty() || mem_id.is_empty() {
            return Ok(None);
        }
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Option<serde_json::Value>> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let result = conn
                .query_row(
                    "SELECT metadata_json FROM memory_items WHERE id = ?1 AND user_id = ?2",
                    params![mem_id, user_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

            match result {
                Some(json_str) => Ok(Some(parse_metadata(&json_str))),
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 完整版元数据更新（对应 Python 版 _update_metadata）
    pub async fn update_metadata_full(
        &self,
        user_id: &str,
        mem_id: &str,
        metadata: &serde_json::Value,
    ) -> XueliResult<bool> {
        let mem_id = mem_id.trim().to_string();
        let user_id = user_id.to_string();
        if user_id.is_empty() || mem_id.is_empty() {
            return Ok(false);
        }
        let metadata = metadata.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<bool> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let now = Utc::now().to_rfc3339();
            let meta_str = serde_json::to_string(&metadata)
                .map_err(|e| XueliError::Serialization(format!("JSON 序列化失败: {e}")))?;
            let affected = conn
                .execute(
                    "UPDATE memory_items SET metadata_json=?1, last_accessed_at=?2 WHERE id=?3 AND user_id=?4",
                    params![meta_str, now, mem_id, user_id],
                )
                .map_err(|e| XueliError::Database(format!("元数据更新失败: {e}")))?;
            Ok(affected > 0)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 获取用户活跃（未归档）记忆
    pub async fn get_active_user_memories(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        let user_id = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Vec<MemoryItem>> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                     FROM memory_items WHERE user_id = ?1 AND is_archived = 0
                     ORDER BY last_accessed_at DESC, created_at DESC",
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
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    /// 获取所有用户 ID（对应 Python 版 get_user_ids）
    pub async fn get_user_ids(&self) -> XueliResult<Vec<String>> {
        self.get_all_user_ids().await
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

        let old_time = Utc::now() - chrono::TimeDelta::hours(1200);

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

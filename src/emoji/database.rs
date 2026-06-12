use rand::seq::SliceRandom;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

use crate::prelude::{XueliError, XueliResult};

/// 表情条目（内存缓存表示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmojiEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub file_path: String,
    pub usage_count: u64,
    pub sha256: String,
    pub emotion_label: Option<String>,
    pub created_at: String,
}

impl EmojiEntry {
    pub fn new(id: &str, name: &str, file_path: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            category: String::new(),
            tags: Vec::new(),
            file_path: file_path.to_string(),
            usage_count: 0,
            sha256: String::new(),
            emotion_label: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn from_sticker(row: &StickerRecord, file_hash: &str) -> Self {
        let entry_id = format!("emoji_{}", &file_hash[..16.min(file_hash.len())]);
        let mut entry = Self::new(&entry_id, "", &row.file_path);
        entry.sha256 = file_hash.to_string();
        entry.category = "sticker".to_string();
        entry.emotion_label = if row.description.is_empty() {
            None
        } else {
            Some(row.description.clone())
        };
        entry.usage_count = row.auto_reply_count as u64;
        entry.created_at = row.first_seen_at.clone();
        entry.tags = row
            .description
            .replace('，', ",")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        entry
    }
}

/// SQLite sticker 行表示
#[derive(Debug, Clone)]
pub struct StickerRecord {
    pub file_hash: String,
    pub file_path: String,
    pub file_format: String,
    pub description: String,
    pub emotion_status: String,
    pub is_registered: bool,
    pub is_banned: bool,
    pub query_count: i64,
    pub auto_reply_count: i64,
    pub last_auto_reply_at: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub message_id: String,
    pub user_id: String,
    pub group_id: Option<String>,
}

/// 表情库统计
#[derive(Debug, Clone, Default)]
pub struct EmojiStats {
    pub total: i64,
    pub registered: i64,
    pub pending: i64,
    pub banned: i64,
}

/// SQLite 持久化存储层
///
/// 提供贴纸的完整 CRUD，异步方法通过 `tokio::task::spawn_blocking` 包裹同步 SQL 操作。
pub struct EmojiDatabase {
    db_path: PathBuf,
    lock: Arc<AsyncMutex<()>>,
    /// 最大存储的已注册表情数量
    max_stored: usize,
    /// 溢出策略: "replace_oldest" 或 "reject_new"
    overflow_policy: String,
}

impl EmojiDatabase {
    pub fn new(db_path: impl AsRef<Path>) -> XueliResult<Self> {
        Self::with_config(db_path, 100, "replace_oldest")
    }

    pub fn with_config(
        db_path: impl AsRef<Path>,
        max_stored: usize,
        overflow_policy: &str,
    ) -> XueliResult<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }

        let store = Self {
            db_path,
            lock: Arc::new(AsyncMutex::new(())),
            max_stored,
            overflow_policy: overflow_policy.to_string(),
        };
        store.init_db()?;
        Ok(store)
    }

    fn init_db(&self) -> XueliResult<()> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;
        conn.execute_batch("PRAGMA busy_timeout=3000")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS stickers (
                file_hash TEXT PRIMARY KEY,
                file_path TEXT NOT NULL DEFAULT '',
                file_format TEXT NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                emotion_status TEXT NOT NULL DEFAULT 'pending',
                is_registered INTEGER NOT NULL DEFAULT 0,
                is_banned INTEGER NOT NULL DEFAULT 0,
                query_count INTEGER NOT NULL DEFAULT 0,
                auto_reply_count INTEGER NOT NULL DEFAULT 0,
                last_auto_reply_at TEXT NOT NULL DEFAULT '',
                first_seen_at TEXT NOT NULL DEFAULT '',
                last_seen_at TEXT NOT NULL DEFAULT '',
                message_id TEXT NOT NULL DEFAULT '',
                user_id TEXT NOT NULL DEFAULT '',
                group_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_stickers_status ON stickers(emotion_status);
            CREATE INDEX IF NOT EXISTS idx_stickers_registered ON stickers(is_registered);",
        )
        .map_err(|e| format!("建表失败: {}", e))?;

        Ok(())
    }

    fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<StickerRecord> {
        Ok(StickerRecord {
            file_hash: row.get(0)?,
            file_path: row.get::<_, String>(1).unwrap_or_default(),
            file_format: row.get::<_, String>(2).unwrap_or_default(),
            description: row.get::<_, String>(3).unwrap_or_default(),
            emotion_status: row.get::<_, String>(4).unwrap_or_default(),
            is_registered: row.get::<_, i32>(5).unwrap_or(0) != 0,
            is_banned: row.get::<_, i32>(6).unwrap_or(0) != 0,
            query_count: row.get::<_, i64>(7).unwrap_or(0),
            auto_reply_count: row.get::<_, i64>(8).unwrap_or(0),
            last_auto_reply_at: row.get::<_, String>(9).unwrap_or_default(),
            first_seen_at: row.get::<_, String>(10).unwrap_or_default(),
            last_seen_at: row.get::<_, String>(11).unwrap_or_default(),
            message_id: row.get::<_, String>(12).unwrap_or_default(),
            user_id: row.get::<_, String>(13).unwrap_or_default(),
            group_id: row.get::<_, Option<String>>(14).unwrap_or(None),
        })
    }

    fn _get_record_sync(conn: &Connection, file_hash: &str) -> Option<StickerRecord> {
        conn.query_row(
            "SELECT file_hash, file_path, file_format, description, emotion_status,
                    is_registered, is_banned, query_count, auto_reply_count,
                    last_auto_reply_at, first_seen_at, last_seen_at,
                    message_id, user_id, group_id
             FROM stickers WHERE file_hash = ?1",
            params![file_hash],
            Self::row_to_record,
        )
        .optional()
        .ok()
        .flatten()
    }

    // ── 同步操作（内部使用）──

    pub fn sync_get_record(&self, file_hash: &str) -> Option<StickerRecord> {
        let conn = Connection::open(&self.db_path).ok()?;
        Self::_get_record_sync(&conn, file_hash)
    }

    // ── 异步操作 ──

    /// 获取 sticker 记录
    pub async fn get_record_async(&self, file_hash: &str) -> Option<StickerRecord> {
        let db_path = self.db_path.clone();
        let key = file_hash.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).ok()?;
            Self::_get_record_sync(&conn, &key)
        })
        .await
        .unwrap_or(None)
    }

    /// 保存贴纸（SHA256 去重、溢出管理、完整持久化）
    pub async fn save_sticker_async(
        &self,
        file_hash: &str,
        file_path: &str,
        file_format: &str,
        description: &str,
        message_id: &str,
        user_id: &str,
        group_id: &str,
    ) -> XueliResult<Option<StickerRecord>> {
        let now = Self::now_iso();
        let db_path = self.db_path.clone();
        let fh = file_hash.to_string();
        let fp = file_path.to_string();
        let ff = file_format.to_string();
        let desc = description.to_string();
        let mid = message_id.to_string();
        let uid = user_id.to_string();
        let gid = if group_id.is_empty() {
            None
        } else {
            Some(group_id.to_string())
        };

        let _guard = self.lock.lock().await;

        let max_stored = self.max_stored;
        let overflow_policy = self.overflow_policy.clone();

        let result: XueliResult<Option<StickerRecord>> = tokio::task::spawn_blocking(move || {
            let conn =
                Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;

            // 检查是否已存在
            if Self::_get_record_sync(&conn, &fh).is_some() {
                conn.execute(
                    "UPDATE stickers SET last_seen_at=?1, query_count=query_count+1 WHERE file_hash=?2",
                    params![now, fh],
                )
                .map_err(|e| format!("更新贴纸失败: {}", e))?;
                return Ok(Self::_get_record_sync(&conn, &fh));
            }

            // 容量溢出检查
            let registered_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM stickers WHERE is_registered=1 AND is_banned=0",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if registered_count >= max_stored as i64 {
                if overflow_policy == "reject_new" {
                    return Ok(None);
                }
                let oldest: Option<(String, String)> = conn
                    .query_row(
                        "SELECT file_hash, file_path FROM stickers WHERE is_registered=1 AND is_banned=0 ORDER BY last_seen_at ASC LIMIT 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .ok()
                    .flatten();
                if let Some((old_hash, old_path)) = oldest {
                    conn.execute("DELETE FROM stickers WHERE file_hash=?1", params![old_hash])
                        .map_err(|e| format!("删除溢出贴纸失败: {}", e))?;
                    if !old_path.is_empty() {
                        let _ = std::fs::remove_file(&old_path);
                    }
                }
            }

            conn.execute(
                "INSERT INTO stickers
                    (file_hash, file_path, file_format, description, emotion_status,
                     is_registered, first_seen_at, last_seen_at,
                     message_id, user_id, group_id)
                 VALUES (?1,?2,?3,?4,'pending',0,?5,?5,?6,?7,?8)",
                params![fh, fp, ff, desc, now, mid, uid, gid],
            )
            .map_err(|e| format!("插入贴纸失败: {}", e))?;

            Ok(Self::_get_record_sync(&conn, &fh))
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?;

        result
    }

    /// 标记贴纸为已分类
    pub async fn register_async(&self, file_hash: &str) -> XueliResult<()> {
        let db_path = self.db_path.clone();
        let key = file_hash.to_string();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn =
                Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "UPDATE stickers SET is_registered=1, emotion_status='classified' WHERE file_hash=?1",
                params![key],
            )
            .map_err(|e| format!("注册贴纸失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 更新贴纸描述
    pub async fn set_description_async(
        &self,
        file_hash: &str,
        description: &str,
    ) -> XueliResult<()> {
        let db_path = self.db_path.clone();
        let key = file_hash.to_string();
        let desc = description.to_string();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "UPDATE stickers SET description=?1 WHERE file_hash=?2",
                params![desc, key],
            )
            .map_err(|e| format!("设置描述失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 更新贴纸分类状态（追踪分类生命周期）
    pub async fn set_emotion_status_async(&self, file_hash: &str, status: &str) -> XueliResult<()> {
        let db_path = self.db_path.clone();
        let key = file_hash.to_string();
        let st = status.to_string();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "UPDATE stickers SET emotion_status=?1 WHERE file_hash=?2",
                params![st, key],
            )
            .map_err(|e| format!("设置分类状态失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 列出待分类的贴纸
    pub async fn list_pending_async(&self) -> XueliResult<Vec<StickerRecord>> {
        self.list_pending_async_with_limit(1).await
    }

    /// 列出待分类的贴纸（自定义数量限制）
    pub async fn list_pending_async_with_limit(
        &self,
        limit: usize,
    ) -> XueliResult<Vec<StickerRecord>> {
        let db_path = self.db_path.clone();
        let effective_limit = limit.max(1);
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            let mut stmt = conn
                .prepare(
                    "SELECT file_hash, file_path, file_format, description, emotion_status,
                            is_registered, is_banned, query_count, auto_reply_count,
                            last_auto_reply_at, first_seen_at, last_seen_at,
                            message_id, user_id, group_id
                     FROM stickers WHERE emotion_status='pending' AND is_banned=0
                     ORDER BY last_seen_at ASC LIMIT ?1",
                )
                .map_err(|e| format!("准备查询失败: {}", e))?;
            let records: Vec<StickerRecord> = stmt
                .query_map(params![effective_limit], Self::row_to_record)
                .map_err(|e| format!("查询失败: {}", e))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(records)
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 按意图关键词匹配贴纸（基于描述文本匹配）
    pub fn find_by_intent(&self, query: &str) -> XueliResult<Vec<StickerRecord>> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;

        let mut stmt = conn
            .prepare(
                "SELECT file_hash, file_path, file_format, description, emotion_status,
                        is_registered, is_banned, query_count, auto_reply_count,
                        last_auto_reply_at, first_seen_at, last_seen_at,
                        message_id, user_id, group_id
                 FROM stickers WHERE emotion_status='classified' AND is_banned=0 LIMIT 200",
            )
            .map_err(|e| format!("准备查询失败: {}", e))?;

        let records: Vec<StickerRecord> = stmt
            .query_map([], Self::row_to_record)
            .map_err(|e| format!("查询失败: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Self::match_by_intent(records, query))
    }

    fn match_by_intent(records: Vec<StickerRecord>, intent: &str) -> Vec<StickerRecord> {
        if intent.is_empty() {
            let mut sorted = records;
            sorted.sort_by_key(|r| r.auto_reply_count);
            return sorted;
        }
        let mut results: Vec<StickerRecord> = records
            .into_iter()
            .filter(|r| {
                r.description.replace('，', ",").split(',').any(|tag| {
                    let t = tag.trim();
                    !t.is_empty() && (t.contains(intent) || intent.contains(t))
                })
            })
            .collect();
        if results.is_empty() {
            // 回退：返回全部按 auto_reply_count 排序
            return Vec::new();
        }
        results.sort_by_key(|r| r.auto_reply_count);
        results
    }

    /// 异步按意图关键词匹配贴纸
    pub async fn find_by_intent_async(&self, intent: &str) -> XueliResult<Vec<StickerRecord>> {
        let db_path = self.db_path.clone();
        let intent = intent.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            let mut stmt = conn
                .prepare(
                    "SELECT file_hash, file_path, file_format, description, emotion_status,
                            is_registered, is_banned, query_count, auto_reply_count,
                            last_auto_reply_at, first_seen_at, last_seen_at,
                            message_id, user_id, group_id
                     FROM stickers WHERE emotion_status='classified' AND is_banned=0 LIMIT 200",
                )
                .map_err(|e| format!("准备查询失败: {}", e))?;

            let records: Vec<StickerRecord> = stmt
                .query_map([], Self::row_to_record)
                .map_err(|e| format!("查询失败: {}", e))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(Self::match_by_intent(records, &intent))
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 根据文件路径查询贴纸记录
    pub fn get_record_by_path(&self, file_path: &str) -> XueliResult<Option<StickerRecord>> {
        let conn = Connection::open(&self.db_path)
            .map_err(|e| XueliError::Internal(format!("打开 DB 失败: {}", e)))?;
        conn.query_row(
            "SELECT file_hash, file_path, file_format, description, emotion_status,
                    is_registered, is_banned, query_count, auto_reply_count,
                    last_auto_reply_at, first_seen_at, last_seen_at,
                    message_id, user_id, group_id
             FROM stickers WHERE file_path = ?1",
            params![file_path],
            Self::row_to_record,
        )
        .optional()
        .map_err(|e| XueliError::Internal(format!("查询失败: {}", e)))
    }

    /// 查找适合回复的候选贴纸
    pub fn find_reply_candidates(
        &self,
        target_intent: &str,
        target_tone: &str,
        target_emotion: &str,
    ) -> XueliResult<Vec<StickerRecord>> {
        let intent = if !target_intent.is_empty() {
            target_intent.to_string()
        } else if !target_tone.is_empty() && !target_emotion.is_empty() {
            format!("{}-{}", target_tone, target_emotion)
        } else {
            target_emotion.to_string()
        };
        self.find_by_intent(&intent)
    }

    /// 异步查找适合回复的候选贴纸
    pub async fn find_reply_candidates_async(
        &self,
        target_intent: &str,
        target_tone: &str,
        target_emotion: &str,
    ) -> XueliResult<Vec<StickerRecord>> {
        let intent = if !target_intent.is_empty() {
            target_intent.to_string()
        } else if !target_tone.is_empty() && !target_emotion.is_empty() {
            format!("{}-{}", target_tone, target_emotion)
        } else {
            target_emotion.to_string()
        };
        self.find_by_intent_async(&intent).await
    }

    /// 获取已注册且未被禁用的贴纸数量
    pub fn get_registered_count(&self) -> XueliResult<i64> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stickers WHERE is_registered=1 AND is_banned=0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count)
    }

    /// 异步获取已注册且未被禁用的贴纸数量
    pub async fn get_registered_count_async(&self) -> XueliResult<i64> {
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM stickers WHERE is_registered=1 AND is_banned=0",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(count)
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 异步检查是否有已分类的表情数据
    pub async fn has_emoji_data_async(&self) -> XueliResult<bool> {
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = match Connection::open(&db_path) {
                Ok(c) => c,
                Err(_) => return Ok(false),
            };
            let result = conn
                .query_row(
                    "SELECT 1 FROM stickers WHERE emotion_status='classified' AND is_banned=0 LIMIT 1",
                    [],
                    |_| Ok(()),
                )
                .is_ok();
            Ok(result)
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 获取统计信息：总数、已注册、待分类、已禁用
    pub fn get_stats(&self) -> XueliResult<EmojiStats> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM stickers", [], |row| row.get(0))
            .unwrap_or(0);
        let registered: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stickers WHERE is_registered=1 AND is_banned=0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stickers WHERE emotion_status='pending' AND is_banned=0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let banned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM stickers WHERE is_banned=1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(EmojiStats {
            total,
            registered,
            pending,
            banned,
        })
    }

    /// 标记贴纸为已发送（增加计数）
    pub async fn mark_auto_reply_sent_async(&self, file_hash: &str) -> XueliResult<()> {
        let db_path = self.db_path.clone();
        let key = file_hash.to_string();
        let now = Self::now_iso();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn =
                Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "UPDATE stickers SET auto_reply_count=auto_reply_count+1, last_auto_reply_at=?1, query_count=query_count+1 WHERE file_hash=?2",
                params![now, key],
            )
            .map_err(|e| format!("标记发送失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 禁用贴纸
    pub async fn ban_async(&self, file_hash: &str) -> XueliResult<()> {
        let db_path = self.db_path.clone();
        let key = file_hash.to_string();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "UPDATE stickers SET is_banned=1 WHERE file_hash=?1",
                params![key],
            )
            .map_err(|e| format!("禁用贴纸失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 获取所有已注册贴纸
    pub async fn get_all_registered_async(&self) -> XueliResult<Vec<StickerRecord>> {
        let db_path = self.db_path.clone();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            let mut stmt = conn
                .prepare(
                    "SELECT file_hash, file_path, file_format, description, emotion_status,
                            is_registered, is_banned, query_count, auto_reply_count,
                            last_auto_reply_at, first_seen_at, last_seen_at,
                            message_id, user_id, group_id
                     FROM stickers WHERE is_registered=1 AND is_banned=0",
                )
                .map_err(|e| format!("准备查询失败: {}", e))?;
            let records: Vec<StickerRecord> = stmt
                .query_map([], Self::row_to_record)
                .map_err(|e| format!("查询失败: {}", e))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(records)
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 是否有已分类的贴纸数据
    pub fn has_emoji_data(&self) -> bool {
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        conn.query_row(
            "SELECT 1 FROM stickers WHERE emotion_status='classified' AND is_banned=0 LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok()
    }
}

/// 表情数据库 — 内存缓存 + SQLite 持久化，按标签和分类检索。
pub struct EmojiDB {
    /// SQLite 持久化层
    database: EmojiDatabase,
    entries: Mutex<Vec<EmojiEntry>>,
    /// SHA256 去重索引
    sha256_index: Mutex<HashMap<String, usize>>,
    /// 被禁用的表情 ID
    banned: Mutex<Vec<String>>,
    max_stickers: usize,
}

impl EmojiDB {
    /// 从存储目录创建（SQLite 文件为 `{dir}/emojis.db`）
    pub fn new(dir: &str) -> Self {
        let db_path = format!("{}/emojis.db", dir);
        let database = EmojiDatabase::new(&db_path).expect("创建 EmojiDatabase 失败");
        let store = Self {
            database,
            entries: Mutex::new(Vec::new()),
            sha256_index: Mutex::new(HashMap::new()),
            banned: Mutex::new(Vec::new()),
            max_stickers: 2000,
        };
        store.load_from_sqlite();
        store
    }

    /// 从 SQLite 加载已注册贴纸到内存缓存
    fn load_from_sqlite(&self) {
        let db_path = self.database.db_path.clone();
        let conn = match Connection::open(&db_path) {
            Ok(c) => c,
            Err(_) => return,
        };

        let mut stmt = match conn.prepare(
            "SELECT file_hash, file_path, file_format, description, emotion_status,
                    is_registered, is_banned, query_count, auto_reply_count,
                    last_auto_reply_at, first_seen_at, last_seen_at,
                    message_id, user_id, group_id
             FROM stickers WHERE is_registered=1 AND is_banned=0",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };

        let records: Vec<StickerRecord> = stmt
            .query_map([], EmojiDatabase::row_to_record)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let mut entries = self.entries.lock().unwrap();
        let mut index = self.sha256_index.lock().unwrap();
        entries.clear();
        index.clear();

        for record in &records {
            let entry = EmojiEntry::from_sticker(record, &record.file_hash);
            let idx = entries.len();
            if !entry.sha256.is_empty() {
                index.insert(entry.sha256.clone(), idx);
            }
            entries.push(entry);
        }
    }

    /// 获取 SQLite 持久化层引用
    pub fn database(&self) -> &EmojiDatabase {
        &self.database
    }

    /// 添加表情条目（SHA256 去重，同时持久化到 SQLite）
    pub fn add_emoji(&self, entry: EmojiEntry) -> XueliResult<String> {
        if !entry.sha256.is_empty() {
            let index = self.sha256_index.lock().unwrap();
            if index.contains_key(&entry.sha256) {
                return Ok(String::new());
            }
            drop(index);
        }

        let emoji_id = {
            let mut entries = self.entries.lock().unwrap();
            for e in entries.iter() {
                if (!entry.sha256.is_empty() && e.sha256 == entry.sha256)
                    || e.file_path == entry.file_path
                {
                    return Ok(e.id.clone());
                }
            }
            let id = entry.id.clone();
            entries.push(entry);

            while entries.len() > self.max_stickers {
                let old = entries.remove(0);
                if !old.sha256.is_empty() {
                    self.sha256_index.lock().unwrap().remove(&old.sha256);
                }
            }

            id
        };

        self.rebuild_index();
        Ok(emoji_id)
    }

    /// 按标签搜索表情
    pub fn find_by_tags(&self, tags: &[&str]) -> XueliResult<Vec<EmojiEntry>> {
        let entries = self.entries.lock().unwrap();
        let banned = self.banned.lock().unwrap();

        let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        let results: Vec<EmojiEntry> = entries
            .iter()
            .filter(|e| {
                !banned.contains(&e.id)
                    && lower_tags
                        .iter()
                        .any(|t| e.tags.iter().any(|et| et.to_lowercase().contains(t)))
            })
            .cloned()
            .collect();

        Ok(results)
    }

    /// 按分类随机获取一个表情
    pub fn get_random(&self, category: Option<&str>) -> XueliResult<Option<EmojiEntry>> {
        let entries = self.entries.lock().unwrap();
        let banned = self.banned.lock().unwrap();
        let mut rng = rand::thread_rng();

        let candidates: Vec<&EmojiEntry> = entries
            .iter()
            .filter(|e| !banned.contains(&e.id) && category.map_or(true, |c| e.category == c))
            .collect();

        Ok(candidates.choose(&mut rng).cloned().cloned())
    }

    /// 捡一个特定情绪标签的表情
    pub fn find_by_emotion(&self, emotion: &str) -> XueliResult<Option<EmojiEntry>> {
        let entries = self.entries.lock().unwrap();
        let mut rng = rand::thread_rng();

        let candidates: Vec<&EmojiEntry> = entries
            .iter()
            .filter(|e| e.emotion_label.as_deref() == Some(emotion))
            .collect();

        Ok(candidates.choose(&mut rng).cloned().cloned())
    }

    /// 增加使用计数
    pub fn increment_usage(&self, emoji_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(entry) = entries.iter_mut().find(|e| e.id == emoji_id) {
                entry.usage_count += 1;
            }
        }
    }

    /// 禁用某个表情
    pub fn ban(&self, emoji_id: &str) {
        self.banned.lock().unwrap().push(emoji_id.to_string());
    }

    /// 表情总数
    pub fn count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// 从 SQLite 同步重新加载内存缓存
    pub fn refresh_cache(&self) {
        self.load_from_sqlite();
    }

    fn rebuild_index(&self) {
        if let (Ok(entries), Ok(mut index)) = (self.entries.lock(), self.sha256_index.lock()) {
            index.clear();
            for (i, entry) in entries.iter().enumerate() {
                if !entry.sha256.is_empty() {
                    index.insert(entry.sha256.clone(), i);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_db() -> (EmojiDB, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = EmojiDB::new(dir.path().to_str().unwrap());
        (db, dir)
    }

    fn make_database() -> (EmojiDatabase, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("emojis.db");
        (EmojiDatabase::new(&db_path).unwrap(), dir)
    }

    #[test]
    fn test_add_and_find_by_tags() {
        let (db, _dir) = make_db();
        let mut e = EmojiEntry::new("e1", "笑脸", "path/img.png");
        e.sha256 = "abc123".into();
        e.tags = vec!["开心".into(), "问候".into()];
        db.add_emoji(e).unwrap();

        let found = db.find_by_tags(&["开心"]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "笑脸");
    }

    #[test]
    fn test_sha256_dedup() {
        let (db, _dir) = make_db();
        let mut e1 = EmojiEntry::new("e1", "A", "p1.png");
        e1.sha256 = "same_hash".into();
        db.add_emoji(e1).unwrap();

        let mut e2 = EmojiEntry::new("e2", "B", "p2.png");
        e2.sha256 = "same_hash".into();
        let id = db.add_emoji(e2).unwrap();
        assert!(id.is_empty());
    }

    #[test]
    fn test_get_random() {
        let (db, _dir) = make_db();
        let mut e = EmojiEntry::new("e1", "笑", "p1.png");
        e.sha256 = "h1".into();
        e.category = "greeting".into();
        db.add_emoji(e).unwrap();

        let result = db.get_random(Some("greeting")).unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_save_sticker_async() {
        let (database, _dir) = make_database();
        let record = database
            .save_sticker_async(
                "abc123hash",
                "/tmp/test.png",
                "png",
                "开心,问候",
                "msg_1",
                "user_1",
                "group_1",
            )
            .await
            .unwrap();
        assert!(record.is_some());
        let r = record.unwrap();
        assert_eq!(r.file_hash, "abc123hash");
        assert_eq!(r.emotion_status, "pending");
        assert!(!r.is_registered);
    }

    #[tokio::test]
    async fn test_save_sticker_duplicate() {
        let (database, _dir) = make_database();
        database
            .save_sticker_async("hash1", "/tmp/a.png", "png", "", "", "", "")
            .await
            .unwrap();
        let result = database
            .save_sticker_async("hash1", "/tmp/b.png", "png", "", "", "", "")
            .await
            .unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_register_then_list_pending() {
        let (database, _dir) = make_database();
        database
            .save_sticker_async("hash_a", "/tmp/a.png", "png", "", "", "", "")
            .await
            .unwrap();
        database
            .save_sticker_async("hash_b", "/tmp/b.png", "png", "", "", "", "")
            .await
            .unwrap();

        // 两个都是 pending
        let pending = database.list_pending_async().await.unwrap();
        assert_eq!(pending.len(), 1); // LIMIT 1

        // 注册第一个
        database.register_async("hash_a").await.unwrap();
        let record = database.get_record_async("hash_a").await.unwrap();
        assert!(record.is_registered);
        assert_eq!(record.emotion_status, "classified");
    }

    #[tokio::test]
    async fn test_set_description_and_emotion_status() {
        let (database, _dir) = make_database();
        database
            .save_sticker_async("hash_x", "/tmp/x.png", "gif", "", "", "", "")
            .await
            .unwrap();

        database
            .set_description_async("hash_x", "开心,可爱")
            .await
            .unwrap();
        database
            .set_emotion_status_async("hash_x", "processing")
            .await
            .unwrap();

        let record = database.get_record_async("hash_x").await.unwrap();
        assert_eq!(record.description, "开心,可爱");
        assert_eq!(record.emotion_status, "processing");
    }

    #[test]
    fn test_find_by_intent() {
        let (database, _dir) = make_database();
        // 需要手动插入已分类的记录进行测试
        let conn = Connection::open(&database.db_path).unwrap();
        conn.execute(
            "INSERT INTO stickers (file_hash, file_path, file_format, description, emotion_status, is_registered) VALUES (?1,?2,?3,?4,'classified',1)",
            params!["hash_1", "/tmp/a.png", "png", "开心,问候"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stickers (file_hash, file_path, file_format, description, emotion_status, is_registered) VALUES (?1,?2,?3,?4,'classified',1)",
            params!["hash_2", "/tmp/b.png", "png", "难过,伤心"],
        )
        .unwrap();

        let found = database.find_by_intent("开心").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file_hash, "hash_1");

        let none = database.find_by_intent("不存在").unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn test_get_stats() {
        let (database, _dir) = make_database();
        let conn = Connection::open(&database.db_path).unwrap();
        conn.execute(
            "INSERT INTO stickers (file_hash, file_path, file_format, description, emotion_status, is_registered) VALUES (?1,?2,?3,?4,'classified',1)",
            params!["h1", "p1", "png", "happy"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stickers (file_hash, file_path, file_format, description, emotion_status, is_registered) VALUES (?1,?2,?3,?4,'pending',0)",
            params!["h2", "p2", "png", ""],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO stickers (file_hash, file_path, file_format, description, emotion_status, is_registered, is_banned) VALUES (?1,?2,?3,?4,'classified',1,1)",
            params!["h3", "p3", "png", "banned"],
        )
        .unwrap();

        let stats = database.get_stats().unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.registered, 1);
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.banned, 1);
    }

    #[test]
    fn test_has_emoji_data() {
        let (database, _dir) = make_database();
        assert!(!database.has_emoji_data());

        let conn = Connection::open(&database.db_path).unwrap();
        conn.execute(
            "INSERT INTO stickers (file_hash, file_path, file_format, description, emotion_status, is_registered) VALUES (?1,?2,?3,?4,'classified',1)",
            params!["h1", "p1", "png", "tag"],
        )
        .unwrap();
        assert!(database.has_emoji_data());
    }
}

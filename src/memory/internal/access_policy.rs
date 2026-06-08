use std::collections::HashMap;

use crate::core::scope::ChatScope;
use crate::core::types::{MemoryItem, MemoryType};

const ADDRESSING_PATTERNS: &[&str] = &["叫我", "称呼我", "喊我"];

/// 记忆元数据的可见性和分类标注
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryVisibility {
    Private,
    Shared,
}

/// 记忆内容的语义分类
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryContentCategory {
    PersonalInfo,
    Preference,
    Relationship,
    Event,
    Opinion,
    Skill,
    Health,
    Finance,
    DailyChat,
    Generic,
}

/// 记忆的适用范围
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryApplicabilityScope {
    SelfOnly,
    DirectUsers,
    GroupMembers,
    Public,
    Unknown,
}

/// 记忆访问的上下文信息
#[derive(Debug, Clone)]
pub struct MemoryAccessContext {
    pub requester_user_id: String,
    pub message_type: String,
    pub group_id: String,
    pub read_scope: String,
    pub platform: String,
    pub hour_of_day: i32,
}

impl Default for MemoryAccessContext {
    fn default() -> Self {
        Self {
            requester_user_id: String::new(),
            message_type: "private".to_string(),
            group_id: String::new(),
            read_scope: "user".to_string(),
            platform: String::new(),
            hour_of_day: -1,
        }
    }
}

/// 记忆访问策略 — 决定哪些记忆对当前上下文可见。
///
/// 包含三层过滤：类型过滤、隐私可见性、适用范围。
pub struct MemoryAccessPolicy {
    pub private_allowed_types: Vec<MemoryType>,
    pub group_allowed_types: Vec<MemoryType>,
}

/// 提示词条目类型
pub type PromptEntry = HashMap<String, serde_json::Value>;

impl MemoryAccessPolicy {
    pub fn new() -> Self {
        Self {
            private_allowed_types: vec![
                MemoryType::Fact,
                MemoryType::Preference,
                MemoryType::Event,
                MemoryType::Opinion,
                MemoryType::Relationship,
            ],
            group_allowed_types: vec![MemoryType::Fact, MemoryType::Event],
        }
    }

    /// 判断某条记忆在当前作用域下是否可访问
    pub fn is_accessible(&self, memory: &MemoryItem, scope: &ChatScope) -> bool {
        let allowed = match scope {
            ChatScope::Private => &self.private_allowed_types,
            ChatScope::Group(_) => &self.group_allowed_types,
        };
        allowed.contains(&memory.memory_type)
    }

    /// 过滤可访问的记忆
    pub fn filter_accessible(&self, memories: &[MemoryItem], scope: &ChatScope) -> Vec<MemoryItem> {
        memories
            .iter()
            .filter(|m| self.is_accessible(m, scope))
            .cloned()
            .collect()
    }

    /// 去重：按内容文本相同去除重复记忆，保留 importance 最高的
    pub fn dedupe_entries(memories: &[MemoryItem]) -> Vec<MemoryItem> {
        let mut best: HashMap<String, MemoryItem> = HashMap::new();
        for m in memories {
            let key = m.content.trim().to_lowercase();
            best.entry(key)
                .and_modify(|existing| {
                    if m.importance > existing.importance {
                        *existing = m.clone();
                    }
                })
                .or_insert_with(|| m.clone());
        }
        let mut result: Vec<_> = best.into_values().collect();
        result.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }

    /// 降序排列（按 importance）
    pub fn sort_by_importance(memories: &mut [MemoryItem]) {
        memories.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// 检查元数据是否标记为 shared
    pub fn is_shared(&self, metadata: Option<&serde_json::Value>) -> bool {
        let meta = match metadata {
            Some(v) => v,
            None => return false,
        };
        meta.get("visibility")
            .and_then(|v| v.as_str())
            .map(|s| s == "shared")
            .unwrap_or(false)
    }

    /// 检查元数据是否标记为 addressing preference
    pub fn is_addressing(&self, metadata: Option<&serde_json::Value>) -> bool {
        let meta = match metadata {
            Some(v) => v,
            None => return false,
        };
        meta.get("content_category")
            .and_then(|v| v.as_str())
            .map(|s| s == "addressing_preference")
            .unwrap_or(false)
    }

    /// 为提示词分类记忆：返回 "private"、"shared" 或 "addressing"
    pub fn classify_for_prompt(
        &self,
        metadata: Option<&serde_json::Value>,
        owner_user_id: &str,
        requester_user_id: &str,
    ) -> &'static str {
        if self.is_addressing(metadata) {
            return "addressing";
        }
        if self.is_shared(metadata) && owner_user_id != requester_user_id {
            return "shared";
        }
        "private"
    }

    /// 对提示词条目去重（按规范化内容文本）
    pub fn dedupe_prompt_entries(&self, entries: &[PromptEntry]) -> Vec<PromptEntry> {
        let mut seen: HashMap<String, PromptEntry> = HashMap::new();
        for entry in entries {
            let content = entry
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if content.is_empty() || seen.contains_key(&content) {
                continue;
            }
            seen.insert(content, entry.clone());
        }
        seen.into_values().collect()
    }

    /// Build a MemoryAccessContext from raw parameters
    pub fn build_context(
        requester_user_id: &str,
        message_type: &str,
        group_id: &str,
        read_scope: &str,
        platform: &str,
        hour_of_day: i32,
    ) -> MemoryAccessContext {
        MemoryAccessContext {
            requester_user_id: requester_user_id.to_string(),
            message_type: message_type.to_string(),
            group_id: group_id.to_string(),
            read_scope: read_scope.to_string(),
            platform: platform.to_string(),
            hour_of_day,
        }
    }

    /// Infer category from content text and metadata tags
    fn _normalize_category(&self, content: &str, metadata: Option<&serde_json::Value>) -> String {
        for pattern in ADDRESSING_PATTERNS {
            if content.contains(pattern) {
                return "addressing_preference".to_string();
            }
        }
        if let Some(meta) = metadata {
            if let Some(cat) = meta.get("content_category").and_then(|v| v.as_str()) {
                return cat.to_string();
            }
            if let Some(tags) = meta.get("tags").and_then(|v| v.as_array()) {
                for tag in tags {
                    if let Some(tag_str) = tag.as_str() {
                        let lower = tag_str.to_lowercase();
                        if lower == "group_rule" {
                            return "group_rule".to_string();
                        }
                        if lower == "bot_rule" {
                            return "bot_rule".to_string();
                        }
                        if lower == "public_rule" {
                            return "public_rule".to_string();
                        }
                    }
                }
            }
        }
        "generic".to_string()
    }

    /// Normalize visibility based on content category
    fn _normalize_visibility(&self, category: &str) -> MemoryVisibility {
        match category {
            "addressing_preference" => MemoryVisibility::Private,
            "group_rule" | "bot_rule" | "public_rule" => MemoryVisibility::Shared,
            _ => MemoryVisibility::Private,
        }
    }

    /// Normalize applicability scope from category and record
    fn _normalize_applicability_scope(
        &self,
        category: &str,
        record: &serde_json::Value,
    ) -> HashMap<String, String> {
        let mut scope = HashMap::new();
        let kind = match category {
            "addressing_preference" => {
                if let Some(uid) = record.get("user_id").and_then(|v| v.as_str()) {
                    scope.insert("user_id".to_string(), uid.to_string());
                }
                "self_only"
            }
            "group_rule" => {
                if let Some(gid) = record.get("group_id").and_then(|v| v.as_str()) {
                    scope.insert("group_id".to_string(), gid.to_string());
                }
                "group_members"
            }
            "bot_rule" | "public_rule" => "public",
            "personal_info" | "preference" => "self_only",
            "event" | "opinion" => "direct_users",
            _ => "public",
        };
        scope.insert("kind".to_string(), kind.to_string());
        scope
    }

    /// Full normalization: category → visibility → applicability_scope
    pub fn normalize_memory_record(&self, record: &serde_json::Value) -> serde_json::Value {
        let content = record.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let metadata = record.get("metadata");

        let category = self._normalize_category(content, metadata);
        let visibility = self._normalize_visibility(&category);
        let scope = self._normalize_applicability_scope(&category, record);

        let mut normalized = match record.clone() {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        normalized.insert(
            "content_category".to_string(),
            serde_json::Value::String(category),
        );
        normalized.insert(
            "visibility".to_string(),
            serde_json::Value::String(match visibility {
                MemoryVisibility::Private => "private".to_string(),
                MemoryVisibility::Shared => "shared".to_string(),
            }),
        );
        normalized.insert(
            "applicability_scope".to_string(),
            serde_json::Value::Object(
                scope
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect(),
            ),
        );
        serde_json::Value::Object(normalized)
    }

    /// 5-way scope matching between memory scope and access context
    fn _scope_matches(
        &self,
        memory_scope: &HashMap<String, String>,
        context: &MemoryAccessContext,
    ) -> bool {
        let kind = memory_scope
            .get("kind")
            .map(|s| s.as_str())
            .unwrap_or("self_only");
        match kind {
            "self_only" => memory_scope
                .get("user_id")
                .map(|uid| uid == &context.requester_user_id)
                .unwrap_or(false),
            "direct_users" => memory_scope
                .get("user_id")
                .map(|uid| uid == &context.requester_user_id)
                .unwrap_or(false),
            "group_members" => {
                memory_scope
                    .get("group_id")
                    .map(|gid| gid == &context.group_id)
                    .unwrap_or(false)
                    || context.read_scope == "group"
            }
            "public" => true,
            _ => false,
        }
    }

    /// Check if memory owner matches the requesting user
    fn _owner_matches(&self, memory_user_id: &str, context: &MemoryAccessContext) -> bool {
        memory_user_id == context.requester_user_id
    }
}

impl Default for MemoryAccessPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_item(content: &str, importance: f64, mt: MemoryType) -> MemoryItem {
        MemoryItem {
            id: format!("id_{}", content),
            user_id: "u1".into(),
            content: content.into(),
            memory_type: mt,
            importance,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[test]
    fn test_filter_accessible_group_blocks_opinion() {
        let policy = MemoryAccessPolicy::new();
        let items = vec![
            make_item("事实A", 0.8, MemoryType::Fact),
            make_item("观点B", 0.7, MemoryType::Opinion),
        ];
        let filtered = policy.filter_accessible(&items, &ChatScope::Group("g1".into()));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].content, "事实A");
    }

    #[test]
    fn test_filter_accessible_private_allows_all() {
        let policy = MemoryAccessPolicy::new();
        let items = vec![
            make_item("事实A", 0.8, MemoryType::Fact),
            make_item("观点B", 0.7, MemoryType::Opinion),
            make_item("关系C", 0.6, MemoryType::Relationship),
        ];
        let filtered = policy.filter_accessible(&items, &ChatScope::Private);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_dedupe_entries() {
        let items = vec![
            make_item("一样的内容", 0.5, MemoryType::Fact),
            make_item("一样的内容", 0.9, MemoryType::Fact),
        ];
        let deduped = MemoryAccessPolicy::dedupe_entries(&items);
        assert_eq!(deduped.len(), 1);
        assert!((deduped[0].importance - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_build_context() {
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert_eq!(ctx.requester_user_id, "userA");
        assert_eq!(ctx.message_type, "group");
        assert_eq!(ctx.group_id, "g1");
        assert_eq!(ctx.read_scope, "group");
        assert_eq!(ctx.platform, "qq");
        assert_eq!(ctx.hour_of_day, 14);
    }

    #[test]
    fn test_normalize_category_addressing() {
        let policy = MemoryAccessPolicy::new();
        let result = policy._normalize_category("请叫我小明", None);
        assert_eq!(result, "addressing_preference");

        let result2 = policy._normalize_category("你可以称呼我老王", None);
        assert_eq!(result2, "addressing_preference");
    }

    #[test]
    fn test_normalize_category_group_rule_via_tags() {
        let policy = MemoryAccessPolicy::new();
        let meta = serde_json::json!({"tags": ["group_rule", "important"]});
        let result = policy._normalize_category("某规则", Some(&meta));
        assert_eq!(result, "group_rule");
    }

    #[test]
    fn test_normalize_category_bot_rule_via_tags() {
        let policy = MemoryAccessPolicy::new();
        let meta = serde_json::json!({"tags": ["bot_rule"]});
        let result = policy._normalize_category("某规则", Some(&meta));
        assert_eq!(result, "bot_rule");
    }

    #[test]
    fn test_normalize_category_public_rule_via_tags() {
        let policy = MemoryAccessPolicy::new();
        let meta = serde_json::json!({"tags": ["public_rule"]});
        let result = policy._normalize_category("某规则", Some(&meta));
        assert_eq!(result, "public_rule");
    }

    #[test]
    fn test_normalize_category_from_content_category() {
        let policy = MemoryAccessPolicy::new();
        let meta = serde_json::json!({"content_category": "event"});
        let result = policy._normalize_category("某事发生", Some(&meta));
        assert_eq!(result, "event");
    }

    #[test]
    fn test_normalize_category_generic() {
        let policy = MemoryAccessPolicy::new();
        let result = policy._normalize_category("今天天气不错", None);
        assert_eq!(result, "generic");
    }

    #[test]
    fn test_normalize_visibility_addressing() {
        let policy = MemoryAccessPolicy::new();
        let result = policy._normalize_visibility("addressing_preference");
        assert_eq!(result, MemoryVisibility::Private);
    }

    #[test]
    fn test_normalize_visibility_group_rule() {
        let policy = MemoryAccessPolicy::new();
        let result = policy._normalize_visibility("group_rule");
        assert_eq!(result, MemoryVisibility::Shared);
    }

    #[test]
    fn test_normalize_visibility_bot_rule() {
        let policy = MemoryAccessPolicy::new();
        let result = policy._normalize_visibility("bot_rule");
        assert_eq!(result, MemoryVisibility::Shared);
    }

    #[test]
    fn test_normalize_visibility_public_rule() {
        let policy = MemoryAccessPolicy::new();
        let result = policy._normalize_visibility("public_rule");
        assert_eq!(result, MemoryVisibility::Shared);
    }

    #[test]
    fn test_normalize_memory_record_addressing() {
        let policy = MemoryAccessPolicy::new();
        let record = serde_json::json!({
            "content": "请叫我杰克",
            "user_id": "userA",
            "importance": 0.8
        });
        let result = policy.normalize_memory_record(&record);
        assert_eq!(
            result["content_category"].as_str().unwrap(),
            "addressing_preference"
        );
        assert_eq!(result["visibility"].as_str().unwrap(), "private");
        let scope = result["applicability_scope"].as_object().unwrap();
        assert_eq!(scope["kind"].as_str().unwrap(), "self_only");
        assert_eq!(scope.get("user_id").and_then(|v| v.as_str()), Some("userA"));
    }

    #[test]
    fn test_normalize_memory_record_group_rule() {
        let policy = MemoryAccessPolicy::new();
        let record = serde_json::json!({
            "content": "群规：禁止广告",
            "group_id": "g123",
            "metadata": {"tags": ["group_rule"]}
        });
        let result = policy.normalize_memory_record(&record);
        assert_eq!(result["content_category"].as_str().unwrap(), "group_rule");
        assert_eq!(result["visibility"].as_str().unwrap(), "shared");
        let scope = result["applicability_scope"].as_object().unwrap();
        assert_eq!(scope["kind"].as_str().unwrap(), "group_members");
        assert_eq!(scope.get("group_id").and_then(|v| v.as_str()), Some("g123"));
    }

    #[test]
    fn test_scope_matches_self_only_match() {
        let policy = MemoryAccessPolicy::new();
        let mut scope = HashMap::new();
        scope.insert("kind".to_string(), "self_only".to_string());
        scope.insert("user_id".to_string(), "userA".to_string());
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert!(policy._scope_matches(&scope, &ctx));
    }

    #[test]
    fn test_scope_matches_self_only_no_match() {
        let policy = MemoryAccessPolicy::new();
        let mut scope = HashMap::new();
        scope.insert("kind".to_string(), "self_only".to_string());
        scope.insert("user_id".to_string(), "userB".to_string());
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert!(!policy._scope_matches(&scope, &ctx));
    }

    #[test]
    fn test_scope_matches_public() {
        let policy = MemoryAccessPolicy::new();
        let mut scope = HashMap::new();
        scope.insert("kind".to_string(), "public".to_string());
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert!(policy._scope_matches(&scope, &ctx));
    }

    #[test]
    fn test_scope_matches_group_members_match() {
        let policy = MemoryAccessPolicy::new();
        let mut scope = HashMap::new();
        scope.insert("kind".to_string(), "group_members".to_string());
        scope.insert("group_id".to_string(), "g1".to_string());
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert!(policy._scope_matches(&scope, &ctx));
    }

    #[test]
    fn test_scope_matches_direct_users_match() {
        let policy = MemoryAccessPolicy::new();
        let mut scope = HashMap::new();
        scope.insert("kind".to_string(), "direct_users".to_string());
        scope.insert("user_id".to_string(), "userA".to_string());
        let ctx = MemoryAccessPolicy::build_context("userA", "private", "", "user", "qq", 14);
        assert!(policy._scope_matches(&scope, &ctx));
    }

    #[test]
    fn test_owner_matches_true() {
        let policy = MemoryAccessPolicy::new();
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert!(policy._owner_matches("userA", &ctx));
    }

    #[test]
    fn test_owner_matches_false() {
        let policy = MemoryAccessPolicy::new();
        let ctx = MemoryAccessPolicy::build_context("userA", "group", "g1", "group", "qq", 14);
        assert!(!policy._owner_matches("userB", &ctx));
    }

    #[test]
    fn test_memory_access_context_default() {
        let ctx = MemoryAccessContext::default();
        assert_eq!(ctx.requester_user_id, "");
        assert_eq!(ctx.message_type, "private");
        assert_eq!(ctx.group_id, "");
        assert_eq!(ctx.read_scope, "user");
        assert_eq!(ctx.platform, "");
        assert_eq!(ctx.hour_of_day, -1);
    }
}

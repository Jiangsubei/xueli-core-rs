/// 存储作用域与 Key 构建工具
///
/// 提供平台感知的 v3 格式存储用户 ID 构建函数，
/// 以及旧版 key 格式的迁移兼容工具。
///
/// 对应 Python 版 `src/core/storage_scope.py`
use crate::core::scope::ChatScope;

/// 归一化存储 key 的作用域标识符（群号、频道号等）
///
/// 处理如 "group:123"、"platform:group:123"、"shared:123" 等 key 格式，
/// 提取最后一个冒号后的部分作为作用域 ID。
pub fn normalize_scope_id(_platform: &str, _scope: &ChatScope, raw_id: &str) -> String {
    let raw = raw_id.trim();
    if raw.is_empty() {
        return String::new();
    }
    if !raw.contains(':') {
        return raw.to_string();
    }
    let parts: Vec<&str> = raw.split(':').filter(|p| !p.trim().is_empty()).collect();
    parts.last().map(|p| p.to_string()).unwrap_or_default()
}

/// 构建平台感知 v3 格式的存储用户 ID
///
/// 格式：
///   - GROUP：  `{platform}:group:{scope_id}:{user_id}`
///   - PRIVATE：`{platform}:private:{user_id}`
///   - 若 platform 为空则回退到原始 user_id
pub fn build_storage_user_id_v3(
    platform: &str,
    user_id: &str,
    scope: &ChatScope,
    scope_id: &str,
) -> String {
    let normalized_user_id = user_id.trim();
    if normalized_user_id.is_empty() {
        return String::new();
    }
    if platform.is_empty() {
        return normalized_user_id.to_string();
    }
    let scope_str = if scope.is_group() { "group" } else { "private" };
    if scope.is_group() && !scope_id.is_empty() {
        let normalized_scope_id = normalize_scope_id(platform, scope, scope_id);
        if !normalized_scope_id.is_empty() {
            return format!(
                "{}:{}:{}:{}",
                platform, scope_str, normalized_scope_id, normalized_user_id
            );
        }
    }
    format!("{}:{}:{}", platform, scope_str, normalized_user_id)
}

/// 构建平台感知的存储用户 ID（便捷入口）
///
/// 根据 message_type 和 group_id 推导 ChatScope，然后委托给 build_storage_user_id_v3。
pub fn build_storage_user_id(
    platform: &str,
    user_id: &str,
    message_type: &str,
    group_id: Option<&str>,
) -> String {
    if platform.is_empty() {
        tracing::warn!("[存储] build_storage_user_id 未传入 platform，降级为无前缀 user_id");
    }
    let scope = if message_type.trim().eq_ignore_ascii_case("group") {
        ChatScope::Group(String::new())
    } else {
        ChatScope::Private
    };
    let scope_id = if let Some(gid) = group_id {
        normalize_scope_id(platform, &scope, gid)
    } else {
        String::new()
    };
    build_storage_user_id_v3(platform, user_id, &scope, &scope_id)
}

/// 将旧版存储 key 转换为 v3 格式（用于迁移）
///
/// 示例：
///   "group:12345:user_xxx" → "napcat:group:12345:user_xxx"
///   "user_xxx"             → "napcat:private:user_xxx"
pub fn legacy_to_v3_key(legacy_key: &str, platform: &str) -> String {
    let key = legacy_key.trim();
    if key.is_empty() || platform.is_empty() {
        return key.to_string();
    }
    if key.starts_with("group:") {
        return format!("{}:{}", platform, key);
    }
    if !key.contains(':') {
        return format!("{}:private:{}", platform, key);
    }
    format!("{}:{}", platform, key)
}

/// 解析存储访问：优先 v3，回退旧版
pub fn resolve_storage_key(legacy: &str, v3: &str) -> String {
    if v3.is_empty() {
        legacy.to_string()
    } else {
        v3.to_string()
    }
}

/// 同时构建旧版和 v3 两种存储 key，返回 (legacy_key, v3_key)
///
/// 调用方应先尝试 v3，再回退旧版。
/// 若 platform 为空，两个 key 相同（原始 user_id）。
pub fn build_both_keys(
    platform: &str,
    user_id: &str,
    message_type: &str,
    group_id: Option<&str>,
    scope: Option<&ChatScope>,
) -> (String, String) {
    let legacy_key = build_storage_user_id(platform, user_id, message_type, group_id);
    if platform.is_empty() {
        return (legacy_key.clone(), legacy_key);
    }
    let effective_scope = scope.cloned().unwrap_or_else(|| {
        if message_type.trim().eq_ignore_ascii_case("group") {
            ChatScope::Group(String::new())
        } else {
            ChatScope::Private
        }
    });
    let scope_id = group_id.map(|gid| gid.to_string()).unwrap_or_default();
    let v3_key = build_storage_user_id_v3(platform, user_id, &effective_scope, &scope_id);
    (legacy_key, v3_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_storage_user_id_v3_private() {
        let scope = ChatScope::Private;
        let key = build_storage_user_id_v3("qq", "user_123", &scope, "");
        assert_eq!(key, "qq:private:user_123");
    }

    #[test]
    fn test_build_storage_user_id_v3_group() {
        let scope = ChatScope::Group("12345".to_string());
        let key = build_storage_user_id_v3("qq", "user_123", &scope, "12345");
        assert_eq!(key, "qq:group:12345:user_123");
    }

    #[test]
    fn test_build_storage_user_id_v3_empty_platform() {
        let scope = ChatScope::Private;
        let key = build_storage_user_id_v3("", "user_123", &scope, "");
        assert_eq!(key, "user_123");
    }

    #[test]
    fn test_build_storage_user_id_v3_empty_user_id() {
        let scope = ChatScope::Private;
        let key = build_storage_user_id_v3("qq", "", &scope, "");
        assert_eq!(key, "");
    }

    #[test]
    fn test_build_storage_user_id_v3_group_no_scope_id() {
        let scope = ChatScope::Group("".to_string());
        let key = build_storage_user_id_v3("qq", "user_123", &scope, "");
        assert_eq!(key, "qq:group:user_123");
    }

    #[test]
    fn test_normalize_scope_id_simple() {
        let scope = ChatScope::Group("".to_string());
        assert_eq!(normalize_scope_id("qq", &scope, "12345"), "12345");
    }

    #[test]
    fn test_normalize_scope_id_with_colon() {
        let scope = ChatScope::Group("".to_string());
        assert_eq!(normalize_scope_id("qq", &scope, "group:12345"), "12345");
    }

    #[test]
    fn test_normalize_scope_id_with_platform_prefix() {
        let scope = ChatScope::Group("".to_string());
        assert_eq!(normalize_scope_id("qq", &scope, "qq:group:12345"), "12345");
    }

    #[test]
    fn test_normalize_scope_id_empty() {
        let scope = ChatScope::Group("".to_string());
        assert_eq!(normalize_scope_id("qq", &scope, ""), "");
    }

    #[test]
    fn test_legacy_to_v3_key_group() {
        let key = legacy_to_v3_key("group:12345:user_xxx", "qq");
        assert_eq!(key, "qq:group:12345:user_xxx");
    }

    #[test]
    fn test_legacy_to_v3_key_private() {
        let key = legacy_to_v3_key("user_xxx", "qq");
        assert_eq!(key, "qq:private:user_xxx");
    }

    #[test]
    fn test_legacy_to_v3_key_empty() {
        let key = legacy_to_v3_key("", "qq");
        assert_eq!(key, "");
    }

    #[test]
    fn test_legacy_to_v3_key_no_platform() {
        let key = legacy_to_v3_key("user_xxx", "");
        assert_eq!(key, "user_xxx");
    }

    #[test]
    fn test_build_both_keys_private() {
        let (legacy, v3) = build_both_keys("qq", "user_123", "private", None, None);
        assert_eq!(legacy, "qq:private:user_123");
        assert_eq!(v3, "qq:private:user_123");
    }

    #[test]
    fn test_build_both_keys_group() {
        let (legacy, v3) = build_both_keys("qq", "user_123", "group", Some("12345"), None);
        assert!(legacy.contains("qq"));
        assert!(legacy.contains("user_123"));
        assert!(v3.contains("qq"));
        assert!(v3.contains("user_123"));
    }

    #[test]
    fn test_build_both_keys_no_platform() {
        let (legacy, v3) = build_both_keys("", "user_123", "private", None, None);
        assert_eq!(legacy, "user_123");
        assert_eq!(v3, "user_123");
    }

    #[test]
    fn test_resolve_storage_key_prefer_v3() {
        assert_eq!(resolve_storage_key("legacy", "v3"), "v3");
    }

    #[test]
    fn test_resolve_storage_key_fallback() {
        assert_eq!(resolve_storage_key("legacy", ""), "legacy");
    }

    #[test]
    fn test_build_storage_user_id_entry() {
        let key = build_storage_user_id("qq", "user_123", "group", Some("12345"));
        assert_eq!(key, "qq:group:12345:user_123");
    }
}

/// 信号载荷的简单内存 TTL 缓存
///
/// 对应 Python 版 `xueli/src/handlers/signals/cache.py`
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 缓存条目
struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

/// 信号缓存（带 TTL 的内存缓存）
pub struct SignalCache<T: Clone> {
    store: HashMap<String, CacheEntry<T>>,
}

impl<T: Clone> SignalCache<T> {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
        }
    }

    /// 获取缓存值（过期自动移除）
    pub fn get(&mut self, key: &str) -> Option<T> {
        let entry = self.store.get(key)?;
        if entry.expires_at < Instant::now() {
            self.store.remove(key);
            return None;
        }
        Some(entry.value.clone())
    }

    /// 设置缓存值
    pub fn set(&mut self, key: &str, value: T, ttl: Duration) {
        let ttl = if ttl.as_secs_f64() < 0.1 {
            Duration::from_millis(100)
        } else {
            ttl
        };
        self.store.insert(
            key.to_string(),
            CacheEntry {
                value,
                expires_at: Instant::now() + ttl,
            },
        );
    }

    /// 取出并移除缓存值
    pub fn pop(&mut self, key: &str) -> Option<T> {
        let entry = self.store.remove(key)?;
        if entry.expires_at < Instant::now() {
            return None;
        }
        Some(entry.value)
    }

    /// 清理所有过期条目
    pub fn cleanup(&mut self) {
        let now = Instant::now();
        self.store.retain(|_, v| v.expires_at >= now);
    }

    /// 当前缓存条目数
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

impl<T: Clone> Default for SignalCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let mut cache = SignalCache::new();
        cache.set("key1", "value1", Duration::from_secs(60));
        assert_eq!(cache.get("key1"), Some("value1"));
    }

    #[test]
    fn test_expired_entry() {
        let mut cache = SignalCache::new();
        cache.set("key1", "value1", Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(cache.get("key1"), None);
    }

    #[test]
    fn test_pop() {
        let mut cache = SignalCache::new();
        cache.set("key1", "value1", Duration::from_secs(60));
        assert_eq!(cache.pop("key1"), Some("value1"));
        assert_eq!(cache.get("key1"), None);
    }

    #[test]
    fn test_cleanup() {
        let mut cache = SignalCache::new();
        cache.set("expired", "v1", Duration::from_millis(100));
        cache.set("valid", "v2", Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(150));
        cache.cleanup();
        assert_eq!(cache.get("expired"), None);
        assert_eq!(cache.get("valid"), Some("v2"));
    }

    #[test]
    fn test_minimum_ttl() {
        let mut cache = SignalCache::new();
        cache.set("key", "value", Duration::from_millis(1)); // < 100ms，自动升级到 100ms
        std::thread::sleep(Duration::from_millis(5));
        // 应该还在，因为 TTL 被提升到了 100ms
        assert_eq!(cache.get("key"), Some("value"));
    }

    #[test]
    fn test_default() {
        let cache: SignalCache<String> = SignalCache::default();
        assert!(cache.is_empty());
    }
}

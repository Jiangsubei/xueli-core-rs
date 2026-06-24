use std::sync::Arc;
use tempfile::TempDir;

use xueli_core::proactive_share::store::ProactiveShareStore;

fn setup_store() -> (TempDir, Arc<ProactiveShareStore>) {
    let dir = TempDir::new().expect("创建临时目录失败");
    let store = Arc::new(ProactiveShareStore::new(dir.path().to_str().unwrap()));
    (dir, store)
}

#[test]
fn test_add_and_count_shares() {
    let (_dir, store) = setup_store();
    store
        .add_share("测试分享内容", "insight", 24.0, "user_1", "group_1")
        .expect("添加分享失败");
    let sent = store.count_sent_today();
    assert_eq!(sent, 0);
}

#[test]
fn test_mark_sent_and_cooldown() {
    let (_dir, store) = setup_store();
    let record = store
        .add_share("内容", "test", 24.0, "user_1", "")
        .expect("添加失败");
    let _ = store.mark_sent(&record.id);
    assert!(!store.is_global_cooldown_active());
    store.set_global_cooldown(1.0);
    assert!(store.is_global_cooldown_active());
}

#[test]
fn test_pending_shares_filter() {
    let (_dir, store) = setup_store();
    store
        .add_share("待发送分享", "insight", 24.0, "", "")
        .expect("添加失败");
    let pending = store.pending_shares_with_cooldown(5, 0.0, "00:00", "23:59");
    assert!(pending.is_ok());
    let items = pending.unwrap();
    assert!(!items.is_empty(), "应有待发送条目");
}

#[test]
fn test_persist_and_reload() {
    let dir = TempDir::new().expect("创建临时目录失败");
    let path = dir.path().to_str().unwrap();
    {
        let store = ProactiveShareStore::new(path);
        store
            .add_share("持久化测试", "test", 24.0, "user_1", "")
            .expect("添加失败");
    }
    {
        let store = ProactiveShareStore::new(path);
        let count = store.count_sent_today();
        assert_eq!(count, 0);
        let pending = store.pending_shares_with_cooldown(10, 0.0, "00:00", "23:59");
        assert!(pending.is_ok());
        assert!(!pending.unwrap().is_empty());
    }
}

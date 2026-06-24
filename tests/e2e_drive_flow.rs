mod common;

use std::sync::Arc;
use tokio::sync::Mutex;
use xueli_core::core::drive::engine::{DriveEngine, DriveEvent};
use xueli_core::core::drive::store::DriveStore;

fn make_drive_engine() -> Arc<Mutex<DriveEngine>> {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let store = DriveStore::new(dir.path().to_str().unwrap());
    Arc::new(Mutex::new(DriveEngine::new(store, "test", true)))
}

#[tokio::test]
async fn test_drive_event_message_processed() {
    let engine = make_drive_engine();
    {
        let mut e = engine.lock().await;
        e.load().await;
    }
    {
        let mut e = engine.lock().await;
        e.on_event(DriveEvent::MessageProcessed {
            user_id: "user_1".to_string(),
            message: "你好".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    }
    let ctx = {
        let e = engine.lock().await;
        e.get_drive_context("user_1")
    };
    assert!(ctx.affective.valence >= -1.0 && ctx.affective.valence <= 1.0);
    assert!(ctx.motivational.contains_key("social_drive"));
}

#[tokio::test]
async fn test_drive_event_disabled() {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let store = DriveStore::new(dir.path().to_str().unwrap());
    let engine = Arc::new(Mutex::new(DriveEngine::new(store, "test", false)));
    {
        let mut e = engine.lock().await;
        e.load().await;
    }
    {
        let mut e = engine.lock().await;
        e.on_event(DriveEvent::MessageProcessed {
            user_id: "user_1".to_string(),
            message: "hello".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    }
    let ctx = {
        let e = engine.lock().await;
        e.get_drive_context("user_1")
    };
    assert_eq!(ctx.affective.valence, 0.0);
}

#[tokio::test]
async fn test_drive_persist_and_reload() {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let _store = DriveStore::new(dir.path().to_str().unwrap());
    let user_id = "user_2";
    {
        let engine = Arc::new(Mutex::new(DriveEngine::new(
            DriveStore::new(dir.path().to_str().unwrap()),
            "test",
            true,
        )));
        {
            let mut e = engine.lock().await;
            e.load().await;
        }
        {
            let mut e = engine.lock().await;
            e.on_event(DriveEvent::MessageProcessed {
                user_id: user_id.to_string(),
                message: "happy news!".to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await;
        }
    }
    {
        let store2 = DriveStore::new(dir.path().to_str().unwrap());
        let engine2 = Arc::new(Mutex::new(DriveEngine::new(store2, "test", true)));
        let mut e = engine2.lock().await;
        e.load().await;
        let ctx = e.get_drive_context(user_id);
        assert!(ctx.affective.valence >= -1.0 && ctx.affective.valence <= 1.0, "valence should be in valid range");
        assert!(ctx.motivational.contains_key("social_drive"), "should have social_drive");
    }
}

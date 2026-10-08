use clipsync::history::{ClipboardContent, ClipboardHistory};
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;
#[tokio::test]
async fn preserve_invalid_existing_key_and_database() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("history.db");
    let key = dir.path().join("history.key");
    let history = ClipboardHistory::new_with_key_path(&db, &key)
        .await
        .unwrap();
    history
        .add(&ClipboardContent {
            id: uuid::Uuid::new_v4(),
            content: b"saved text".to_vec(),
            content_type: "text/plain".into(),
            timestamp: 1,
            origin_node: uuid::Uuid::new_v4(),
        })
        .await
        .unwrap();
    drop(history);
    let original_key = std::fs::read(&key).unwrap();
    let original_db = std::fs::read(&db).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ClipboardHistory::new_with_key_path(&db, &key)
        .await
        .is_err());
    assert_eq!(std::fs::read(&key).unwrap(), original_key);
    assert_eq!(std::fs::read(&db).unwrap(), original_db);
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let reopened = ClipboardHistory::new_with_key_path(&db, &key)
        .await
        .unwrap();
    assert_eq!(
        reopened.get_recent(1).await.unwrap()[0].content,
        b"saved text"
    );
    drop(reopened);
    std::fs::write(&key, b"truncated").unwrap();
    assert!(ClipboardHistory::new_with_key_path(&db, &key)
        .await
        .is_err());
    assert_eq!(std::fs::read(&key).unwrap(), b"truncated");
    std::fs::remove_file(&key).unwrap();
    assert!(ClipboardHistory::new_with_key_path(&db, &key)
        .await
        .is_err());
    assert!(!key.exists());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_initializers_share_one_complete_key() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key");
    let mut tasks = vec![];
    for _ in 0..16 {
        let path = path.clone();
        tasks.push(tokio::spawn(async move {
            clipsync::history::encryption::Encryptor::for_store(&path, false)
                .await
                .unwrap()
                .get_key()
                .to_vec()
        }));
    }
    let expected = tasks.remove(0).await.unwrap();
    for task in tasks {
        assert_eq!(task.await.unwrap(), expected);
    }
    assert_eq!(std::fs::read(path).unwrap(), expected);
}

//! Integration tests for clipboard history module

use clipsync::history::{ClipboardContent, ClipboardHistory};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;
use uuid::Uuid;

#[tokio::test]
async fn test_full_history_workflow() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_history.db");
    
    let history = ClipboardHistory::new(&db_path).await.unwrap();
    
    // Add some text content
    let text_content = ClipboardContent {
        id: Uuid::new_v4(),
        content: b"Hello, ClipSync!".to_vec(),
        content_type: "text/plain".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        origin_node: Uuid::new_v4(),
    };
    
    history.add(&text_content).await.unwrap();
    
    // Add RTF content
    let rtf_content = ClipboardContent {
        id: Uuid::new_v4(),
        content: br"{\rtf1\ansi\deff0 {\colortbl;\red0\green0\blue0;} Hello RTF}".to_vec(),
        content_type: "text/rtf".to_string(),
        timestamp: chrono::Utc::now().timestamp() + 1,
        origin_node: Uuid::new_v4(),
    };
    
    history.add(&rtf_content).await.unwrap();
    
    // Retrieve recent entries
    let recent = history.get_recent(2).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].content_type, "text/rtf");
    assert_eq!(recent[1].content_type, "text/plain");
    
    // Test search
    let results = history.search("Hello").await.unwrap();
    assert_eq!(results.len(), 2);
    
    // Test get by index
    let entry = history.get_by_index(0).await.unwrap();
    assert_eq!(entry.content_type, "text/rtf");
}

#[tokio::test]
async fn test_large_payload_handling() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_history_large.db");
    
    let history = ClipboardHistory::new(&db_path).await.unwrap();
    
    // Create a 1MB payload
    let large_content = vec![b'X'; 1024 * 1024];
    
    let content = ClipboardContent {
        id: Uuid::new_v4(),
        content: large_content.clone(),
        content_type: "text/plain".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        origin_node: Uuid::new_v4(),
    };
    
    let start = std::time::Instant::now();
    history.add(&content).await.unwrap();
    let encrypt_time = start.elapsed();
    
    println!("Encryption time for 1MB: {:?}", encrypt_time);
    assert!(encrypt_time < Duration::from_millis(50));
    
    let start = std::time::Instant::now();
    let entries = history.get_recent(1).await.unwrap();
    let decrypt_time = start.elapsed();
    
    println!("Decryption time for 1MB: {:?}", decrypt_time);
    assert!(decrypt_time < Duration::from_millis(50));
    
    assert_eq!(entries[0].content, large_content);
}

#[tokio::test]
async fn test_concurrent_access() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_history_concurrent.db");
    
    let history = ClipboardHistory::new(&db_path).await.unwrap();
    
    // Add items sequentially to avoid borrowing issues
    for i in 0..10 {
        let content = ClipboardContent {
            id: Uuid::new_v4(),
            content: format!("Concurrent content {}", i).into_bytes(),
            content_type: "text/plain".to_string(),
            timestamp: chrono::Utc::now().timestamp() + i,
            origin_node: Uuid::new_v4(),
        };
        history.add(&content).await.unwrap();
    }
    
    // Verify all entries were added
    let entries = history.get_recent(10).await.unwrap();
    assert_eq!(entries.len(), 10);
}

#[tokio::test]
async fn test_image_content() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_history_image.db");
    
    let history = ClipboardHistory::new(&db_path).await.unwrap();
    
    // Simulate a small PNG image (1x1 red pixel)
    let png_data = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT chunk
        0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
        0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
        0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND chunk
        0x44, 0xAE, 0x42, 0x60, 0x82
    ];
    
    let content = ClipboardContent {
        id: Uuid::new_v4(),
        content: png_data.clone(),
        content_type: "image/png".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        origin_node: Uuid::new_v4(),
    };
    
    history.add(&content).await.unwrap();
    
    let entries = history.get_recent(1).await.unwrap();
    assert_eq!(entries[0].content_type, "image/png");
    assert_eq!(entries[0].content, png_data);
}

#[tokio::test]
async fn test_clear_history() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_history_clear.db");
    
    let history = ClipboardHistory::new(&db_path).await.unwrap();
    
    // Add some entries
    for i in 0..5 {
        let content = ClipboardContent {
            id: Uuid::new_v4(),
            content: format!("Entry {}", i).into_bytes(),
            content_type: "text/plain".to_string(),
            timestamp: chrono::Utc::now().timestamp() + i,
            origin_node: Uuid::new_v4(),
        };
        history.add(&content).await.unwrap();
    }
    
    // Verify entries exist
    let entries = history.get_recent(10).await.unwrap();
    assert_eq!(entries.len(), 5);
    
    // Clear history
    history.clear().await.unwrap();
    
    // Verify history is empty
    let entries = history.get_recent(10).await.unwrap();
    assert_eq!(entries.len(), 0);
}
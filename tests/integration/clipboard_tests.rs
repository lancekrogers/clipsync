//! Integration tests for clipboard functionality

use clipsync::clipboard::{create_provider, ClipboardContent, MAX_CLIPBOARD_SIZE};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
#[cfg(feature = "integration-tests")]
async fn test_clipboard_text_roundtrip() {
    let provider = create_provider().await.unwrap();
    
    // Set text
    let content = ClipboardContent::text("Integration test text");
    provider.set_content(&content).await.unwrap();
    
    // Get text back
    let retrieved = provider.get_content().await.unwrap();
    assert_eq!(retrieved.as_text(), Some("Integration test text".to_string()));
}

#[tokio::test]
#[cfg(feature = "integration-tests")]
async fn test_clipboard_clear() {
    let provider = create_provider().await.unwrap();
    
    // Set some content
    let content = ClipboardContent::text("Will be cleared");
    provider.set_content(&content).await.unwrap();
    
    // Clear clipboard
    provider.clear().await.unwrap();
    
    // Try to get content (should be empty or error)
    match provider.get_content().await {
        Ok(content) => {
            // Some platforms return empty string instead of error
            assert!(content.data.is_empty() || content.as_text() == Some("".to_string()));
        }
        Err(_) => {
            // This is also acceptable - no content
        }
    }
}

#[tokio::test]
#[cfg(feature = "integration-tests")]
async fn test_clipboard_size_limit() {
    let provider = create_provider().await.unwrap();
    
    // Create content larger than limit
    let large_data = vec![b'A'; MAX_CLIPBOARD_SIZE + 1];
    let content = ClipboardContent {
        mime_type: "text/plain".to_string(),
        data: large_data,
        timestamp: 0,
    };
    
    // Should fail with TooLarge error
    let result = provider.set_content(&content).await;
    assert!(matches!(result, Err(clipsync::clipboard::ClipboardError::TooLarge { .. })));
}

#[tokio::test]
#[cfg(feature = "integration-tests")]
async fn test_clipboard_watch() {
    let provider = create_provider().await.unwrap();
    let mut watcher = provider.watch().await.unwrap();
    
    // Set content after starting watcher
    let content = ClipboardContent::text("Watch test");
    provider.set_content(&content).await.unwrap();
    
    // Should receive event within timeout
    let result = timeout(Duration::from_secs(2), watcher.receiver.recv()).await;
    
    match result {
        Ok(Some(event)) => {
            assert_eq!(event.content.as_text(), Some("Watch test".to_string()));
        }
        _ => panic!("Did not receive clipboard event"),
    }
}

#[tokio::test]
async fn test_clipboard_content_creation() {
    // Test text content
    let text = ClipboardContent::text("Hello");
    assert_eq!(text.mime_type, "text/plain");
    assert_eq!(text.as_text(), Some("Hello".to_string()));
    assert!(text.is_text());
    assert!(!text.is_image());
    
    // Test RTF content
    let rtf_data = b"{\\rtf1 Hello}".to_vec();
    let rtf = ClipboardContent::rtf(rtf_data.clone());
    assert_eq!(rtf.mime_type, "text/rtf");
    assert_eq!(rtf.data, rtf_data);
    assert!(rtf.is_text());
    
    // Test image content
    let image_data = vec![0xFF, 0xD8, 0xFF]; // JPEG header
    let image = ClipboardContent::image(image_data.clone(), "jpeg");
    assert_eq!(image.mime_type, "image/jpeg");
    assert_eq!(image.data, image_data);
    assert!(image.is_image());
    assert!(!image.is_text());
}

#[test]
fn test_clipboard_content_size() {
    let content = ClipboardContent::text("12345");
    assert_eq!(content.size(), 5);
    
    let large_content = ClipboardContent::text("A".repeat(1000));
    assert_eq!(large_content.size(), 1000);
}
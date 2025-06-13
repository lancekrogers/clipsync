//! Clipboard abstraction layer for cross-platform clipboard operations
//!
//! This module provides a platform-agnostic interface for clipboard operations
//! with implementations for macOS, X11, and Wayland.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::mpsc;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod x11;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod wayland;

/// Maximum clipboard content size (5MB)
pub const MAX_CLIPBOARD_SIZE: usize = 5 * 1024 * 1024;

/// Clipboard content with metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipboardContent {
    /// MIME type of the content
    pub mime_type: String,
    /// The actual content data
    pub data: Vec<u8>,
    /// Timestamp when the content was captured (Unix timestamp)
    pub timestamp: i64,
}

impl ClipboardContent {
    /// Create new text content
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            mime_type: "text/plain".to_string(),
            data: text.into_bytes(),
            timestamp: current_timestamp(),
        }
    }

    /// Create new RTF content
    pub fn rtf(data: Vec<u8>) -> Self {
        Self {
            mime_type: "text/rtf".to_string(),
            data,
            timestamp: current_timestamp(),
        }
    }

    /// Create new image content
    pub fn image(data: Vec<u8>, format: &str) -> Self {
        Self {
            mime_type: format!("image/{}", format),
            data,
            timestamp: current_timestamp(),
        }
    }

    /// Get content as text if possible
    pub fn as_text(&self) -> Option<String> {
        if self.mime_type == "text/plain" {
            String::from_utf8(self.data.clone()).ok()
        } else {
            None
        }
    }

    /// Check if content is text
    pub fn is_text(&self) -> bool {
        self.mime_type.starts_with("text/")
    }

    /// Check if content is image
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    /// Get size of content in bytes
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

/// Clipboard provider trait
#[async_trait]
pub trait ClipboardProvider: Send + Sync {
    /// Get current clipboard content
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError>;

    /// Set clipboard content
    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError>;

    /// Clear clipboard
    async fn clear(&self) -> Result<(), ClipboardError>;

    /// Get provider name
    fn name(&self) -> &str;

    /// Start watching for clipboard changes
    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError>;
}

/// Clipboard selection type (mainly for X11)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSelection {
    /// Primary selection (middle-click on Linux)
    Primary,
    /// Clipboard selection (Ctrl+C/V)
    Clipboard,
}

/// Clipboard change event
#[derive(Debug, Clone)]
pub struct ClipboardEvent {
    /// The new clipboard content
    pub content: ClipboardContent,
    /// Which selection changed (if applicable)
    pub selection: Option<ClipboardSelection>,
}

/// Clipboard watcher for monitoring changes
pub struct ClipboardWatcher {
    /// Channel receiver for clipboard events
    pub receiver: mpsc::Receiver<ClipboardEvent>,
    /// Handle to stop watching
    _handle: Box<dyn Send + Sync>,
}

impl ClipboardWatcher {
    /// Create a new watcher with the given receiver
    pub fn new(
        receiver: mpsc::Receiver<ClipboardEvent>,
        handle: impl Send + Sync + 'static,
    ) -> Self {
        Self {
            receiver,
            _handle: Box::new(handle),
        }
    }
}

/// Clipboard errors
#[derive(Debug, Error)]
pub enum ClipboardError {
    /// Platform-specific error
    #[error("Platform error: {0}")]
    Platform(String),

    /// Content too large
    #[error("Content too large: {size} bytes (max: {max} bytes)")]
    TooLarge { size: usize, max: usize },

    /// Unsupported content type
    #[error("Unsupported content type: {0}")]
    UnsupportedType(String),

    /// No content available
    #[error("No clipboard content available")]
    NoContent,

    /// Watch error
    #[error("Failed to watch clipboard: {0}")]
    WatchError(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Create a clipboard provider for the current platform
pub async fn create_provider() -> Result<Box<dyn ClipboardProvider>, ClipboardError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacOSClipboard::new()?))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Try Wayland first, fall back to X11
        match std::env::var("WAYLAND_DISPLAY") {
            Ok(_) => {
                match wayland::WaylandClipboard::new().await {
                    Ok(provider) => Ok(Box::new(provider)),
                    Err(_) => {
                        // Fall back to X11
                        Ok(Box::new(x11::X11Clipboard::new()?))
                    }
                }
            }
            Err(_) => {
                // Use X11
                Ok(Box::new(x11::X11Clipboard::new()?))
            }
        }
    }

    #[cfg(not(any(target_os = "macos", unix)))]
    {
        Err(ClipboardError::Platform("Unsupported platform".to_string()))
    }
}

/// Get current timestamp
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_content_text() {
        let content = ClipboardContent::text("Hello, world!");
        assert_eq!(content.mime_type, "text/plain");
        assert_eq!(content.as_text(), Some("Hello, world!".to_string()));
        assert!(content.is_text());
        assert!(!content.is_image());
    }

    #[test]
    fn test_clipboard_content_image() {
        let content = ClipboardContent::image(vec![1, 2, 3], "png");
        assert_eq!(content.mime_type, "image/png");
        assert!(content.is_image());
        assert!(!content.is_text());
        assert_eq!(content.size(), 3);
    }

    #[test]
    fn test_clipboard_content_size() {
        let large_data = vec![0u8; 1024 * 1024]; // 1MB
        let content = ClipboardContent::rtf(large_data);
        assert_eq!(content.size(), 1024 * 1024);
    }
}

//! X11 clipboard implementation

use super::{
    ClipboardContent, ClipboardError, ClipboardEvent, ClipboardProvider, ClipboardSelection,
    ClipboardWatcher, MAX_CLIPBOARD_SIZE,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use x11_clipboard::Clipboard as X11ClipboardLib;

/// X11 clipboard provider
pub struct X11Clipboard {
    clipboard: Arc<X11ClipboardLib>,
}

impl X11Clipboard {
    /// Create a new X11 clipboard provider
    pub fn new() -> Result<Self, ClipboardError> {
        let clipboard = X11ClipboardLib::new()
            .map_err(|e| ClipboardError::Platform(format!("Failed to connect to X11: {}", e)))?;

        Ok(Self {
            clipboard: Arc::new(clipboard),
        })
    }

    /// Read content from a specific selection
    fn read_selection(
        &self,
        selection: ClipboardSelection,
    ) -> Result<Option<Vec<u8>>, ClipboardError> {
        let atom = match selection {
            ClipboardSelection::Primary => self.clipboard.setter.atoms.primary,
            ClipboardSelection::Clipboard => self.clipboard.setter.atoms.clipboard,
        };

        // Load clipboard content with a timeout
        let result = self
            .clipboard
            .load(
                atom,
                self.clipboard.setter.atoms.utf8_string,
                self.clipboard.setter.atoms.property,
                Duration::from_millis(500),
            )
            .map_err(|e| ClipboardError::Platform(format!("Failed to read clipboard: {}", e)))?;

        if result.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result))
        }
    }

    /// Write content to a specific selection
    fn write_selection(
        &self,
        selection: ClipboardSelection,
        data: &[u8],
    ) -> Result<(), ClipboardError> {
        let atom = match selection {
            ClipboardSelection::Primary => self.clipboard.setter.atoms.primary,
            ClipboardSelection::Clipboard => self.clipboard.setter.atoms.clipboard,
        };

        self.clipboard
            .store(atom, self.clipboard.setter.atoms.utf8_string, data)
            .map_err(|e| ClipboardError::Platform(format!("Failed to write clipboard: {}", e)))?;

        Ok(())
    }

    /// Try to detect content type from data
    fn detect_content_type(data: &[u8]) -> String {
        // Simple heuristic: if it's valid UTF-8, it's text
        if std::str::from_utf8(data).is_ok() {
            "text/plain".to_string()
        } else {
            // Could be improved with more sophisticated detection
            "application/octet-stream".to_string()
        }
    }
}

#[async_trait]
impl ClipboardProvider for X11Clipboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        // Try clipboard selection first, then primary
        if let Some(data) = self.read_selection(ClipboardSelection::Clipboard)? {
            let mime_type = Self::detect_content_type(&data);

            if mime_type == "text/plain" {
                let text = String::from_utf8(data).map_err(|_| {
                    ClipboardError::Platform("Invalid UTF-8 in clipboard".to_string())
                })?;
                return Ok(ClipboardContent::text(text));
            } else {
                return Ok(ClipboardContent {
                    mime_type,
                    data,
                    timestamp: super::current_timestamp(),
                });
            }
        }

        // Try primary selection
        if let Some(data) = self.read_selection(ClipboardSelection::Primary)? {
            let mime_type = Self::detect_content_type(&data);

            if mime_type == "text/plain" {
                let text = String::from_utf8(data).map_err(|_| {
                    ClipboardError::Platform("Invalid UTF-8 in clipboard".to_string())
                })?;
                return Ok(ClipboardContent::text(text));
            } else {
                return Ok(ClipboardContent {
                    mime_type,
                    data,
                    timestamp: super::current_timestamp(),
                });
            }
        }

        Err(ClipboardError::NoContent)
    }

    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        // Check size limit
        if content.size() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge {
                size: content.size(),
                max: MAX_CLIPBOARD_SIZE,
            });
        }

        // For now, only support text content on X11
        if !content.is_text() {
            return Err(ClipboardError::UnsupportedType(content.mime_type.clone()));
        }

        // Write to both clipboard and primary selection
        self.write_selection(ClipboardSelection::Clipboard, &content.data)?;
        self.write_selection(ClipboardSelection::Primary, &content.data)?;

        Ok(())
    }

    async fn clear(&self) -> Result<(), ClipboardError> {
        self.write_selection(ClipboardSelection::Clipboard, b"")?;
        self.write_selection(ClipboardSelection::Primary, b"")?;
        Ok(())
    }

    fn name(&self) -> &str {
        "X11"
    }

    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        let (tx, rx) = mpsc::channel(100);
        let clipboard = Arc::clone(&self.clipboard);

        let handle = tokio::spawn(async move {
            let mut last_clipboard_content: Option<Vec<u8>> = None;
            let mut last_primary_content: Option<Vec<u8>> = None;
            let mut ticker = interval(Duration::from_millis(200));

            // Helper to create a temporary X11Clipboard
            let temp_clipboard = match X11Clipboard::new() {
                Ok(c) => c,
                Err(_) => return,
            };

            loop {
                ticker.tick().await;

                // Check clipboard selection
                if let Ok(Some(data)) = temp_clipboard.read_selection(ClipboardSelection::Clipboard)
                {
                    if last_clipboard_content.as_ref() != Some(&data) {
                        last_clipboard_content = Some(data.clone());

                        let mime_type = X11Clipboard::detect_content_type(&data);
                        let content = if mime_type == "text/plain" {
                            if let Ok(text) = String::from_utf8(data) {
                                ClipboardContent::text(text)
                            } else {
                                continue;
                            }
                        } else {
                            ClipboardContent {
                                mime_type,
                                data,
                                timestamp: super::current_timestamp(),
                            }
                        };

                        let event = ClipboardEvent {
                            content,
                            selection: Some(ClipboardSelection::Clipboard),
                        };

                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                }

                // Check primary selection
                if let Ok(Some(data)) = temp_clipboard.read_selection(ClipboardSelection::Primary) {
                    if last_primary_content.as_ref() != Some(&data) {
                        last_primary_content = Some(data.clone());

                        let mime_type = X11Clipboard::detect_content_type(&data);
                        let content = if mime_type == "text/plain" {
                            if let Ok(text) = String::from_utf8(data) {
                                ClipboardContent::text(text)
                            } else {
                                continue;
                            }
                        } else {
                            ClipboardContent {
                                mime_type,
                                data,
                                timestamp: super::current_timestamp(),
                            }
                        };

                        let event = ClipboardEvent {
                            content,
                            selection: Some(ClipboardSelection::Primary),
                        };

                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(ClipboardWatcher::new(rx, handle))
    }
}

// X11Clipboard is Send + Sync
unsafe impl Send for X11Clipboard {}
unsafe impl Sync for X11Clipboard {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_x11_clipboard_name() {
        if let Ok(clipboard) = X11Clipboard::new() {
            assert_eq!(clipboard.name(), "X11");
        }
    }

    #[tokio::test]
    async fn test_x11_clipboard_text() {
        // This test will only work on systems with X11
        if std::env::var("DISPLAY").is_err() {
            return;
        }

        if let Ok(clipboard) = X11Clipboard::new() {
            // Set text
            let content = ClipboardContent::text("Hello from X11!");
            clipboard.set_content(&content).await.unwrap();

            // Get text
            let retrieved = clipboard.get_content().await.unwrap();
            assert_eq!(retrieved.as_text(), Some("Hello from X11!".to_string()));
        }
    }
}

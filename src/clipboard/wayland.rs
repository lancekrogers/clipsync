//! Linux clipboard access through arboard's maintained Wayland data-control backend.
//! Compositors without data-control may fall back to X11 through arboard.
use super::{
    ClipboardContent, ClipboardError, ClipboardEvent, ClipboardProvider, ClipboardWatcher,
    MAX_CLIPBOARD_SIZE,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct WaylandClipboard {
    clipboard: Arc<Mutex<arboard::Clipboard>>,
}
fn error(e: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Platform(e.to_string())
}
impl WaylandClipboard {
    pub async fn new() -> Result<Self, ClipboardError> {
        let clipboard = tokio::task::spawn_blocking(arboard::Clipboard::new)
            .await
            .map_err(error)?
            .map_err(error)?;
        Ok(Self {
            clipboard: Arc::new(Mutex::new(clipboard)),
        })
    }
}
struct WatchTask(tokio::task::JoinHandle<()>);
impl Drop for WatchTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[async_trait]
impl ClipboardProvider for WaylandClipboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        let clipboard = self.clipboard.clone();
        let text = tokio::task::spawn_blocking(move || {
            clipboard.lock().map_err(error)?.get_text().map_err(error)
        })
        .await
        .map_err(error)??;
        if text.len() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge {
                size: text.len(),
                max: MAX_CLIPBOARD_SIZE,
            });
        }
        Ok(ClipboardContent::text(text))
    }
    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        if content.data.len() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge {
                size: content.data.len(),
                max: MAX_CLIPBOARD_SIZE,
            });
        }
        let text = content
            .as_text()
            .ok_or_else(|| ClipboardError::UnsupportedType(content.mime_type.clone()))?;
        let clipboard = self.clipboard.clone();
        tokio::task::spawn_blocking(move || {
            clipboard
                .lock()
                .map_err(error)?
                .set_text(text)
                .map_err(error)
        })
        .await
        .map_err(error)?
    }
    async fn clear(&self) -> Result<(), ClipboardError> {
        let clipboard = self.clipboard.clone();
        tokio::task::spawn_blocking(move || clipboard.lock().map_err(error)?.clear().map_err(error))
            .await
            .map_err(error)?
    }
    fn name(&self) -> &str {
        "Linux (Wayland data-control / X11)"
    }
    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        let clipboard = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let task = tokio::spawn(async move {
            let mut last = None;
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                interval.tick().await;
                if let Ok(content) = clipboard.get_content().await {
                    if last.as_ref() != Some(&content.data) {
                        last = Some(content.data.clone());
                        if tx
                            .send(ClipboardEvent {
                                content,
                                selection: None,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        Ok(ClipboardWatcher::new(rx, WatchTask(task)))
    }
}

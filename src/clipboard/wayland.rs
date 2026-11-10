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
    max_size: usize,
    image_capture: Arc<std::sync::atomic::AtomicBool>,
    image_worker: Arc<tokio::sync::Semaphore>,
}
fn error(e: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Platform(e.to_string())
}
impl WaylandClipboard {
    pub async fn new() -> Result<Self, ClipboardError> {
        Self::new_with_limit(MAX_CLIPBOARD_SIZE).await
    }
    pub async fn new_with_limit(max_size: usize) -> Result<Self, ClipboardError> {
        let clipboard = tokio::task::spawn_blocking(arboard::Clipboard::new)
            .await
            .map_err(error)?
            .map_err(error)?;
        Ok(Self {
            clipboard: Arc::new(Mutex::new(clipboard)),
            max_size,
            image_capture: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            image_worker: Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }
    /// Read raw MIME bytes, never arboard's pre-decoding image API. The caller
    /// holds a single worker permit until this blocking operation has actually ended.
    fn read_png(max_size: usize) -> Result<Option<Vec<u8>>, ClipboardError> {
        use std::io::Read;
        use wl_clipboard_rs::paste::{self, ClipboardType, MimeType, Seat};
        let offered = match paste::get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
            Ok(types) => types,
            Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
            Err(err) => return Err(error(err)),
        };
        if offered.contains("text/uri-list") {
            return Err(ClipboardError::UnsupportedType("text/uri-list".into()));
        }
        let (mut reader, _) = match paste::get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Specific("image/png"),
        ) {
            Ok(result) => result,
            Err(paste::Error::NoMimeType | paste::Error::ClipboardEmpty) => return Ok(None),
            Err(err) => return Err(error(err)),
        };
        let mut result = Vec::new();
        let mut chunk = [0u8; 16384];
        loop {
            let remaining = max_size.saturating_sub(result.len());
            let allowed = remaining.saturating_add(1).min(chunk.len());
            let count = reader.read(&mut chunk[..allowed]).map_err(error)?;
            if count == 0 {
                return Ok(Some(result));
            }
            if count > remaining {
                return Err(ClipboardError::TooLarge {
                    size: max_size.saturating_add(1),
                    max: max_size,
                });
            }
            result.try_reserve_exact(count).map_err(error)?;
            result.extend_from_slice(&chunk[..count]);
        }
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
        if self
            .image_capture
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let permit = self.image_worker.clone().try_acquire_owned().map_err(|_| {
                ClipboardError::Platform("Wayland image read is still in progress".into())
            })?;
            let max = self.max_size;
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                Self::read_png(max)
            });
            let image = tokio::time::timeout(std::time::Duration::from_secs(3), task)
                .await
                .map_err(|_| ClipboardError::Platform("Wayland image read timed out".into()))?
                .map_err(error)??;
            if let Some(bytes) = image {
                return Ok(ClipboardContent::image(bytes, "png"));
            }
        }
        let clipboard = self.clipboard.clone();
        let text = tokio::task::spawn_blocking(move || {
            clipboard.lock().map_err(error)?.get_text().map_err(error)
        })
        .await
        .map_err(error)??;
        if text.len() > self.max_size {
            return Err(ClipboardError::TooLarge {
                size: text.len(),
                max: self.max_size,
            });
        }
        Ok(ClipboardContent::text(text))
    }
    fn enable_image_capture(&self) -> bool {
        self.image_capture
            .store(true, std::sync::atomic::Ordering::Relaxed);
        true
    }

    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        if content.data.len() > self.max_size {
            return Err(ClipboardError::TooLarge {
                size: content.data.len(),
                max: self.max_size,
            });
        }
        if content.mime_type == "image/png" {
            let permit = self.image_worker.clone().try_acquire_owned().map_err(|_| {
                ClipboardError::Platform("Wayland image operation is still in progress".into())
            })?;
            let bytes = content.data.clone();
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                use wl_clipboard_rs::copy::{MimeType, Options, Source};
                Options::new()
                    .copy(
                        Source::Bytes(bytes.into_boxed_slice()),
                        MimeType::Specific("image/png".into()),
                    )
                    .map_err(|e| {
                        ClipboardError::WriteOutcomeUnknown(format!("Wayland image write: {e}"))
                    })
            });
            return tokio::time::timeout(std::time::Duration::from_secs(3), task)
                .await
                .map_err(|_| {
                    ClipboardError::WriteOutcomeUnknown("Wayland image write timed out".into())
                })?
                .map_err(error)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires a disposable Wayland data-control session"]
    async fn raw_png_roundtrip_and_size_limit() {
        let writer = WaylandClipboard::new_with_limit(1024).await.unwrap();
        let reader = WaylandClipboard::new_with_limit(64).await.unwrap();
        assert!(reader.enable_image_capture());
        writer
            .set_content(&ClipboardContent::image(vec![7; 64], "png"))
            .await
            .unwrap();
        drop(writer);
        let content = reader.get_content().await.unwrap();
        assert_eq!(content.mime_type, "image/png");
        assert_eq!(content.data, vec![7; 64]);
        let writer = WaylandClipboard::new_with_limit(1024).await.unwrap();
        writer
            .set_content(&ClipboardContent::image(vec![8; 65], "png"))
            .await
            .unwrap();
        assert!(matches!(
            reader.get_content().await,
            Err(ClipboardError::TooLarge { max: 64, .. })
        ));
        tokio::task::spawn_blocking(|| {
            use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
            Options::new()
                .copy_multi(vec![
                    MimeSource {
                        source: Source::Bytes(vec![1, 2, 3].into_boxed_slice()),
                        mime_type: MimeType::Specific("image/png".into()),
                    },
                    MimeSource {
                        source: Source::Bytes(
                            b"file:///tmp/example.png".to_vec().into_boxed_slice(),
                        ),
                        mime_type: MimeType::Specific("text/uri-list".into()),
                    },
                ])
                .unwrap();
        })
        .await
        .unwrap();
        assert!(
            matches!(reader.get_content().await, Err(ClipboardError::UnsupportedType(kind)) if kind == "text/uri-list")
        );
        let permit = reader.image_worker.clone().try_acquire_owned().unwrap();
        assert!(reader
            .get_content()
            .await
            .unwrap_err()
            .to_string()
            .contains("still in progress"));
        drop(permit);
    }
}

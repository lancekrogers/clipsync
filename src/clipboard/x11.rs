//! X11 clipboard implementation

use super::{
    ClipboardContent, ClipboardError, ClipboardEvent, ClipboardProvider, ClipboardSelection,
    ClipboardWatcher, MAX_CLIPBOARD_SIZE,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::interval;
use x11_clipboard::Clipboard as X11ClipboardLib;
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{AtomEnum, ConnectionExt, Property},
        Event,
    },
    CURRENT_TIME,
};

/// X11 clipboard provider
pub struct X11Clipboard {
    clipboard: Arc<X11ClipboardLib>,
    max_size: usize,
    image_capture: std::sync::atomic::AtomicBool,
    image_worker: Arc<tokio::sync::Semaphore>,
}

impl X11Clipboard {
    /// Create a new X11 clipboard provider
    pub fn new() -> Result<Self, ClipboardError> {
        Self::new_with_limit(MAX_CLIPBOARD_SIZE)
    }

    pub fn new_with_limit(max_size: usize) -> Result<Self, ClipboardError> {
        let clipboard = X11ClipboardLib::new()
            .map_err(|e| ClipboardError::Platform(format!("Failed to connect to X11: {}", e)))?;

        Ok(Self {
            clipboard: Arc::new(clipboard),
            max_size,
            image_capture: std::sync::atomic::AtomicBool::new(false),
            image_worker: Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }

    fn clipboard_owner(clipboard: &X11ClipboardLib) -> Result<u32, ClipboardError> {
        clipboard
            .setter
            .connection
            .get_selection_owner(clipboard.setter.atoms.clipboard)
            .map_err(|e| ClipboardError::Platform(format!("X11 owner request: {e}")))?
            .reply()
            .map(|reply| reply.owner)
            .map_err(|e| ClipboardError::Platform(format!("X11 owner reply: {e}")))
    }

    /// Read an X11 PNG target with an encoded-byte bound, including INCR transfers.
    /// An independent getter connection prevents concurrent reads from stealing events.
    fn read_png(max_size: usize) -> Result<Option<Vec<u8>>, ClipboardError> {
        fn platform(e: impl std::fmt::Display) -> ClipboardError {
            ClipboardError::Platform(format!("X11 image read: {e}"))
        }
        let context = x11_clipboard::Context::new(None).map_err(platform)?;
        let target = context.get_atom("image/png").map_err(platform)?;
        let selection = context.atoms.clipboard;
        let property = context.atoms.property;
        // Do not turn a copied file into an image just because its owner also
        // supplies a thumbnail. Refuse missing or oversized TARGETS rather
        // than guessing which representations were present.
        let uri_list = context.get_atom("text/uri-list").map_err(platform)?;
        let targets = context.atoms.targets;
        let targets_cookie = context
            .connection
            .convert_selection(context.window, selection, targets, property, CURRENT_TIME)
            .map_err(platform)?;
        let targets_seq = targets_cookie.sequence_number();
        targets_cookie.check().map_err(platform)?;
        let targets_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() >= targets_deadline {
                return Err(ClipboardError::Platform(
                    "X11 targets read timed out".into(),
                ));
            }
            let Some((event, event_seq)) = context
                .connection
                .poll_for_event_with_sequence()
                .map_err(platform)?
            else {
                std::thread::sleep(Duration::from_micros(100));
                continue;
            };
            if event_seq < targets_seq {
                continue;
            }
            if let Event::SelectionNotify(event) = event {
                if event.selection != selection
                    || event.requestor != context.window
                    || event.target != targets
                {
                    continue;
                }
                if event.property == u32::from(AtomEnum::NONE) {
                    return Ok(None);
                }
                let reply = context
                    .connection
                    .get_property(true, context.window, property, AtomEnum::ANY, 0, 1024)
                    .map_err(platform)?
                    .reply()
                    .map_err(platform)?;
                if reply.type_ != u32::from(AtomEnum::ATOM)
                    || reply.format != 32
                    || reply.bytes_after != 0
                {
                    return Err(ClipboardError::Platform(
                        "Invalid or oversized X11 targets".into(),
                    ));
                }
                let offered: Vec<_> = reply
                    .value32()
                    .ok_or_else(|| platform("Invalid X11 targets"))?
                    .collect();
                if offered.contains(&uri_list) {
                    return Err(ClipboardError::UnsupportedType("text/uri-list".into()));
                }
                if !offered.contains(&target) {
                    return Ok(None);
                }
                break;
            }
        }
        let cookie = context
            .connection
            .convert_selection(context.window, selection, target, property, CURRENT_TIME)
            .map_err(platform)?;
        let seq = cookie.sequence_number();
        cookie.check().map_err(platform)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut incremental = false;
        let mut data = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err(ClipboardError::Platform("X11 image read timed out".into()));
            }
            let Some((event, event_seq)) = context
                .connection
                .poll_for_event_with_sequence()
                .map_err(platform)?
            else {
                std::thread::sleep(Duration::from_micros(100));
                continue;
            };
            if event_seq < seq {
                continue;
            }
            match event {
                Event::SelectionNotify(event)
                    if event.selection == selection && event.requestor == context.window =>
                {
                    if event.property == u32::from(AtomEnum::NONE) {
                        return Ok(None);
                    }
                    let words = u32::try_from(max_size.saturating_add(4) / 4).unwrap_or(u32::MAX);
                    let reply = context
                        .connection
                        .get_property(false, context.window, property, AtomEnum::ANY, 0, words)
                        .map_err(platform)?
                        .reply()
                        .map_err(platform)?;
                    if reply.type_ == context.atoms.incr {
                        let declared = reply
                            .value32()
                            .and_then(|mut values| values.next())
                            .unwrap_or(0) as usize;
                        if declared > max_size {
                            return Err(ClipboardError::TooLarge {
                                size: declared,
                                max: max_size,
                            });
                        }
                        context
                            .connection
                            .delete_property(context.window, property)
                            .map_err(platform)?
                            .check()
                            .map_err(platform)?;
                        incremental = true;
                    } else {
                        if reply.type_ != target {
                            return Err(ClipboardError::Platform(
                                "X11 image target mismatch".into(),
                            ));
                        }
                        if reply.bytes_after != 0 || reply.value.len() > max_size {
                            return Err(ClipboardError::TooLarge {
                                size: max_size.saturating_add(1),
                                max: max_size,
                            });
                        }
                        return Ok(Some(reply.value));
                    }
                }
                Event::PropertyNotify(event)
                    if incremental
                        && event.window == context.window
                        && event.atom == property
                        && event.state == Property::NEW_VALUE =>
                {
                    let remaining = max_size.saturating_sub(data.len());
                    let words = u32::try_from(remaining.saturating_add(4) / 4).unwrap_or(u32::MAX);
                    let reply = context
                        .connection
                        .get_property(true, context.window, property, AtomEnum::ANY, 0, words)
                        .map_err(platform)?
                        .reply()
                        .map_err(platform)?;
                    if reply.type_ != target {
                        return Err(ClipboardError::Platform(
                            "X11 image chunk type mismatch".into(),
                        ));
                    }
                    if reply.bytes_after != 0 || reply.value.len() > remaining {
                        return Err(ClipboardError::TooLarge {
                            size: max_size.saturating_add(1),
                            max: max_size,
                        });
                    }
                    if reply.value.is_empty() {
                        return Ok(Some(data));
                    }
                    data.extend_from_slice(&reply.value);
                }
                _ => {}
            }
        }
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
        if self
            .image_capture
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let max = self.max_size;
            let clipboard = self.clipboard.clone();
            let permit = self.image_worker.clone().try_acquire_owned().map_err(|_| {
                ClipboardError::Platform("X11 image operation is still in progress".into())
            })?;
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let before = Self::clipboard_owner(&clipboard)?;
                let content = Self::read_png(max)?;
                if before != Self::clipboard_owner(&clipboard)? {
                    return Err(ClipboardError::Ambiguous);
                }
                Ok(content)
            });
            if let Some(data) = tokio::time::timeout(Duration::from_secs(12), task)
                .await
                .map_err(|_| ClipboardError::Platform("X11 image read timed out".into()))?
                .map_err(|e| ClipboardError::Platform(format!("X11 image task: {e}")))??
            {
                return Ok(ClipboardContent::image(data, "png"));
            }
        }
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

    fn enable_image_capture(&self) -> bool {
        self.image_capture
            .store(true, std::sync::atomic::Ordering::Relaxed);
        true
    }

    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        // Check size limit
        if content.size() > self.max_size {
            return Err(ClipboardError::TooLarge {
                size: content.size(),
                max: self.max_size,
            });
        }

        if content.mime_type == "image/png" {
            let permit = self.image_worker.clone().try_acquire_owned().map_err(|_| {
                ClipboardError::Platform("X11 image operation is still in progress".into())
            })?;
            let clipboard = self.clipboard.clone();
            let bytes = content.data.clone();
            let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_cancelled = cancelled.clone();
            let task = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let target = clipboard
                    .setter
                    .get_atom("image/png")
                    .map_err(|e| ClipboardError::Platform(format!("X11 image target: {e}")))?;
                if worker_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(ClipboardError::Platform(
                        "X11 image write cancelled before selection change".into(),
                    ));
                }
                clipboard
                    .store(clipboard.setter.atoms.clipboard, target, bytes)
                    .map_err(|e| {
                        ClipboardError::WriteOutcomeUnknown(format!("X11 image write: {e}"))
                    })
            });
            return match tokio::time::timeout(Duration::from_secs(12), task).await {
                Ok(Ok(result)) => result,
                Ok(Err(err)) => Err(ClipboardError::WriteOutcomeUnknown(format!(
                    "X11 image task: {err}"
                ))),
                Err(_) => {
                    cancelled.store(true, std::sync::atomic::Ordering::Release);
                    Err(ClipboardError::WriteOutcomeUnknown(
                        "X11 image write timed out".into(),
                    ))
                }
            };
        }
        if !content.is_text() {
            return Err(ClipboardError::UnsupportedType(content.mime_type.clone()));
        }

        // Write to both clipboard and primary selection
        self.write_selection(ClipboardSelection::Clipboard, &content.data)?;
        self.write_selection(ClipboardSelection::Primary, &content.data)?;

        Ok(())
    }

    async fn reconcile_unknown_image(
        &self,
        expected: &ClipboardContent,
        observed: &ClipboardContent,
    ) -> Result<super::ImageAttribution, ClipboardError> {
        use super::ImageAttribution::{Foreign, Owned, Unknown};
        if expected.mime_type != "image/png" || observed.mime_type != "image/png" {
            return Ok(Unknown);
        }
        let permit = match self.image_worker.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return Ok(Unknown),
        };
        let clipboard = self.clipboard.clone();
        let max = self.max_size;
        let expected = expected.data.clone();
        let observed = observed.data.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let before = Self::clipboard_owner(&clipboard)?;
            let current = Self::read_png(max)?;
            let after = Self::clipboard_owner(&clipboard)?;
            if before == 0 || before != after || current.as_deref() != Some(observed.as_slice()) {
                return Ok(Unknown);
            }
            if before == clipboard.setter.window && observed == expected {
                Ok(Owned)
            } else {
                Ok(Foreign)
            }
        });
        match tokio::time::timeout(Duration::from_secs(12), task).await {
            Ok(Ok(result)) => result,
            _ => Ok(Unknown),
        }
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

    #[tokio::test]
    #[ignore = "Requires a disposable X11 clipboard session"]
    async fn copied_file_thumbnail_is_not_read_as_an_image() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use x11rb::protocol::xproto::{
            EventMask, PropMode, SelectionNotifyEvent, SELECTION_NOTIFY_EVENT,
        };
        use x11rb::wrapper::ConnectionExt as _;
        let owner = x11_clipboard::Context::new(None).unwrap();
        let png = owner.get_atom("image/png").unwrap();
        let uri = owner.get_atom("text/uri-list").unwrap();
        owner
            .connection
            .set_selection_owner(owner.window, owner.atoms.clipboard, CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let stop = done.clone();
        let requested_image = Arc::new(AtomicBool::new(false));
        let requested = requested_image.clone();
        let source = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match owner.connection.poll_for_event().unwrap() {
                    Some(Event::SelectionRequest(event)) => {
                        if event.target == owner.atoms.targets {
                            owner
                                .connection
                                .change_property32(
                                    PropMode::REPLACE,
                                    event.requestor,
                                    event.property,
                                    AtomEnum::ATOM,
                                    &[owner.atoms.targets, png, uri],
                                )
                                .unwrap();
                        } else if event.target == png {
                            requested.store(true, Ordering::Release);
                        }
                        owner
                            .connection
                            .send_event(
                                false,
                                event.requestor,
                                EventMask::NO_EVENT,
                                SelectionNotifyEvent {
                                    response_type: SELECTION_NOTIFY_EVENT,
                                    sequence: 0,
                                    time: event.time,
                                    requestor: event.requestor,
                                    selection: event.selection,
                                    target: event.target,
                                    property: event.property,
                                },
                            )
                            .unwrap();
                        owner.connection.flush().unwrap();
                    }
                    _ => std::thread::sleep(Duration::from_millis(1)),
                }
            }
        });
        let reader = X11Clipboard::new_with_limit(1024).unwrap();
        reader.image_capture.store(true, Ordering::Release);
        let result = reader.get_content().await;
        done.store(true, Ordering::Release);
        source.join().unwrap();
        assert!(
            matches!(result, Err(ClipboardError::UnsupportedType(kind)) if kind == "text/uri-list")
        );
        assert!(!requested_image.load(Ordering::Acquire));
    }

    #[test]
    fn test_x11_clipboard_name() {
        if let Ok(clipboard) = X11Clipboard::new() {
            assert_eq!(clipboard.name(), "X11");
        }
    }

    #[tokio::test]
    #[ignore = "Requires a disposable X11 clipboard session"]
    async fn image_owner_attribution_distinguishes_foreign_and_stale_snapshots() {
        let first = X11Clipboard::new_with_limit(1024).unwrap();
        let other = X11Clipboard::new_with_limit(1024).unwrap();
        let a = ClipboardContent::image(vec![1, 2, 3], "png");
        first.set_content(&a).await.unwrap();
        assert_eq!(
            first.reconcile_unknown_image(&a, &a).await.unwrap(),
            super::super::ImageAttribution::Owned
        );
        other.set_content(&a).await.unwrap();
        assert_eq!(
            first.reconcile_unknown_image(&a, &a).await.unwrap(),
            super::super::ImageAttribution::Foreign
        );
        let b = ClipboardContent::image(vec![4, 5, 6], "png");
        other.set_content(&b).await.unwrap();
        assert_eq!(
            first.reconcile_unknown_image(&a, &a).await.unwrap(),
            super::super::ImageAttribution::Unknown
        );
        assert_eq!(X11Clipboard::read_png(1024).unwrap().unwrap(), b.data);
    }

    #[tokio::test]
    #[ignore = "Requires a disposable X11 clipboard session"]
    async fn png_incremental_transfer_obeys_bound() {
        use x11rb::connection::RequestConnection;
        let writer = X11Clipboard::new_with_limit(64 * 1024 * 1024).unwrap();
        let request_max = writer.clipboard.setter.connection.maximum_request_bytes();
        assert!(
            request_max < 32 * 1024 * 1024,
            "test X server max request unexpectedly large: {request_max}"
        );
        let size = request_max + 1024;
        let data = vec![0x5a; size];
        writer
            .set_content(&ClipboardContent::image(data.clone(), "png"))
            .await
            .unwrap();
        let result = X11Clipboard::read_png(size).unwrap().unwrap();
        assert_eq!(result, data);
        assert!(matches!(
            X11Clipboard::read_png(1024),
            Err(ClipboardError::TooLarge { max: 1024, .. })
        ));
    }

    #[tokio::test]
    #[ignore = "Requires a disposable X11 clipboard session"]
    async fn png_roundtrip_bounds_and_primary_selection() {
        let writer = X11Clipboard::new_with_limit(1024 * 1024).unwrap();
        let reader = X11Clipboard::new_with_limit(64).unwrap();
        reader
            .image_capture
            .store(true, std::sync::atomic::Ordering::Relaxed);
        writer
            .set_content(&ClipboardContent::text("primary text"))
            .await
            .unwrap();
        writer
            .set_content(&ClipboardContent::image(vec![7; 64], "png"))
            .await
            .unwrap();
        let image = reader.get_content().await.unwrap();
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.data, vec![7; 64]);
        assert_eq!(
            writer
                .read_selection(ClipboardSelection::Primary)
                .unwrap()
                .unwrap(),
            b"primary text"
        );
        writer
            .set_content(&ClipboardContent::image(vec![8; 65], "png"))
            .await
            .unwrap();
        assert!(matches!(
            reader.get_content().await,
            Err(ClipboardError::TooLarge { max: 64, .. })
        ));
    }

    #[tokio::test]
    #[ignore = "Requires a disposable X11 clipboard session"]
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

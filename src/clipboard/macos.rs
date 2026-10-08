//! macOS clipboard implementation using NSPasteboard

use super::{
    ClipboardContent, ClipboardError, ClipboardEvent, ClipboardProvider, ClipboardWatcher,
    MAX_CLIPBOARD_SIZE,
};
use async_trait::async_trait;
use cocoa::base::{id, nil};
use cocoa::foundation::{NSArray, NSAutoreleasePool, NSData, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

/// NSPasteboard type constants
const NS_PASTEBOARD_TYPE_STRING: &str = "NSPasteboardTypeString";
const NS_PASTEBOARD_TYPE_RTF: &str = "NSPasteboardTypeRTF";
const NS_PASTEBOARD_TYPE_PNG: &str = "NSPasteboardTypePNG";
const NS_PASTEBOARD_TYPE_TIFF: &str = "NSPasteboardTypeTIFF";

/// macOS clipboard provider
pub struct MacOSClipboard {
    pasteboard: id,
}

impl MacOSClipboard {
    /// Create a new macOS clipboard provider
    pub fn new() -> Result<Self, ClipboardError> {
        unsafe {
            let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard == nil {
                return Err(ClipboardError::Platform(
                    "Failed to get general pasteboard".to_string(),
                ));
            }

            Ok(Self { pasteboard })
        }
    }

    /// Get NSString for pasteboard type
    unsafe fn get_type_string(type_str: &str) -> id {
        match type_str {
            NS_PASTEBOARD_TYPE_STRING => cocoa::appkit::NSPasteboardTypeString,
            NS_PASTEBOARD_TYPE_RTF => cocoa::appkit::NSPasteboardTypeRTF,
            NS_PASTEBOARD_TYPE_PNG => cocoa::appkit::NSPasteboardTypePNG,
            NS_PASTEBOARD_TYPE_TIFF => cocoa::appkit::NSPasteboardTypeTIFF,
            _ => unreachable!("unsupported pasteboard type"),
        }
    }

    /// Read string from pasteboard
    unsafe fn read_string(&self) -> Result<Option<String>, ClipboardError> {
        let pool = NSAutoreleasePool::new(nil);

        let string_type = Self::get_type_string(NS_PASTEBOARD_TYPE_STRING);
        let types = NSArray::arrayWithObject(nil, string_type);

        let available: id = msg_send![self.pasteboard, availableTypeFromArray: types];
        if available == nil {
            let _: () = msg_send![pool, drain];
            return Ok(None);
        }

        let string_data: id = msg_send![self.pasteboard, stringForType: string_type];
        if string_data == nil {
            let _: () = msg_send![pool, drain];
            return Ok(None);
        }

        let utf8_ptr: *const c_char = msg_send![string_data, UTF8String];
        if utf8_ptr.is_null() {
            let _: () = msg_send![pool, drain];
            return Ok(None);
        }

        let c_str = CStr::from_ptr(utf8_ptr);
        let result = c_str.to_string_lossy().into_owned();

        let _: () = msg_send![pool, drain];
        Ok(Some(result))
    }

    /// Read RTF data from pasteboard
    unsafe fn read_rtf(&self) -> Result<Option<Vec<u8>>, ClipboardError> {
        let pool = NSAutoreleasePool::new(nil);

        let rtf_type = Self::get_type_string(NS_PASTEBOARD_TYPE_RTF);
        let types = NSArray::arrayWithObject(nil, rtf_type);

        let available: id = msg_send![self.pasteboard, availableTypeFromArray: types];
        if available == nil {
            let _: () = msg_send![pool, drain];
            return Ok(None);
        }

        let data: id = msg_send![self.pasteboard, dataForType: rtf_type];
        if data == nil {
            let _: () = msg_send![pool, drain];
            return Ok(None);
        }

        let length: usize = msg_send![data, length];
        let bytes: *const u8 = msg_send![data, bytes];

        if bytes.is_null() || length == 0 {
            let _: () = msg_send![pool, drain];
            return Ok(None);
        }

        let result = std::slice::from_raw_parts(bytes, length).to_vec();

        let _: () = msg_send![pool, drain];
        Ok(Some(result))
    }

    /// Read image data from pasteboard
    unsafe fn read_image(&self) -> Result<Option<(Vec<u8>, &'static str)>, ClipboardError> {
        let pool = NSAutoreleasePool::new(nil);

        // Try PNG first
        let png_type = Self::get_type_string(NS_PASTEBOARD_TYPE_PNG);
        let png_types = NSArray::arrayWithObject(nil, png_type);

        let available: id = msg_send![self.pasteboard, availableTypeFromArray: png_types];
        if available != nil {
            let data: id = msg_send![self.pasteboard, dataForType: png_type];
            if data != nil {
                let length: usize = msg_send![data, length];
                let bytes: *const u8 = msg_send![data, bytes];

                if !bytes.is_null() && length > 0 {
                    let result = std::slice::from_raw_parts(bytes, length).to_vec();
                    let _: () = msg_send![pool, drain];
                    return Ok(Some((result, "png")));
                }
            }
        }

        // Try TIFF
        let tiff_type = Self::get_type_string(NS_PASTEBOARD_TYPE_TIFF);
        let tiff_types = NSArray::arrayWithObject(nil, tiff_type);

        let available: id = msg_send![self.pasteboard, availableTypeFromArray: tiff_types];
        if available != nil {
            let data: id = msg_send![self.pasteboard, dataForType: tiff_type];
            if data != nil {
                let length: usize = msg_send![data, length];
                let bytes: *const u8 = msg_send![data, bytes];

                if !bytes.is_null() && length > 0 {
                    let result = std::slice::from_raw_parts(bytes, length).to_vec();
                    let _: () = msg_send![pool, drain];
                    return Ok(Some((result, "tiff")));
                }
            }
        }

        let _: () = msg_send![pool, drain];
        Ok(None)
    }

    /// Get current change count
    unsafe fn get_change_count(&self) -> i64 {
        msg_send![self.pasteboard, changeCount]
    }
}

#[async_trait]
impl ClipboardProvider for MacOSClipboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        unsafe {
            // Try text first
            if let Some(text) = self.read_string()? {
                return Ok(ClipboardContent::text(text));
            }

            // Try RTF
            if let Some(rtf_data) = self.read_rtf()? {
                return Ok(ClipboardContent::rtf(rtf_data));
            }

            // Try image
            if let Some((image_data, format)) = self.read_image()? {
                return Ok(ClipboardContent::image(image_data, format));
            }

            Err(ClipboardError::NoContent)
        }
    }

    async fn set_content(&self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        // Check size limit
        if content.size() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge {
                size: content.size(),
                max: MAX_CLIPBOARD_SIZE,
            });
        }

        unsafe {
            let pool = NSAutoreleasePool::new(nil);

            // Clear the pasteboard first
            let _: () = msg_send![self.pasteboard, clearContents];

            match content.mime_type.as_str() {
                "text/plain" => {
                    if let Some(text) = content.as_text() {
                        let string = NSString::alloc(nil).init_str(&text);
                        let _string_type = Self::get_type_string(NS_PASTEBOARD_TYPE_STRING);
                        let success: bool = msg_send![self.pasteboard,
                            writeObjects: NSArray::arrayWithObject(nil, string)];

                        if !success {
                            let _: () = msg_send![pool, drain];
                            return Err(ClipboardError::Platform(
                                "Failed to write text to pasteboard".to_string(),
                            ));
                        }
                    }
                }
                "text/rtf" => {
                    let data = NSData::dataWithBytes_length_(
                        nil,
                        content.data.as_ptr() as *const _,
                        content.data.len() as u64,
                    );
                    let rtf_type = Self::get_type_string(NS_PASTEBOARD_TYPE_RTF);

                    let success: bool = msg_send![self.pasteboard,
                        setData: data
                        forType: rtf_type];

                    if !success {
                        let _: () = msg_send![pool, drain];
                        return Err(ClipboardError::Platform(
                            "Failed to write RTF to pasteboard".to_string(),
                        ));
                    }
                }
                mime_type if mime_type.starts_with("image/") => {
                    let data = NSData::dataWithBytes_length_(
                        nil,
                        content.data.as_ptr() as *const _,
                        content.data.len() as u64,
                    );

                    let image_type = if mime_type.ends_with("png") {
                        Self::get_type_string(NS_PASTEBOARD_TYPE_PNG)
                    } else {
                        Self::get_type_string(NS_PASTEBOARD_TYPE_TIFF)
                    };

                    let success: bool = msg_send![self.pasteboard,
                        setData: data
                        forType: image_type];

                    if !success {
                        let _: () = msg_send![pool, drain];
                        return Err(ClipboardError::Platform(
                            "Failed to write image to pasteboard".to_string(),
                        ));
                    }
                }
                _ => {
                    let _: () = msg_send![pool, drain];
                    return Err(ClipboardError::UnsupportedType(content.mime_type.clone()));
                }
            }

            let _: () = msg_send![pool, drain];
            Ok(())
        }
    }

    async fn clear(&self) -> Result<(), ClipboardError> {
        unsafe {
            let _: () = msg_send![self.pasteboard, clearContents];
            Ok(())
        }
    }

    fn name(&self) -> &str {
        "macOS (NSPasteboard)"
    }

    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        let (tx, rx) = mpsc::channel(100);

        let handle = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(async move {
                let mut ticker = interval(Duration::from_millis(100));

                // Create our own clipboard instance for the watcher
                let watcher_clipboard = match MacOSClipboard::new() {
                    Ok(c) => c,
                    Err(_) => return,
                };

                let mut last_change_count = unsafe { watcher_clipboard.get_change_count() };

                loop {
                    ticker.tick().await;

                    let current_count = unsafe { watcher_clipboard.get_change_count() };

                    if current_count != last_change_count {
                        last_change_count = current_count;

                        if let Ok(content) = watcher_clipboard.get_content().await {
                            let event = ClipboardEvent {
                                content,
                                selection: None, // macOS doesn't have selections
                            };

                            if tx.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            })
        });

        Ok(ClipboardWatcher::new(rx, handle))
    }
}

// Safety: NSPasteboard is thread-safe according to Apple documentation
unsafe impl Send for MacOSClipboard {}
unsafe impl Sync for MacOSClipboard {}

#[cfg(test)]
mod tests {
    use super::*;

    struct NamedPasteboard(MacOSClipboard);
    impl NamedPasteboard {
        fn new() -> Self {
            unsafe {
                let pool = NSAutoreleasePool::new(nil);
                let name = NSString::alloc(nil)
                    .init_str(&format!("clipsync-test-{}", uuid::Uuid::new_v4()));
                let pasteboard: id = msg_send![class!(NSPasteboard), pasteboardWithName: name];
                assert_ne!(pasteboard, nil);
                let _: id = msg_send![pasteboard, retain];
                let _: () = msg_send![name, release];
                let _: () = msg_send![pool, drain];
                Self(MacOSClipboard { pasteboard })
            }
        }
    }
    impl Drop for NamedPasteboard {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0.pasteboard, releaseGlobally];
                let _: () = msg_send![self.0.pasteboard, release];
            }
        }
    }
    #[tokio::test]
    async fn named_pasteboards_roundtrip_without_touching_general_clipboard() {
        let first = NamedPasteboard::new();
        let second = NamedPasteboard::new();
        let text = "isolated native clipboard café\nsecond line";
        first
            .0
            .set_content(&ClipboardContent::text(text))
            .await
            .unwrap();
        let content = first.0.get_content().await.unwrap();
        assert_eq!(content.as_text().as_deref(), Some(text));
        second.0.set_content(&content).await.unwrap();
        assert_eq!(
            second.0.get_content().await.unwrap().as_text().as_deref(),
            Some(text)
        );
        second.0.clear().await.unwrap();
        assert!(second.0.get_content().await.is_err());
        assert_eq!(
            first.0.get_content().await.unwrap().as_text().as_deref(),
            Some(text)
        );
    }
}

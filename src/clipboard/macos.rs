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
const PRIVATE_IMAGE_MARKER: &str = "org.clipsync.private.image-write";

/// macOS clipboard provider
pub struct MacOSClipboard {
    pasteboard: id,
    last_image_marker: std::sync::Mutex<Option<String>>,
    max_size: usize,
    image_capture: std::sync::atomic::AtomicBool,
}

impl MacOSClipboard {
    /// Create a new macOS clipboard provider
    pub fn new() -> Result<Self, ClipboardError> {
        Self::new_with_limit(MAX_CLIPBOARD_SIZE)
    }

    pub fn new_with_limit(max_size: usize) -> Result<Self, ClipboardError> {
        // Fail closed if an isolation request reaches a normal production build.
        if let Some(name) = std::env::var_os("CLIPSYNC_TEST_PASTEBOARD") {
            #[cfg(any(test, feature = "integration-tests"))]
            return Self::named_for_testing_with_limit(
                name.to_str().ok_or_else(|| {
                    ClipboardError::Platform("Test pasteboard name must be UTF-8".into())
                })?,
                max_size,
            );
            #[cfg(not(any(test, feature = "integration-tests")))]
            {
                let _ = name;
                return Err(ClipboardError::Platform(
                    "CLIPSYNC_TEST_PASTEBOARD requires the integration-tests feature".into(),
                ));
            }
        }
        unsafe {
            let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard == nil {
                return Err(ClipboardError::Platform(
                    "Failed to get general pasteboard".to_string(),
                ));
            }

            let _: id = msg_send![pasteboard, retain];
            Ok(Self {
                pasteboard,
                last_image_marker: std::sync::Mutex::new(None),
                max_size,
                image_capture: std::sync::atomic::AtomicBool::new(false),
            })
        }
    }

    /// Use a separate native pasteboard for a deliberately isolated test build.
    #[cfg(any(test, feature = "integration-tests"))]
    fn named_for_testing(name: &str) -> Result<Self, ClipboardError> {
        Self::named_for_testing_with_limit(name, MAX_CLIPBOARD_SIZE)
    }

    #[cfg(any(test, feature = "integration-tests"))]
    fn named_for_testing_with_limit(name: &str, max_size: usize) -> Result<Self, ClipboardError> {
        let suffix = name.strip_prefix("org.clipsync.test.").ok_or_else(|| {
            ClipboardError::Platform("Test pasteboard must start with org.clipsync.test.".into())
        })?;
        if suffix.is_empty()
            || !suffix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(ClipboardError::Platform(
                "Invalid test pasteboard name".into(),
            ));
        }
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let native_name = NSString::alloc(nil).init_str(name);
            let pasteboard: id = msg_send![class!(NSPasteboard), pasteboardWithName: native_name];
            let _: () = msg_send![native_name, release];
            if pasteboard != nil {
                let _: id = msg_send![pasteboard, retain];
            }
            let _: () = msg_send![pool, drain];
            if pasteboard == nil {
                return Err(ClipboardError::Platform(
                    "Cannot open test pasteboard".into(),
                ));
            }
            Ok(Self {
                pasteboard,
                last_image_marker: std::sync::Mutex::new(None),
                max_size,
                image_capture: std::sync::atomic::AtomicBool::new(false),
            })
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

                if length > self.max_size {
                    let _: () = msg_send![pool, drain];
                    return Err(ClipboardError::TooLarge {
                        size: length,
                        max: self.max_size,
                    });
                }
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

                if length > super::png::MAX_NATIVE_TIFF_BYTES {
                    let _: () = msg_send![pool, drain];
                    return Err(ClipboardError::TooLarge {
                        size: length,
                        max: super::png::MAX_NATIVE_TIFF_BYTES,
                    });
                }
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

    /// Write image data only while the clear token still matches. This detects
    /// some intervening writers; it cannot make the check and write atomic.
    unsafe fn write_image_with_token(&self, item: id, token: i64) -> Result<(), ClipboardError> {
        if self.get_change_count() != token {
            return Err(ClipboardError::WriteOutcomeUnknown(
                "Pasteboard changed after clearing".into(),
            ));
        }
        self.write_image_after_check(item, token)
    }

    unsafe fn write_image_after_check(&self, item: id, token: i64) -> Result<(), ClipboardError> {
        let success: bool =
            msg_send![self.pasteboard, writeObjects: NSArray::arrayWithObject(nil, item)];
        if !success || self.get_change_count() != token || self.has_ambiguous_image_items() {
            return Err(ClipboardError::WriteOutcomeUnknown(
                "Image write failed or pasteboard changed".into(),
            ));
        }
        Ok(())
    }

    /// A private image marker in a multi-item board cannot safely identify the intended copy.
    unsafe fn has_ambiguous_image_items(&self) -> bool {
        let pool = NSAutoreleasePool::new(nil);
        let items: id = msg_send![self.pasteboard, pasteboardItems];
        let count: usize = if items == nil {
            0
        } else {
            msg_send![items, count]
        };
        let marker_type = NSString::alloc(nil).init_str(PRIVATE_IMAGE_MARKER);
        let mut found = false;
        if count > 1 {
            for index in 0..count {
                let item: id = msg_send![items, objectAtIndex: index];
                let marker: id = msg_send![item, stringForType: marker_type];
                if marker != nil {
                    found = true;
                    break;
                }
            }
        }
        let _: () = msg_send![marker_type, release];
        let _: () = msg_send![pool, drain];
        found
    }

    /// Attribute a sole, stable native item using the per-write marker.
    unsafe fn attribute_marked_image(
        &self,
        content: &ClipboardContent,
        observed: &ClipboardContent,
    ) -> super::ImageAttribution {
        use super::ImageAttribution::{Foreign, Owned, Unknown};
        if content.mime_type != "image/png"
            || observed.mime_type != "image/png"
            || content.size() > self.max_size
            || observed.size() > self.max_size
        {
            return Unknown;
        }
        let marker = self
            .last_image_marker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(marker) = marker else { return Unknown };
        let pool = NSAutoreleasePool::new(nil);
        let before = self.get_change_count();
        let items: id = msg_send![self.pasteboard, pasteboardItems];
        let count: usize = if items == nil {
            0
        } else {
            msg_send![items, count]
        };
        let mut result = Unknown;
        if count == 1 {
            let item: id = msg_send![items, objectAtIndex: 0usize];
            let marker_type = NSString::alloc(nil).init_str(PRIVATE_IMAGE_MARKER);
            let value: id = msg_send![item, stringForType: marker_type];
            let marker_ptr: *const c_char = if value == nil {
                std::ptr::null()
            } else {
                msg_send![value, UTF8String]
            };
            let image_type = Self::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let data: id = msg_send![item, dataForType: image_type];
            if data != nil {
                let length: usize = msg_send![data, length];
                let ptr: *const u8 = msg_send![data, bytes];
                if !ptr.is_null()
                    && length == observed.data.len()
                    && std::slice::from_raw_parts(ptr, length) == observed.data.as_slice()
                {
                    if marker_ptr.is_null()
                        || CStr::from_ptr(marker_ptr).to_bytes() != marker.as_bytes()
                    {
                        result = Foreign;
                    } else if observed.data == content.data {
                        result = Owned;
                    }
                }
            }
            let _: () = msg_send![marker_type, release];
        }
        let after_items: id = msg_send![self.pasteboard, pasteboardItems];
        let after_count: usize = if after_items == nil {
            0
        } else {
            msg_send![after_items, count]
        };
        if before != self.get_change_count() || after_count != 1 {
            result = Unknown;
        }
        let _: () = msg_send![pool, drain];
        result
    }

    unsafe fn has_type(&self, type_name: &str) -> bool {
        let pool = NSAutoreleasePool::new(nil);
        let kind = NSString::alloc(nil).init_str(type_name);
        let types = NSArray::arrayWithObject(nil, kind);
        let available: id = msg_send![self.pasteboard, availableTypeFromArray: types];
        let result = available != nil;
        let _: () = msg_send![kind, release];
        let _: () = msg_send![pool, drain];
        result
    }

    /// Revalidate the whole snapshot after reading; `after_read` permits an
    /// isolated test to force an append at the check/read boundary.
    unsafe fn read_content_checked(
        &self,
        after_read: impl FnOnce(),
    ) -> Result<ClipboardContent, ClipboardError> {
        let before = self.get_change_count();
        if self.has_ambiguous_image_items() {
            return Err(ClipboardError::Ambiguous);
        }
        // File copies may offer a preview bitmap; keep those as file/text copies.
        // Rich text similarly takes precedence over embedded image representations.
        let file_copy = self.has_type("public.file-url") || self.has_type("NSFilenamesPboardType");
        let rich_text = self.has_type("public.rtf");
        let preferred_image = if self
            .image_capture
            .load(std::sync::atomic::Ordering::Relaxed)
            && !file_copy
            && !rich_text
        {
            self.read_image()?
        } else {
            None
        };
        let result = if let Some((image, format)) = preferred_image {
            Ok(ClipboardContent::image(image, format))
        } else if let Some(text) = self.read_string()? {
            Ok(ClipboardContent::text(text))
        } else if let Some(rtf) = self.read_rtf()? {
            Ok(ClipboardContent::rtf(rtf))
        } else if !file_copy {
            match self.read_image()? {
                Some((image, format)) => Ok(ClipboardContent::image(image, format)),
                None => Err(ClipboardError::NoContent),
            }
        } else {
            Err(ClipboardError::NoContent)
        };
        after_read();
        if before != self.get_change_count() || self.has_ambiguous_image_items() {
            return Err(ClipboardError::Ambiguous);
        }
        result
    }

    /// Get current change count
    unsafe fn get_change_count(&self) -> i64 {
        msg_send![self.pasteboard, changeCount]
    }
}

#[async_trait]
impl ClipboardProvider for MacOSClipboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        unsafe { self.read_content_checked(|| {}) }
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

        // Reject unsupported formats and invalid text before touching the board.
        match content.mime_type.as_str() {
            "text/plain" if content.as_text().is_none() => {
                return Err(ClipboardError::Platform(
                    "Invalid UTF-8 in clipboard".into(),
                ))
            }
            "text/plain" | "text/rtf" | "image/png" | "image/tiff" => {}
            _ => return Err(ClipboardError::UnsupportedType(content.mime_type.clone())),
        }

        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            // Prepare native image data before the destructive clear.
            let prepared_image: id =
                if matches!(content.mime_type.as_str(), "image/png" | "image/tiff") {
                    let data = NSData::dataWithBytes_length_(
                        nil,
                        content.data.as_ptr() as *const _,
                        content.data.len() as u64,
                    );
                    let kind = if content.mime_type == "image/png" {
                        Self::get_type_string(NS_PASTEBOARD_TYPE_PNG)
                    } else {
                        Self::get_type_string(NS_PASTEBOARD_TYPE_TIFF)
                    };
                    let item: id = msg_send![class!(NSPasteboardItem), new];
                    if item == nil || data == nil {
                        if item != nil {
                            let _: () = msg_send![item, release];
                        }
                        let _: () = msg_send![pool, drain];
                        return Err(ClipboardError::Platform(
                            "Failed to prepare image pasteboard item".into(),
                        ));
                    }
                    let staged: bool = msg_send![item, setData: data forType: kind];
                    // Some native consumers inspect NSPasteboardItem directly and do
                    // not use AppKit's board-level PNG-to-TIFF conversion. Offer
                    // both representations on the same item when conversion succeeds.
                    if content.mime_type == "image/png" {
                        let dimensions = image::ImageReader::with_format(
                            std::io::Cursor::new(&content.data),
                            image::ImageFormat::Png,
                        )
                        .into_dimensions();
                        if dimensions.is_ok_and(|(w, h)| {
                            w > 0
                                && h > 0
                                && w <= 8192
                                && h <= 8192
                                && u64::from(w) * u64::from(h) <= 16_777_216
                        }) {
                            let rep: id =
                                msg_send![class!(NSBitmapImageRep), imageRepWithData: data];
                            if rep != nil {
                                let tiff: id = msg_send![rep, TIFFRepresentation];
                                if tiff != nil {
                                    let length: usize = msg_send![tiff, length];
                                    if length <= super::png::MAX_NATIVE_TIFF_BYTES {
                                        let tiff_kind =
                                            Self::get_type_string(NS_PASTEBOARD_TYPE_TIFF);
                                        let _: bool =
                                            msg_send![item, setData: tiff forType: tiff_kind];
                                    }
                                }
                            }
                        }
                    }
                    let marker_type = NSString::alloc(nil).init_str(PRIVATE_IMAGE_MARKER);
                    let marker_string = uuid::Uuid::new_v4().to_string();
                    let marker_value = NSString::alloc(nil).init_str(&marker_string);
                    let marked: bool =
                        msg_send![item, setString: marker_value forType: marker_type];
                    let _: () = msg_send![marker_value, release];
                    let _: () = msg_send![marker_type, release];
                    if !staged || !marked {
                        let _: () = msg_send![item, release];
                        let _: () = msg_send![pool, drain];
                        return Err(ClipboardError::Platform(
                            "Failed to prepare image pasteboard data".into(),
                        ));
                    }
                    *self
                        .last_image_marker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(marker_string);
                    let _: id = msg_send![item, autorelease];
                    item
                } else {
                    nil
                };

            // Clear the pasteboard first
            let clear_token: i64 = msg_send![self.pasteboard, clearContents];

            match content.mime_type.as_str() {
                "text/plain" => {
                    if let Some(text) = content.as_text() {
                        let string = NSString::alloc(nil).init_str(&text);
                        let _string_type = Self::get_type_string(NS_PASTEBOARD_TYPE_STRING);
                        let success: bool = msg_send![self.pasteboard,
                            writeObjects: NSArray::arrayWithObject(nil, string)];

                        if !success {
                            let _: () = msg_send![pool, drain];
                            return Err(ClipboardError::WriteOutcomeUnknown(
                                "Failed to write text after clearing the pasteboard".to_string(),
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
                        return Err(ClipboardError::WriteOutcomeUnknown(
                            "Failed to write RTF after clearing the pasteboard".to_string(),
                        ));
                    }
                }
                "image/png" | "image/tiff" => {
                    let mut result = self.write_image_with_token(prepared_image, clear_token);
                    if result.is_ok()
                        && content.mime_type == "image/png"
                        && self.attribute_marked_image(content, content)
                            != crate::clipboard::ImageAttribution::Owned
                    {
                        result = Err(ClipboardError::WriteOutcomeUnknown(
                            "Could not attribute the written image".into(),
                        ));
                    }
                    if let Err(err) = result {
                        let _: () = msg_send![pool, drain];
                        return Err(err);
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

    async fn reconcile_unknown_image(
        &self,
        content: &ClipboardContent,
        observed: &ClipboardContent,
    ) -> Result<super::ImageAttribution, ClipboardError> {
        Ok(unsafe { self.attribute_marked_image(content, observed) })
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

impl Drop for MacOSClipboard {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.pasteboard, release];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NamedPasteboard(MacOSClipboard);
    impl NamedPasteboard {
        fn new() -> Self {
            Self(
                MacOSClipboard::named_for_testing(&format!(
                    "org.clipsync.test.{}",
                    uuid::Uuid::new_v4()
                ))
                .unwrap(),
            )
        }
    }
    impl Drop for NamedPasteboard {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0.pasteboard, releaseGlobally];
            }
        }
    }
    #[test]
    fn rejects_non_test_pasteboard_names() {
        for name in [
            "NSGeneralPboard",
            "",
            "org.clipsync.test.",
            "org.clipsync.test.bad/name",
        ] {
            assert!(MacOSClipboard::named_for_testing(name).is_err());
        }
    }
    #[tokio::test]
    async fn image_text_file_and_rich_text_representations_are_selected_deliberately() {
        let board = NamedPasteboard::new();
        unsafe fn install(board: &MacOSClipboard, extra_type: Option<&str>) {
            let pool = NSAutoreleasePool::new(nil);
            let _: i64 = msg_send![board.pasteboard, clearContents];
            let item: id = msg_send![class!(NSPasteboardItem), new];
            let text = NSString::alloc(nil).init_str("visible text");
            let text_type = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_STRING);
            let ok: bool = msg_send![item, setString: text forType: text_type];
            assert!(ok);
            let image = [1u8, 2, 3];
            let data =
                NSData::dataWithBytes_length_(nil, image.as_ptr() as *const _, image.len() as u64);
            let png = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let ok: bool = msg_send![item, setData: data forType: png];
            assert!(ok);
            if let Some(extra) = extra_type {
                let kind = NSString::alloc(nil).init_str(extra);
                let ok: bool = msg_send![item, setData: data forType: kind];
                assert!(ok);
                let _: () = msg_send![kind, release];
            }
            let ok: bool =
                msg_send![board.pasteboard, writeObjects: NSArray::arrayWithObject(nil, item)];
            assert!(ok);
            let _: () = msg_send![item, release];
            let _: () = msg_send![text, release];
            let _: () = msg_send![pool, drain];
        }
        unsafe {
            install(&board.0, None);
        }
        assert_eq!(
            board.0.get_content().await.unwrap().as_text().as_deref(),
            Some("visible text")
        );
        board.0.enable_image_capture();
        assert_eq!(board.0.get_content().await.unwrap().mime_type, "image/png");
        for kind in ["public.file-url", "public.rtf"] {
            unsafe {
                install(&board.0, Some(kind));
            }
            assert_eq!(
                board.0.get_content().await.unwrap().as_text().as_deref(),
                Some("visible text")
            );
        }
    }

    #[tokio::test]
    async fn named_pasteboard_uses_configured_size_limit() {
        let name = format!("org.clipsync.test.{}", uuid::Uuid::new_v4());
        let board =
            NamedPasteboard(MacOSClipboard::named_for_testing_with_limit(&name, 8).unwrap());
        let small = ClipboardContent::image(vec![1; 8], "png");
        board.0.set_content(&small).await.unwrap();
        assert_eq!(board.0.get_content().await.unwrap().data.len(), 8);
        let large = ClipboardContent::image(vec![1; 9], "png");
        assert!(matches!(
            board.0.set_content(&large).await,
            Err(ClipboardError::TooLarge { size: 9, max: 8 })
        ));
        assert_eq!(board.0.get_content().await.unwrap().data.len(), 8);
    }

    #[tokio::test]
    async fn named_pasteboard_rejects_oversized_image_before_copying_bytes() {
        let board = NamedPasteboard::new();
        let bytes = vec![1u8; MAX_CLIPBOARD_SIZE + 1];
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let _: i64 = msg_send![board.0.pasteboard, clearContents];
            let data =
                NSData::dataWithBytes_length_(nil, bytes.as_ptr() as *const _, bytes.len() as u64);
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let ok: bool = msg_send![board.0.pasteboard, setData: data forType: kind];
            assert!(ok);
            let result = board.0.get_content().await;
            assert!(
                matches!(result, Err(ClipboardError::TooLarge { size, max }) if size == MAX_CLIPBOARD_SIZE + 1 && max == MAX_CLIPBOARD_SIZE)
            );
            let _: () = msg_send![pool, drain];
        }
    }

    #[tokio::test]
    async fn unsupported_type_and_invalid_text_do_not_clear_pasteboard() {
        let board = NamedPasteboard::new();
        board
            .0
            .set_content(&ClipboardContent::text("sentinel"))
            .await
            .unwrap();
        let count = unsafe { board.0.get_change_count() };
        for content in [
            ClipboardContent::image(vec![1], "jpeg"),
            ClipboardContent {
                mime_type: "text/plain".into(),
                data: vec![0xff],
                timestamp: 0,
            },
        ] {
            assert!(board.0.set_content(&content).await.is_err());
            assert_eq!(unsafe { board.0.get_change_count() }, count);
            assert_eq!(
                board.0.get_content().await.unwrap().as_text().as_deref(),
                Some("sentinel")
            );
        }
    }

    #[tokio::test]
    async fn named_pasteboards_roundtrip_png_bytes() {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=")
            .unwrap();
        let first = NamedPasteboard::new();
        let second = NamedPasteboard::new();
        first
            .0
            .set_content(&ClipboardContent::image(png.clone(), "png"))
            .await
            .unwrap();
        // A consumer that reads TIFF directly from the item cannot use the
        // board-level synthesized representation.
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let items: id = msg_send![first.0.pasteboard, pasteboardItems];
            let item: id = msg_send![items, objectAtIndex: 0usize];
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_TIFF);
            let tiff: id = msg_send![item, dataForType: kind];
            assert_ne!(tiff, nil);
            let len: usize = msg_send![tiff, length];
            let ptr: *const u8 = msg_send![tiff, bytes];
            let decoded = image::load_from_memory_with_format(
                std::slice::from_raw_parts(ptr, len),
                image::ImageFormat::Tiff,
            )
            .unwrap()
            .to_rgba8();
            let expected = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .unwrap()
                .to_rgba8();
            assert_eq!(decoded, expected);
            let _: () = msg_send![pool, drain];
        }
        let content = first.0.get_content().await.unwrap();
        assert_eq!(content.mime_type, "image/png");
        assert_eq!(content.data, png);
        second.0.set_content(&content).await.unwrap();
        let received = second.0.get_content().await.unwrap();
        assert_eq!(received.mime_type, "image/png");
        assert_eq!(received.data, png);
    }

    #[tokio::test]
    async fn named_pasteboard_tiff_pixel_and_change_count_probe() {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=")
            .unwrap();
        let board = NamedPasteboard::new();
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let data =
                NSData::dataWithBytes_length_(nil, png.as_ptr() as *const _, png.len() as u64);
            let rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: data];
            assert_ne!(rep, nil);
            let width: isize = msg_send![rep, pixelsWide];
            let height: isize = msg_send![rep, pixelsHigh];
            assert_eq!((width, height), (2, 1));
            let tiff: id = msg_send![rep, TIFFRepresentation];
            assert_ne!(tiff, nil);
            let len: usize = msg_send![tiff, length];
            let ptr: *const u8 = msg_send![tiff, bytes];
            let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
            let before = board.0.get_change_count();
            board
                .0
                .set_content(&ClipboardContent::image(bytes, "tiff"))
                .await
                .unwrap();
            let after = board.0.get_change_count();
            assert_ne!(before, after);
            let result = board.0.get_content().await.unwrap();
            assert_eq!(result.mime_type, "image/tiff");
            let native = NSData::dataWithBytes_length_(
                nil,
                result.data.as_ptr() as *const _,
                result.data.len() as u64,
            );
            let decoded: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: native];
            assert_ne!(decoded, nil);
            let mut first = [0u64; 4];
            let mut second = [0u64; 4];
            let _: () = msg_send![decoded, getPixel: first.as_mut_ptr() atX: 0isize y: 0isize];
            let _: () = msg_send![decoded, getPixel: second.as_mut_ptr() atX: 1isize y: 0isize];
            assert_eq!(first, [255, 0, 0, 255]);
            assert_eq!(second, [0, 128, 255, 128]);
            assert_eq!(after, board.0.get_change_count());
            let _: () = msg_send![pool, drain];
        }
    }

    #[tokio::test]
    async fn named_pasteboard_reads_tiff_larger_than_png_limit() {
        use image::codecs::tiff::TiffEncoder;
        use std::io::Cursor;
        let board = NamedPasteboard(
            MacOSClipboard::named_for_testing_with_limit(
                &format!("org.clipsync.test.{}", uuid::Uuid::new_v4()),
                5 * 1024 * 1024,
            )
            .unwrap(),
        );
        let (width, height) = (2048, 1024);
        let pixels = vec![255u8; width as usize * height as usize * 4];
        let mut out = Cursor::new(Vec::new());
        TiffEncoder::new(&mut out)
            .encode(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        let tiff = out.into_inner();
        assert!(tiff.len() > board.0.max_size);
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let _: i64 = msg_send![board.0.pasteboard, clearContents];
            let item: id = msg_send![class!(NSPasteboardItem), new];
            let data =
                NSData::dataWithBytes_length_(nil, tiff.as_ptr() as *const _, tiff.len() as u64);
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_TIFF);
            let ok: bool = msg_send![item, setData: data forType: kind];
            assert!(ok);
            let ok: bool =
                msg_send![board.0.pasteboard, writeObjects: NSArray::arrayWithObject(nil, item)];
            assert!(ok);
            let _: () = msg_send![item, release];
            let _: () = msg_send![pool, drain];
        }
        board.0.enable_image_capture();
        let captured = board.0.get_content().await.unwrap();
        assert_eq!(captured.mime_type, "image/tiff");
        assert_eq!(captured.data, tiff);
        let bounds = crate::transport::image_capability::ImageLimits {
            max_bytes: board.0.max_size,
            max_width: width,
            max_height: height,
            max_pixels: u64::from(width) * u64::from(height),
        };
        let png = super::super::png::from_tiff(&captured.data, bounds, 64 * 1024 * 1024).unwrap();
        assert!(png.len() < board.0.max_size);
        assert_eq!(
            super::super::png::decode(&png, bounds, 64 * 1024 * 1024)
                .unwrap()
                .rgba,
            pixels
        );
    }

    #[tokio::test]
    async fn append_between_snapshot_read_and_validation_is_rejected() {
        let board = NamedPasteboard::new();
        board
            .0
            .set_content(&ClipboardContent::image(vec![1, 2, 3], "png"))
            .await
            .unwrap();
        unsafe {
            let result = board.0.read_content_checked(|| {
                let pool = NSAutoreleasePool::new(nil);
                let foreign = NSString::alloc(nil).init_str("concurrent append");
                let ok: bool = msg_send![board.0.pasteboard, writeObjects: NSArray::arrayWithObject(nil, foreign)];
                assert!(ok);
                let _: () = msg_send![foreign, release];
                let _: () = msg_send![pool, drain];
            });
            assert!(matches!(result, Err(ClipboardError::Ambiguous)));
        }
        board
            .0
            .set_content(&ClipboardContent::text("next copy"))
            .await
            .unwrap();
        assert_eq!(
            board.0.get_content().await.unwrap().as_text().as_deref(),
            Some("next copy")
        );
    }

    #[tokio::test]
    async fn attribution_rejects_stale_observation_after_foreign_replacement() {
        let board = NamedPasteboard::new();
        let image_a = ClipboardContent::image(vec![1, 2, 3], "png");
        board.0.set_content(&image_a).await.unwrap();
        let observed_a = board.0.get_content().await.unwrap();
        let image_b = ClipboardContent::image(vec![4, 5, 6], "png");
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let _: i64 = msg_send![board.0.pasteboard, clearContents];
            let data = NSData::dataWithBytes_length_(
                nil,
                image_b.data.as_ptr() as *const _,
                image_b.data.len() as u64,
            );
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let ok: bool = msg_send![board.0.pasteboard, setData: data forType: kind];
            assert!(ok);
            let _: () = msg_send![pool, drain];
        }
        assert_eq!(
            board
                .0
                .reconcile_unknown_image(&image_a, &observed_a)
                .await
                .unwrap(),
            crate::clipboard::ImageAttribution::Unknown
        );
        assert_eq!(board.0.get_content().await.unwrap().data, image_b.data);
    }

    #[tokio::test]
    async fn identical_image_from_new_owner_is_foreign() {
        let board = NamedPasteboard::new();
        let image = ClipboardContent::image(vec![1, 2, 3], "png");
        board.0.set_content(&image).await.unwrap();
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let _: i64 = msg_send![board.0.pasteboard, clearContents];
            let data = NSData::dataWithBytes_length_(
                nil,
                image.data.as_ptr() as *const _,
                image.data.len() as u64,
            );
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let ok: bool = msg_send![board.0.pasteboard, setData: data forType: kind];
            assert!(ok);
            let _: () = msg_send![pool, drain];
        }
        assert_eq!(
            board
                .0
                .reconcile_unknown_image(&image, &image)
                .await
                .unwrap(),
            crate::clipboard::ImageAttribution::Foreign
        );
    }

    #[tokio::test]
    async fn mixed_marked_snapshot_is_skipped_until_next_copy() {
        let board = NamedPasteboard::new();
        let written = ClipboardContent::image(vec![1, 2, 3], "png");
        board.0.set_content(&written).await.unwrap();
        assert_eq!(
            board
                .0
                .reconcile_unknown_image(&written, &written)
                .await
                .unwrap(),
            crate::clipboard::ImageAttribution::Owned
        );
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let foreign = NSString::alloc(nil).init_str("foreign copy");
            let ok: bool =
                msg_send![board.0.pasteboard, writeObjects: NSArray::arrayWithObject(nil, foreign)];
            assert!(ok);
            let _: () = msg_send![foreign, release];
            let _: () = msg_send![pool, drain];
        }
        assert!(matches!(
            board.0.get_content().await,
            Err(ClipboardError::Ambiguous)
        ));
        assert_eq!(
            board
                .0
                .reconcile_unknown_image(&written, &written)
                .await
                .unwrap(),
            crate::clipboard::ImageAttribution::Unknown
        );
        board
            .0
            .set_content(&ClipboardContent::text("next copy"))
            .await
            .unwrap();
        assert_eq!(
            board.0.get_content().await.unwrap().as_text().as_deref(),
            Some("next copy")
        );
    }

    #[tokio::test]
    async fn named_pasteboard_private_marker_survives_an_appended_item_probe() {
        let board = NamedPasteboard::new();
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let _: i64 = msg_send![board.0.pasteboard, clearContents];
            let item: id = msg_send![class!(NSPasteboardItem), new];
            let marker_type = NSString::alloc(nil).init_str("org.clipsync.test.write-marker");
            let marker = NSString::alloc(nil).init_str("unique-fixture-marker");
            let ok: bool = msg_send![item, setString: marker forType: marker_type];
            assert!(ok);
            let ok: bool =
                msg_send![board.0.pasteboard, writeObjects: NSArray::arrayWithObject(nil, item)];
            assert!(ok);
            let token = board.0.get_change_count();
            let before: id = msg_send![board.0.pasteboard, pasteboardItems];
            let first_count: usize = msg_send![before, count];
            let other = NSString::alloc(nil).init_str("concurrent appended text");
            let ok: bool =
                msg_send![board.0.pasteboard, writeObjects: NSArray::arrayWithObject(nil, other)];
            assert!(ok);
            let after: id = msg_send![board.0.pasteboard, pasteboardItems];
            let second_count: usize = msg_send![after, count];
            let existing: id = msg_send![board.0.pasteboard, stringForType: marker_type];
            println!("marker append: count {first_count}->{second_count}, token_changed={}, marker_present={}", token != board.0.get_change_count(), existing != nil);
            assert!(second_count > first_count);
            assert_ne!(existing, nil);
            let _: () = msg_send![other, release];
            let _: () = msg_send![marker, release];
            let _: () = msg_send![marker_type, release];
            let _: () = msg_send![item, release];
            let _: () = msg_send![pool, drain];
        }
    }

    #[tokio::test]
    async fn named_pasteboard_write_objects_without_clear_probe() {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=").unwrap();
        let board = NamedPasteboard::new();
        board
            .0
            .set_content(&ClipboardContent::text("existing copy"))
            .await
            .unwrap();
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let before = board.0.get_change_count();
            let item: id = msg_send![class!(NSPasteboardItem), new];
            let data =
                NSData::dataWithBytes_length_(nil, bytes.as_ptr() as *const _, bytes.len() as u64);
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let staged: bool = msg_send![item, setData: data forType: kind];
            assert!(staged);
            let ok: bool =
                msg_send![board.0.pasteboard, writeObjects: NSArray::arrayWithObject(nil, item)];
            let after = board.0.get_change_count();
            let text = board.0.read_string().unwrap();
            let image = board.0.read_image().unwrap();
            println!("writeObjects without clear: ok={ok}, token_changed={}, text={text:?}, png_present={}", after != before, image.is_some());
            let _: () = msg_send![item, release];
            let _: () = msg_send![pool, drain];
        }
    }

    #[tokio::test]
    async fn write_interleaving_leaves_mixed_snapshot_ambiguous_then_recovers() {
        let name = format!("org.clipsync.test.{}", uuid::Uuid::new_v4());
        let first = NamedPasteboard(MacOSClipboard::named_for_testing(&name).unwrap());
        let other = MacOSClipboard::named_for_testing(&name).unwrap();
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let item: id = msg_send![class!(NSPasteboardItem), new];
            let data = NSData::dataWithBytes_length_(nil, [1u8].as_ptr() as *const _, 1);
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let staged: bool = msg_send![item, setData: data forType: kind];
            assert!(staged);
            let marker_type = NSString::alloc(nil).init_str(PRIVATE_IMAGE_MARKER);
            let marker = NSString::alloc(nil).init_str("interleaving-test");
            let marked: bool = msg_send![item, setString: marker forType: marker_type];
            assert!(marked);
            let token: i64 = msg_send![first.0.pasteboard, clearContents];
            assert_eq!(first.0.get_change_count(), token); // engine's pre-write check
            other
                .set_content(&ClipboardContent::text("foreign copy"))
                .await
                .unwrap();
            assert!(matches!(
                first.0.write_image_after_check(item, token),
                Err(ClipboardError::WriteOutcomeUnknown(_))
            ));
            assert!(matches!(
                first.0.get_content().await,
                Err(ClipboardError::Ambiguous)
            ));
            other
                .set_content(&ClipboardContent::text("next copy"))
                .await
                .unwrap();
            assert_eq!(
                first.0.get_content().await.unwrap().as_text().as_deref(),
                Some("next copy")
            );
            let _: () = msg_send![marker, release];
            let _: () = msg_send![marker_type, release];
            let _: () = msg_send![item, release];
            let _: () = msg_send![pool, drain];
        }
    }

    #[tokio::test]
    async fn named_pasteboard_intervening_write_changes_clear_token() {
        let name = format!("org.clipsync.test.{}", uuid::Uuid::new_v4());
        let first = NamedPasteboard(MacOSClipboard::named_for_testing(&name).unwrap());
        let other = MacOSClipboard::named_for_testing(&name).unwrap();
        unsafe {
            let token: i64 = msg_send![first.0.pasteboard, clearContents];
            other
                .set_content(&ClipboardContent::text("concurrent copy"))
                .await
                .unwrap();
            assert_ne!(first.0.get_change_count(), token);
            let png = [1u8];
            let pool = NSAutoreleasePool::new(nil);
            let data = NSData::dataWithBytes_length_(nil, png.as_ptr() as *const _, 1);
            let kind = MacOSClipboard::get_type_string(NS_PASTEBOARD_TYPE_PNG);
            let item: id = msg_send![class!(NSPasteboardItem), new];
            let staged: bool = msg_send![item, setData: data forType: kind];
            assert!(staged);
            assert!(matches!(
                first.0.write_image_with_token(item, token),
                Err(ClipboardError::WriteOutcomeUnknown(_))
            ));
            let _: () = msg_send![item, release];
            let _: () = msg_send![pool, drain];
            // The token check avoids this intervening write, but cannot make
            // the following check-and-write atomic.
            assert_eq!(
                other.get_content().await.unwrap().as_text().as_deref(),
                Some("concurrent copy")
            );
        }
    }

    #[tokio::test]
    async fn named_pasteboard_change_count_detects_another_owner_write() {
        let name = format!("org.clipsync.test.{}", uuid::Uuid::new_v4());
        let first = NamedPasteboard(MacOSClipboard::named_for_testing(&name).unwrap());
        let other = MacOSClipboard::named_for_testing(&name).unwrap();
        first
            .0
            .set_content(&ClipboardContent::text("first"))
            .await
            .unwrap();
        let token = unsafe { first.0.get_change_count() };
        assert_eq!(
            first.0.get_content().await.unwrap().as_text().as_deref(),
            Some("first")
        );
        other
            .set_content(&ClipboardContent::text("second"))
            .await
            .unwrap();
        assert_ne!(token, unsafe { first.0.get_change_count() });
        assert_eq!(
            first.0.get_content().await.unwrap().as_text().as_deref(),
            Some("second")
        );
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

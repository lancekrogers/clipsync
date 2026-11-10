//! Validation of per-session PNG admission limits.

use super::protocol::{CapabilitiesPayload, ClipboardFormat};

/// Capability extension keys. Values are nonzero base-10 integers.
pub const MAX_WIDTH: &str = "clipsync.image.png.max_width";
pub const MAX_HEIGHT: &str = "clipsync.image.png.max_height";
pub const MAX_PIXELS: &str = "clipsync.image.png.max_pixels";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLimits {
    pub max_bytes: usize,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
}

impl ImageLimits {
    pub fn capabilities(self) -> CapabilitiesPayload {
        CapabilitiesPayload {
            formats: vec![ClipboardFormat::Image {
                mime_type: "image/png".into(),
            }],
            compression: Vec::new(),
            max_message_size: self.max_bytes,
            streaming_support: false,
            keepalive_interval: 0,
            extensions: std::collections::HashMap::from([
                (MAX_WIDTH.into(), self.max_width.to_string()),
                (MAX_HEIGHT.into(), self.max_height.to_string()),
                (MAX_PIXELS.into(), self.max_pixels.to_string()),
            ]),
        }
    }

    /// Missing or malformed limits do not authorize image delivery.
    pub fn from_capabilities(payload: &CapabilitiesPayload) -> Option<Self> {
        if !payload
            .formats
            .iter()
            .any(|f| matches!(f, ClipboardFormat::Image { mime_type } if mime_type == "image/png"))
        {
            return None;
        }
        fn number<T: std::str::FromStr>(value: Option<&String>) -> Option<T> {
            let value = value?;
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            value.parse().ok()
        }
        let limits = Self {
            max_bytes: payload.max_message_size,
            max_width: number(payload.extensions.get(MAX_WIDTH))?,
            max_height: number(payload.extensions.get(MAX_HEIGHT))?,
            max_pixels: number(payload.extensions.get(MAX_PIXELS))?,
        };
        if limits.max_bytes == 0
            || limits.max_width == 0
            || limits.max_height == 0
            || limits.max_pixels == 0
        {
            return None;
        }
        Some(limits)
    }

    pub fn admits(&self, bytes: usize, width: u32, height: u32) -> bool {
        bytes > 0 && bytes <= self.max_bytes && self.admits_dimensions(width, height)
    }

    pub fn admits_dimensions(&self, width: u32, height: u32) -> bool {
        width > 0
            && height > 0
            && width <= self.max_width
            && height <= self.max_height
            && u64::from(width) * u64::from(height) <= self.max_pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn payload() -> CapabilitiesPayload {
        CapabilitiesPayload {
            formats: vec![ClipboardFormat::Image {
                mime_type: "image/png".into(),
            }],
            compression: vec![],
            max_message_size: 100,
            streaming_support: false,
            keepalive_interval: 10,
            extensions: HashMap::from([
                (MAX_WIDTH.into(), "10".into()),
                (MAX_HEIGHT.into(), "5".into()),
                (MAX_PIXELS.into(), "40".into()),
            ]),
        }
    }

    #[test]
    fn rejects_missing_invalid_zero_and_overflowing_limits() {
        let p = payload();
        for bad in [
            "",
            "0",
            "-1",
            "+1",
            " 1",
            "999999999999999999999999999999999999",
        ] {
            let mut value = p.clone();
            value.extensions.insert(MAX_WIDTH.into(), bad.into());
            assert_eq!(ImageLimits::from_capabilities(&value), None, "{bad}");
        }
        let mut missing = p.clone();
        missing.extensions.remove(MAX_PIXELS);
        assert_eq!(ImageLimits::from_capabilities(&missing), None);
        let mut absent = p;
        absent.formats.clear();
        assert_eq!(ImageLimits::from_capabilities(&absent), None);
    }

    #[test]
    fn admits_only_within_all_advertised_bounds() {
        let l = ImageLimits::from_capabilities(&payload()).unwrap();
        assert!(l.admits(100, 10, 4));
        assert!(!l.admits(101, 10, 4));
        assert!(!l.admits(100, 10, 5));
        assert!(!l.admits(100, 11, 1));
        assert!(!l.admits(100, 1, 6));
        assert!(!l.admits(0, 1, 1));
    }
}

//! Preview-only history page builders for control IPC.
use crate::{
    adapters::{ClipboardData, ClipboardEntry},
    control::schema::{
        HistoryEntryDetail, HistoryEntryView, HistoryPage, HISTORY_PAGE_MAX, HISTORY_PREVIEW_CHARS,
        HISTORY_QUERY_MAX_BYTES,
    },
};
use anyhow::{bail, Result};
use uuid::Uuid;

pub fn clamp_limit(limit: u32) -> u32 {
    limit.clamp(1, HISTORY_PAGE_MAX)
}

pub fn validate_query(query: &str) -> Result<()> {
    if query.len() > HISTORY_QUERY_MAX_BYTES {
        bail!("History query too long");
    }
    Ok(())
}

/// Fold newlines and other control characters to spaces; truncate to preview budget.
pub fn preview_text(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let count = folded.chars().count();
    if count <= HISTORY_PREVIEW_CHARS {
        return folded;
    }
    folded.chars().take(HISTORY_PREVIEW_CHARS).collect()
}

fn entry_text(entry: &ClipboardEntry) -> Option<&str> {
    match &entry.content {
        ClipboardData::Text(text) => Some(text.as_str()),
        ClipboardData::Image(_) => None,
    }
}

fn content_type(entry: &ClipboardEntry) -> &'static str {
    match &entry.content {
        ClipboardData::Text(_) => "text/plain",
        ClipboardData::Image(_) => "image/png",
    }
}

pub fn to_view(entry: &ClipboardEntry) -> Option<HistoryEntryView> {
    let text = entry_text(entry).unwrap_or("[Image]");
    Some(HistoryEntryView {
        id: entry.id,
        timestamp: entry.timestamp.to_rfc3339(),
        origin_node: entry.source,
        content_type: content_type(entry).into(),
        content_chars: entry_text(entry).map_or(0, |text| text.chars().count() as u32),
        preview: preview_text(text),
    })
}

pub fn to_detail(entry: &ClipboardEntry, max_bytes: usize) -> Result<HistoryEntryDetail> {
    let text = entry_text(entry).unwrap_or("[Image]").to_string();
    if text.len() > max_bytes {
        bail!("Clipboard exceeds configured size limit");
    }
    let view = to_view(entry).ok_or_else(|| anyhow::anyhow!("History entry not found"))?;
    Ok(HistoryEntryDetail {
        id: view.id,
        timestamp: view.timestamp,
        origin_node: view.origin_node,
        content_type: view.content_type,
        content_chars: view.content_chars,
        preview: view.preview,
        text,
        image_preview: None,
    })
}

/// Decode under the same admission limits as sync, then send bounded previews
/// over the protected local control socket. The small RGB fallback composites
/// transparent pixels against the terminal preview's dark background.
pub fn image_preview(
    data: &[u8],
    limits: crate::transport::image_capability::ImageLimits,
    budget: u64,
    png_budget: usize,
) -> Result<super::schema::ImagePreview> {
    use image::{DynamicImage, RgbaImage};
    let decoded = crate::clipboard::png::decode(data, limits, budget)?;
    let image = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or_else(|| anyhow::anyhow!("Invalid image dimensions"))?;
    use base64::Engine;
    use std::io::Cursor;
    // Encode a separately resized preview, never the original clipboard bytes.
    // Leave enough frame space for the fallback RGB thumbnail and metadata.
    let image = DynamicImage::ImageRgba8(image);
    let mut png = None;
    for size in [1600, 1024, 768, 512, 256] {
        let high = if image.width() <= size && image.height() <= size {
            image.clone()
        } else {
            image.thumbnail(size, size)
        };
        let mut encoded = Cursor::new(Vec::new());
        if high.write_to(&mut encoded, image::ImageFormat::Png).is_ok()
            && encoded.get_ref().len() <= png_budget
        {
            png = Some(base64::engine::general_purpose::STANDARD.encode(encoded.into_inner()));
            break;
        }
    }
    let size = super::schema::IMAGE_PREVIEW_MAX_DIMENSION;
    let preview = if decoded.width > size || decoded.height > size {
        image.thumbnail(size, size).to_rgba8()
    } else {
        image.into_rgba8()
    };
    let (width, height) = preview.dimensions();
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for pixel in preview.pixels() {
        let alpha = u32::from(pixel[3]);
        for color in &pixel.0[..3] {
            rgb.push(((u32::from(*color) * alpha + 24 * (255 - alpha) + 127) / 255) as u8);
        }
    }
    Ok(super::schema::ImagePreview {
        width,
        height,
        rgb,
        png,
    })
}

pub fn page_from_entries(
    entries: &[ClipboardEntry],
    query: Option<String>,
    limit: u32,
) -> HistoryPage {
    let limit = clamp_limit(limit) as usize;
    let mut views = Vec::new();
    let mut truncated = false;
    for entry in entries.iter().take(limit) {
        if let Some(view) = to_view(entry) {
            if view.preview.chars().count()
                < entry_text(entry).map(|t| t.chars().count()).unwrap_or(0)
            {
                truncated = true;
            }
            views.push(view);
        }
    }
    HistoryPage {
        query,
        total_returned: views.len() as u32,
        truncated,
        entries: views,
    }
}

pub fn missing_id_message(_id: Uuid) -> &'static str {
    "History entry not found"
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use base64::Engine;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    #[test]
    fn preview_retains_detail_and_respects_wire_budget() {
        let image = RgbaImage::from_fn(640, 400, |x, y| {
            Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                ((x * 7 + y * 13) % 256) as u8,
                255,
            ])
        });
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let limits = crate::transport::image_capability::ImageLimits {
            max_bytes: bytes.len(),
            max_width: 8192,
            max_height: 8192,
            max_pixels: 16_777_216,
        };
        let result = image_preview(&bytes, limits, 64 * 1024 * 1024, 2 * 1024 * 1024).unwrap();
        assert!(result.is_valid());
        let png = base64::engine::general_purpose::STANDARD
            .decode(result.png.unwrap())
            .unwrap();
        let image = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
        assert_eq!((image.width(), image.height()), (640, 400));
        let limited = image_preview(&bytes, limits, 64 * 1024 * 1024, 1).unwrap();
        assert!(limited.png.is_none());
        assert!(limited.is_valid());
    }
}

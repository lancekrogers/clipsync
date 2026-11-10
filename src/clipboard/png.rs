//! PNG admission with caller-supplied limits before pixel allocation.

use std::io::Cursor;

use image::{codecs::png::PngDecoder, DynamicImage, ImageDecoder, Limits};

use super::ClipboardError;
use crate::transport::image_capability::ImageLimits;

/// TIFF is an intermediate native representation and can be much larger than
/// the normalized PNG. Bound it separately from the encoded wire limit.
pub const MAX_NATIVE_TIFF_BYTES: usize = 64 * 1024 * 1024;

pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Validate and decode a single-frame PNG. The decoder's allocation budget is
/// best-effort; callers must also bound encoded input and concurrent decodes.
pub fn decode(
    data: &[u8],
    bounds: ImageLimits,
    decoder_budget: u64,
) -> Result<DecodedPng, ClipboardError> {
    if data.is_empty() || data.len() > bounds.max_bytes || decoder_budget == 0 {
        return Err(ClipboardError::Platform(
            "PNG exceeds encoded size limit".into(),
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(bounds.max_width);
    limits.max_image_height = Some(bounds.max_height);
    limits.max_alloc = Some(decoder_budget);
    let decoder = PngDecoder::with_limits(Cursor::new(data), limits)
        .map_err(|e| ClipboardError::Platform(format!("Invalid PNG: {e}")))?;
    if decoder
        .is_apng()
        .map_err(|e| ClipboardError::Platform(format!("Invalid PNG: {e}")))?
    {
        return Err(ClipboardError::UnsupportedType("animated PNG".into()));
    }
    let (width, height) = decoder.dimensions();
    if !bounds.admits(data.len(), width, height) {
        return Err(ClipboardError::Platform("PNG exceeds image limits".into()));
    }
    // DynamicImage conversion may allocate both the decoded source and RGBA.
    // Refuse budgets too small for the maximum 16-bit RGBA source plus RGBA8.
    let required = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(12))
        .ok_or_else(|| ClipboardError::Platform("PNG dimensions overflow".into()))?;
    if required > decoder_budget {
        return Err(ClipboardError::Platform(
            "PNG exceeds decoded memory budget".into(),
        ));
    }
    let rgba = DynamicImage::from_decoder(decoder)
        .map_err(|e| ClipboardError::Platform(format!("Invalid PNG: {e}")))?
        .into_rgba8()
        .into_raw();
    Ok(DecodedPng {
        width,
        height,
        rgba,
    })
}

/// Convert a bounded TIFF input to a PNG. The decoder's internal allocations
/// are best-effort limited; this alone is not an aggregate memory guarantee.
pub fn from_tiff(
    data: &[u8],
    bounds: ImageLimits,
    decoder_budget: u64,
) -> Result<Vec<u8>, ClipboardError> {
    use image::{
        codecs::{png::PngEncoder, tiff::TiffDecoder},
        ExtendedColorType, ImageEncoder,
    };
    use std::io::Write;
    if data.is_empty() || data.len() > MAX_NATIVE_TIFF_BYTES || decoder_budget == 0 {
        return Err(ClipboardError::Platform(
            "TIFF exceeds native input limit".into(),
        ));
    }
    let mut decoder = TiffDecoder::new(Cursor::new(data))
        .map_err(|e| ClipboardError::Platform(format!("Invalid TIFF: {e}")))?;
    // Floating-point TIFF can require substantially larger conversion buffers.
    // Keep this path to integer formats until that allocation is accounted for.
    if decoder.color_type().bytes_per_pixel() > 8 {
        return Err(ClipboardError::UnsupportedType(
            "floating-point TIFF".into(),
        ));
    }
    let (width, height) = decoder.dimensions();
    if !bounds.admits_dimensions(width, height) {
        return Err(ClipboardError::Platform("TIFF exceeds image limits".into()));
    }
    let required = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(20))
        .ok_or_else(|| ClipboardError::Platform("TIFF dimensions overflow".into()))?;
    if required > decoder_budget {
        return Err(ClipboardError::Platform(
            "TIFF exceeds decoded memory budget".into(),
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(bounds.max_width);
    limits.max_image_height = Some(bounds.max_height);
    limits.max_alloc = Some(decoder_budget);
    decoder
        .set_limits(limits)
        .map_err(|e| ClipboardError::Platform(format!("TIFF limits: {e}")))?;
    let rgba = DynamicImage::from_decoder(decoder)
        .map_err(|e| ClipboardError::Platform(format!("Invalid TIFF: {e}")))?
        .into_rgba8();
    struct LimitedWriter {
        bytes: Vec<u8>,
        limit: usize,
        overflowed: bool,
    }
    impl Write for LimitedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                self.overflowed = true;
                return Err(std::io::Error::other("PNG exceeds encoded limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit: bounds.max_bytes,
        overflowed: false,
    };
    PngEncoder::new(&mut writer)
        .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| ClipboardError::Platform(format!("TIFF to PNG: {e}")))?;
    // The PNG encoder can emit its final trailer while dropping its internal
    // writer, where an I/O error is not returned by `write_image`.
    if writer.overflowed {
        return Err(ClipboardError::Platform("PNG exceeds encoded limit".into()));
    }
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn fixture() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=").unwrap()
    }
    fn limits() -> ImageLimits {
        ImageLimits {
            max_bytes: 1024,
            max_width: 2,
            max_height: 1,
            max_pixels: 2,
        }
    }
    #[test]
    fn decodes_pixels_and_enforces_limits() {
        let data = fixture();
        let result = decode(&data, limits(), 4096).unwrap();
        assert_eq!((result.width, result.height), (2, 1));
        assert_eq!(result.rgba, [255, 0, 0, 255, 0, 128, 255, 128]);
        let mut small = limits();
        small.max_bytes = data.len() - 1;
        assert!(decode(&data, small, 4096).is_err());
        small = limits();
        small.max_pixels = 1;
        assert!(decode(&data, small, 4096).is_err());
        small = limits();
        small.max_width = 1;
        assert!(decode(&data, small, 4096).is_err());
        assert!(decode(&data, limits(), 12).is_err());
    }
    #[test]
    fn converts_tiff_with_alpha_and_enforces_bounds() {
        use image::codecs::tiff::TiffEncoder;
        let mut out = Cursor::new(Vec::new());
        TiffEncoder::new(&mut out)
            .encode(
                &[255, 0, 0, 255, 0, 128, 255, 128],
                2,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let tiff = out.into_inner();
        let converted = from_tiff(&tiff, limits(), 4096).unwrap();
        let decoded = decode(&converted, limits(), 4096).unwrap();
        assert_eq!(decoded.rgba, [255, 0, 0, 255, 0, 128, 255, 128]);
        let mut small = limits();
        small.max_pixels = 1;
        assert!(from_tiff(&tiff, small, 4096).is_err());
        assert!(from_tiff(b"not a tiff", limits(), 4096).is_err());
    }

    #[test]
    fn larger_native_tiff_can_normalize_within_png_limit() {
        use image::codecs::tiff::TiffEncoder;
        let (width, height) = (2048, 1024);
        let pixels = vec![255u8; width as usize * height as usize * 4];
        let mut out = Cursor::new(Vec::new());
        TiffEncoder::new(&mut out)
            .encode(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        let tiff = out.into_inner();
        let limits = ImageLimits {
            max_bytes: 5 * 1024 * 1024,
            max_width: width,
            max_height: height,
            max_pixels: u64::from(width) * u64::from(height),
        };
        assert!(tiff.len() > limits.max_bytes);
        let converted = from_tiff(&tiff, limits, 64 * 1024 * 1024).unwrap();
        assert!(converted.len() < limits.max_bytes);
        assert_eq!(
            decode(&converted, limits, 64 * 1024 * 1024).unwrap().rgba,
            pixels
        );
        let mut too_small = limits;
        too_small.max_bytes = converted.len() - 1;
        assert!(from_tiff(&tiff, too_small, 64 * 1024 * 1024).is_err());
    }

    #[test]
    #[ignore = "allocates a synthetic 5K RGBA image for resource-limit verification"]
    fn synthetic_5k_image_fits_proposed_limits() {
        use image::ImageEncoder;
        let width = 5120u32;
        let height = 2880u32;
        let pixels = vec![128u8; width as usize * height as usize * 4];
        let mut data = Vec::new();
        image::codecs::png::PngEncoder::new(&mut data)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        let limits = ImageLimits {
            max_bytes: 5 * 1024 * 1024,
            max_width: 8192,
            max_height: 8192,
            max_pixels: 16_777_216,
        };
        let decoded = decode(&data, limits, 384 * 1024 * 1024).unwrap();
        assert_eq!((decoded.width, decoded.height), (width, height));
        assert_eq!(decoded.rgba.len(), pixels.len());
        assert_eq!(&decoded.rgba[..4], &[128, 128, 128, 128]);
    }

    #[test]
    fn rejects_truncated_or_wrong_format() {
        let data = fixture();
        assert!(decode(&data[..data.len() / 2], limits(), 4096).is_err());
        assert!(decode(b"not a png", limits(), 4096).is_err());
    }
}

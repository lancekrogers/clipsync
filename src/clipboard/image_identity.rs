//! Stable pixel identity for clipboard image observation, separate from wire checksums.

use sha2::{Digest, Sha256};

use super::ClipboardError;

/// Hash tightly packed, top-to-bottom, straight-alpha RGBA8 pixels.
/// Dimensions are encoded as unsigned big-endian 32-bit integers. Fully
/// transparent RGB is normalized for identity; partially transparent RGB is not.
pub fn rgba8_identity(width: u32, height: u32, pixels: &[u8]) -> Result<String, ClipboardError> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|n| n.checked_mul(4))
        .filter(|_| width != 0 && height != 0)
        .ok_or_else(|| ClipboardError::Platform("Invalid image dimensions".into()))?;
    if pixels.len() != expected {
        return Err(ClipboardError::Platform("Invalid RGBA image length".into()));
    }
    let mut hash = Sha256::new();
    hash.update(b"clipsync-image-rgba8-v1");
    hash.update(width.to_be_bytes());
    hash.update(height.to_be_bytes());
    for pixel in pixels.chunks_exact(4) {
        if pixel[3] == 0 {
            hash.update([0, 0, 0, 0]);
        } else {
            hash.update(pixel);
        }
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::rgba8_identity;

    #[test]
    fn normalizes_only_fully_transparent_rgb() {
        let a = [1, 2, 3, 0, 4, 5, 6, 127];
        let b = [9, 8, 7, 0, 4, 5, 6, 127];
        assert_eq!(
            rgba8_identity(2, 1, &a).unwrap(),
            "4f53f977c574ed71a4ddfa4732b9df21ecfe3d94afe297268a72b26871d7a674"
        );
        let c = [9, 8, 7, 0, 4, 5, 7, 127];
        assert_eq!(
            rgba8_identity(2, 1, &a).unwrap(),
            rgba8_identity(2, 1, &b).unwrap()
        );
        assert_ne!(
            rgba8_identity(2, 1, &a).unwrap(),
            rgba8_identity(2, 1, &c).unwrap()
        );
        assert_ne!(
            rgba8_identity(2, 1, &a).unwrap(),
            rgba8_identity(1, 2, &a).unwrap()
        );
    }

    #[test]
    fn rejects_invalid_dimensions_and_lengths() {
        assert!(rgba8_identity(0, 1, &[]).is_err());
        assert!(rgba8_identity(1, 1, &[0; 3]).is_err());
        assert!(rgba8_identity(u32::MAX, u32::MAX, &[]).is_err());
    }
}

//! Negotiation for history removal messages.
//!
//! Peers that predate `MessageType::HistoryOperation` fail to parse it and drop the
//! session, so it is only sent to sessions whose capabilities carry this key.

use super::protocol::CapabilitiesPayload;

pub const HISTORY_OPS: &str = "clipsync.history.ops";
const HISTORY_OPS_VERSION: &str = "1";

pub fn advertise(payload: &mut CapabilitiesPayload) {
    payload
        .extensions
        .insert(HISTORY_OPS.into(), HISTORY_OPS_VERSION.into());
}

pub fn supported(payload: &CapabilitiesPayload) -> bool {
    payload
        .extensions
        .get(HISTORY_OPS)
        .is_some_and(|version| version == HISTORY_OPS_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::image_capability::ImageLimits;

    #[test]
    fn requires_the_exact_version() {
        let mut payload = ImageLimits {
            max_bytes: 10,
            max_width: 1,
            max_height: 1,
            max_pixels: 1,
        }
        .capabilities();
        assert!(!supported(&payload));
        payload.extensions.insert(HISTORY_OPS.into(), "2".into());
        assert!(!supported(&payload));
        advertise(&mut payload);
        assert!(supported(&payload));
        assert!(ImageLimits::from_capabilities(&payload).is_some());
    }
}

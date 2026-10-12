//! Outbound dial policy for discovery-driven peer connections.

use std::time::{Duration, Instant};
use uuid::Uuid;

/// Grace period before the higher node id may dial a lower-id peer when the preferred path failed.
pub const FALLBACK_INITIATOR_GRACE: Duration = Duration::from_secs(3);

/// Preferred outbound initiator: the node with the lower id dials toward a higher-id peer.
pub fn preferred_initiator(local_id: Uuid, peer_id: Uuid) -> bool {
    local_id < peer_id
}

/// Whether this node should attempt an outbound dial to `peer_id` right now.
pub fn should_attempt_outbound_dial(
    local_id: Uuid,
    peer_id: Uuid,
    connected: bool,
    backoff_until: Option<Instant>,
    peer_first_seen: Instant,
    now: Instant,
) -> bool {
    if connected || peer_id == local_id {
        return false;
    }
    if backoff_until.is_some_and(|until| until > now) {
        return false;
    }
    if preferred_initiator(local_id, peer_id) {
        return true;
    }
    now.duration_since(peer_first_seen) >= FALLBACK_INITIATOR_GRACE
}

/// Exponential backoff after a failed dial attempt.
pub fn dial_backoff(attempt: u32) -> Duration {
    let capped = attempt.min(5);
    Duration::from_secs(1_u64 << capped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid) {
        let low = Uuid::from_u128(1);
        let high = Uuid::from_u128(2);
        (low, high)
    }

    #[test]
    fn preferred_initiator_is_lower_id_toward_higher_peer() {
        let (low, high) = ids();
        assert!(preferred_initiator(low, high));
        assert!(!preferred_initiator(high, low));
    }

    #[test]
    fn lower_id_dials_higher_peer_immediately_when_not_connected() {
        let (low, high) = ids();
        let now = Instant::now();
        assert!(should_attempt_outbound_dial(
            low, high, false, None, now, now
        ));
    }

    #[test]
    fn higher_id_waits_for_fallback_grace_before_dialing_lower_peer() {
        let (low, high) = ids();
        let now = Instant::now();
        assert!(!should_attempt_outbound_dial(
            high, low, false, None, now, now
        ));
        assert!(should_attempt_outbound_dial(
            high,
            low,
            false,
            None,
            now - FALLBACK_INITIATOR_GRACE,
            now
        ));
    }

    #[test]
    fn both_orderings_can_dial_after_grace_when_still_disconnected() {
        let (low, high) = ids();
        let now = Instant::now();
        let seen = now - FALLBACK_INITIATOR_GRACE - Duration::from_millis(1);
        assert!(should_attempt_outbound_dial(
            low, high, false, None, seen, now
        ));
        assert!(should_attempt_outbound_dial(
            high, low, false, None, seen, now
        ));
    }

    #[test]
    fn connected_and_backoff_suppress_dial() {
        let (low, high) = ids();
        let now = Instant::now();
        assert!(!should_attempt_outbound_dial(
            low, high, true, None, now, now
        ));
        assert!(!should_attempt_outbound_dial(
            low,
            high,
            false,
            Some(now + Duration::from_secs(30)),
            now,
            now
        ));
    }
}

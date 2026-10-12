//! Discovery-driven outbound connection attempts.

use crate::discovery::PeerInfo;
use crate::sync::connection_policy::{dial_backoff, should_attempt_outbound_dial};
use crate::transport::TransportManager;
use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

pub struct PeerDialState {
    pub first_seen: Instant,
    pub backoff_until: Instant,
    pub attempts: u32,
}

pub async fn discovery_connect_peers(
    transport: &TransportManager,
    local_id: Uuid,
    peers: &[PeerInfo],
    retry: &mut HashMap<Uuid, PeerDialState>,
    now: Instant,
) {
    retry.retain(|id, _| peers.iter().any(|p| p.id == *id));
    for peer in peers {
        let connected = transport.is_connected(peer.id).await;
        let state = retry.entry(peer.id).or_insert_with(|| PeerDialState {
            first_seen: now,
            backoff_until: now,
            attempts: 0,
        });
        if !should_attempt_outbound_dial(
            local_id,
            peer.id,
            connected,
            Some(state.backoff_until),
            state.first_seen,
            now,
        ) {
            continue;
        }
        match transport.connect_peer(peer).await {
            Ok(()) => {
                retry.remove(&peer.id);
            }
            Err(e) => {
                tracing::debug!("Peer {} connect failed: {e}", peer.id);
                state.attempts = state.attempts.saturating_add(1).min(5);
                state.backoff_until = now + dial_backoff(state.attempts);
            }
        }
    }
}

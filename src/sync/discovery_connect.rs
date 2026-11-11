//! Discovery-driven outbound connection attempts.

use crate::discovery::PeerInfo;
use crate::sync::connection_policy::{
    dial_backoff, should_attempt_outbound_dial, MAX_DIAL_ATTEMPTS,
};
use crate::sync::known_peers::KnownPeers;
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
    connect_peers(transport, local_id, peers, retry, now, None).await;
}

/// Dial each disconnected peer once per backoff window. Addresses that authenticate
/// are remembered in `known` so a restart can redial without discovery.
pub async fn connect_peers(
    transport: &TransportManager,
    local_id: Uuid,
    peers: &[PeerInfo],
    retry: &mut HashMap<Uuid, PeerDialState>,
    now: Instant,
    known: Option<&KnownPeers>,
) {
    retry.retain(|id, _| peers.iter().any(|p| p.id == *id));
    for peer in peers {
        let connected = transport.is_connected(peer.id).await;
        let state = retry.entry(peer.id).or_insert_with(|| PeerDialState {
            first_seen: now,
            backoff_until: now,
            attempts: 0,
        });
        if connected {
            // The next disconnect starts a fresh streak, even after an inbound session.
            state.attempts = 0;
            state.backoff_until = now;
        }
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
        let candidates = peer.connect_candidates();
        if state.attempts == 0 {
            let action = match known {
                Some(known) if known.contains(peer.id).await => "Redialing known",
                _ => "Dialing",
            };
            tracing::info!(
                "{action} peer {} ({}) at {candidates:?}",
                peer.id,
                peer.name
            );
        }
        match transport.dial_peer(peer).await {
            Ok(verified) => {
                retry.remove(&peer.id);
                if let (Some(known), Some(addr)) = (known, verified) {
                    if let Err(e) = known.record(peer.id, &peer.name, addr).await {
                        tracing::warn!("Could not remember address for peer {}: {e:#}", peer.id);
                    }
                }
            }
            Err(e) => {
                let reached_cap = state.attempts + 1 == MAX_DIAL_ATTEMPTS;
                state.attempts = state.attempts.saturating_add(1).min(MAX_DIAL_ATTEMPTS);
                let backoff = dial_backoff(state.attempts);
                state.backoff_until = now + backoff;
                if reached_cap {
                    tracing::info!(
                        "Peer {} still unreachable after {} attempts; retrying every {}s: {e}",
                        peer.id,
                        state.attempts,
                        backoff.as_secs()
                    );
                } else {
                    tracing::debug!("Peer {} connect failed: {e}", peer.id);
                }
            }
        }
    }
}

//! Peer management and tracking

use crate::discovery::types::{DiscoveryEvent, DiscoveryMethod, PeerInfo};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

const PEER_TIMEOUT_SECS: i64 = 300; // 5 minutes
const CLEANUP_INTERVAL_SECS: u64 = 60; // 1 minute

/// Manages discovered peers and their lifecycle
#[derive(Clone)]
pub struct PeerManager {
    inner: Arc<PeerManagerInner>,
}

struct PeerManagerInner {
    /// Map of peer ID to peer info
    peers: RwLock<HashMap<Uuid, PeerEntry>>,
    /// Event broadcaster
    event_tx: Mutex<mpsc::Sender<DiscoveryEvent>>,
    /// Event receivers
    event_listeners: Mutex<Vec<mpsc::Sender<DiscoveryEvent>>>,
}

/// Internal peer entry with additional tracking
struct PeerEntry {
    info: PeerInfo,
    discovery_method: DiscoveryMethod,
    first_seen: i64,
    consecutive_failures: u32,
}

impl PeerManager {
    /// Create a new peer manager
    pub fn new() -> Self {
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let inner = Arc::new(PeerManagerInner {
            peers: RwLock::new(HashMap::new()),
            event_tx: Mutex::new(event_tx),
            event_listeners: Mutex::new(Vec::new()),
        });

        // Spawn event broadcaster
        let inner_clone = inner.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let listeners = inner_clone.event_listeners.lock().await;
                for listener in listeners.iter() {
                    let _ = listener.send(event.clone()).await;
                }
            }
        });

        // Spawn cleanup task
        let inner_clone = inner.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let _ = Self::cleanup_stale_peers(&inner_clone).await;
            }
        });

        Self { inner }
    }

    /// Add or update a peer
    pub async fn add_peer(&self, peer: PeerInfo, method: DiscoveryMethod) -> Result<()> {
        let mut peers = self.inner.peers.write().await;
        let now = Utc::now().timestamp();

        let event = if let Some(existing) = peers.get_mut(&peer.id) {
            // Update existing peer
            existing.info = peer.clone();
            existing.info.last_seen = now;
            existing.consecutive_failures = 0;
            DiscoveryEvent::PeerUpdated(peer)
        } else {
            // New peer
            peers.insert(
                peer.id,
                PeerEntry {
                    info: peer.clone(),
                    discovery_method: method,
                    first_seen: now,
                    consecutive_failures: 0,
                },
            );
            DiscoveryEvent::PeerDiscovered(peer)
        };

        // Send event
        drop(peers);
        self.send_event(event).await;

        Ok(())
    }

    /// Remove a peer
    pub async fn remove_peer(&self, peer_id: Uuid) -> Result<()> {
        let mut peers = self.inner.peers.write().await;
        if peers.remove(&peer_id).is_some() {
            drop(peers);
            self.send_event(DiscoveryEvent::PeerLost(peer_id)).await;
        }
        Ok(())
    }

    /// Mark a peer as failed
    pub async fn mark_peer_failed(&self, peer_id: Uuid) -> Result<()> {
        let mut peers = self.inner.peers.write().await;
        if let Some(entry) = peers.get_mut(&peer_id) {
            entry.consecutive_failures += 1;

            // Remove peer after 3 consecutive failures
            if entry.consecutive_failures >= 3 {
                drop(peers);
                self.remove_peer(peer_id).await?;
            }
        }
        Ok(())
    }

    /// Get a specific peer
    pub async fn get_peer(&self, peer_id: Uuid) -> Option<PeerInfo> {
        let peers = self.inner.peers.read().await;
        peers.get(&peer_id).map(|entry| entry.info.clone())
    }

    /// Get all active peers
    pub async fn get_all_peers(&self) -> Result<Vec<PeerInfo>> {
        let peers = self.inner.peers.read().await;
        Ok(peers.values().map(|entry| entry.info.clone()).collect())
    }

    /// Get peers discovered by a specific method
    pub async fn get_peers_by_method(&self, method: DiscoveryMethod) -> Result<Vec<PeerInfo>> {
        let peers = self.inner.peers.read().await;
        Ok(peers
            .values()
            .filter(|entry| entry.discovery_method == method)
            .map(|entry| entry.info.clone())
            .collect())
    }

    /// Subscribe to peer events
    pub fn subscribe(&self) -> mpsc::Receiver<DiscoveryEvent> {
        let (tx, rx) = mpsc::channel(100);

        // Clone the inner reference for the spawned task
        let inner = self.inner.clone();
        tokio::spawn(async move {
            inner.event_listeners.lock().await.push(tx);
        });

        rx
    }

    /// Send an event to all listeners
    async fn send_event(&self, event: DiscoveryEvent) {
        let tx = self.inner.event_tx.lock().await;
        let _ = tx.send(event).await;
    }

    /// Clean up stale peers
    async fn cleanup_stale_peers(inner: &Arc<PeerManagerInner>) -> Result<()> {
        let now = Utc::now().timestamp();
        let mut peers = inner.peers.write().await;
        let mut to_remove = Vec::new();

        for (id, entry) in peers.iter() {
            // Skip manually configured peers
            if entry.discovery_method == DiscoveryMethod::Manual {
                continue;
            }

            // Check if peer has timed out
            if now - entry.info.last_seen > PEER_TIMEOUT_SECS {
                to_remove.push(*id);
            }
        }

        // Remove stale peers
        for id in to_remove {
            peers.remove(&id);

            // Send event
            let tx = inner.event_tx.lock().await;
            let _ = tx.send(DiscoveryEvent::PeerLost(id)).await;
        }

        Ok(())
    }

    /// Update peer's last seen time
    pub async fn touch_peer(&self, peer_id: Uuid) -> Result<()> {
        let mut peers = self.inner.peers.write().await;
        if let Some(entry) = peers.get_mut(&peer_id) {
            entry.info.last_seen = Utc::now().timestamp();
            entry.consecutive_failures = 0;
        }
        Ok(())
    }

    /// Get peer statistics
    pub async fn get_stats(&self) -> PeerStats {
        let peers = self.inner.peers.read().await;

        let mut stats = PeerStats::default();
        stats.total_peers = peers.len();

        for entry in peers.values() {
            match entry.discovery_method {
                DiscoveryMethod::Mdns => stats.mdns_peers += 1,
                DiscoveryMethod::Manual => stats.manual_peers += 1,
                DiscoveryMethod::Broadcast => stats.broadcast_peers += 1,
                DiscoveryMethod::CloudRelay => stats.cloud_peers += 1,
            }

            if entry.consecutive_failures > 0 {
                stats.failing_peers += 1;
            }
        }

        stats
    }
}

/// Statistics about managed peers
#[derive(Debug, Default, Clone)]
pub struct PeerStats {
    pub total_peers: usize,
    pub mdns_peers: usize,
    pub manual_peers: usize,
    pub broadcast_peers: usize,
    pub cloud_peers: usize,
    pub failing_peers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::types::PeerMetadata;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn create_test_peer(id: Uuid, name: &str) -> PeerInfo {
        PeerInfo {
            id,
            name: name.to_string(),
            addresses: vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
                9090,
            )],
            port: 9090,
            version: "1.0.0".to_string(),
            platform: "test".to_string(),
            metadata: PeerMetadata {
                ssh_fingerprint: None,
                capabilities: vec![],
                device_name: None,
            },
            last_seen: Utc::now().timestamp(),
        }
    }

    #[tokio::test]
    async fn test_add_and_get_peer() {
        let manager = PeerManager::new();
        let peer_id = Uuid::new_v4();
        let peer = create_test_peer(peer_id, "test-peer");

        manager
            .add_peer(peer.clone(), DiscoveryMethod::Mdns)
            .await
            .unwrap();

        let retrieved = manager.get_peer(peer_id).await.unwrap();
        assert_eq!(retrieved.id, peer_id);
        assert_eq!(retrieved.name, "test-peer");
    }

    #[tokio::test]
    async fn test_remove_peer() {
        let manager = PeerManager::new();
        let peer_id = Uuid::new_v4();
        let peer = create_test_peer(peer_id, "test-peer");

        manager.add_peer(peer, DiscoveryMethod::Mdns).await.unwrap();
        assert!(manager.get_peer(peer_id).await.is_some());

        manager.remove_peer(peer_id).await.unwrap();
        assert!(manager.get_peer(peer_id).await.is_none());
    }

    #[tokio::test]
    async fn test_peer_failure_tracking() {
        let manager = PeerManager::new();
        let peer_id = Uuid::new_v4();
        let peer = create_test_peer(peer_id, "test-peer");

        manager.add_peer(peer, DiscoveryMethod::Mdns).await.unwrap();

        // First two failures should not remove peer
        manager.mark_peer_failed(peer_id).await.unwrap();
        assert!(manager.get_peer(peer_id).await.is_some());

        manager.mark_peer_failed(peer_id).await.unwrap();
        assert!(manager.get_peer(peer_id).await.is_some());

        // Third failure should remove peer
        manager.mark_peer_failed(peer_id).await.unwrap();
        assert!(manager.get_peer(peer_id).await.is_none());
    }

    #[tokio::test]
    async fn test_peer_events() {
        let manager = PeerManager::new();

        // Give a small delay for event system to initialize
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let mut receiver = manager.subscribe();

        let peer_id = Uuid::new_v4();
        let peer = create_test_peer(peer_id, "test-peer");

        // Add peer
        manager
            .add_peer(peer.clone(), DiscoveryMethod::Mdns)
            .await
            .unwrap();

        // Should receive discovered event
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(event, DiscoveryEvent::PeerDiscovered(_)));

        // Update peer
        manager.add_peer(peer, DiscoveryMethod::Mdns).await.unwrap();

        // Should receive updated event
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(event, DiscoveryEvent::PeerUpdated(_)));
    }
}

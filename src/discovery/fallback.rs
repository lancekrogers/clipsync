//! Fallback discovery mechanisms for when mDNS is unavailable

use crate::discovery::{
    types::{DiscoveryMethod, PeerMetadata},
    Discovery, DiscoveryEvent, PeerInfo, PeerManager, ServiceInfo,
};
use crate::Config;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{interval, Duration};
use uuid::Uuid;

const BROADCAST_PORT: u16 = 9091;
const BROADCAST_INTERVAL_SECS: u64 = 30;
const BROADCAST_MAGIC: &[u8] = b"CLIPSYNC";

/// Fallback discovery using manual configuration and broadcast
pub struct FallbackDiscovery {
    peer_manager: PeerManager,
    config: Arc<Config>,
    manual_peers: Arc<RwLock<Vec<ManualPeer>>>,
    broadcast_socket: Arc<Mutex<Option<UdpSocket>>>,
    broadcast_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    our_info: Arc<RwLock<Option<ServiceInfo>>>,
}

/// Manually configured peer
#[derive(Debug, Clone)]
struct ManualPeer {
    id: Option<Uuid>,
    address: String,
    port: u16,
}

impl FallbackDiscovery {
    /// Create new fallback discovery
    pub fn new(peer_manager: PeerManager, config: &Config) -> Result<Self> {
        // Parse manual peers from config
        let manual_peers = Self::parse_manual_peers(config)?;

        Ok(Self {
            peer_manager,
            config: Arc::new(config.clone()),
            manual_peers: Arc::new(RwLock::new(manual_peers)),
            broadcast_socket: Arc::new(Mutex::new(None)),
            broadcast_task: Arc::new(Mutex::new(None)),
            our_info: Arc::new(RwLock::new(None)),
        })
    }

    /// Parse manual peers from configuration
    fn parse_manual_peers(_config: &Config) -> Result<Vec<ManualPeer>> {
        // For now, return empty list as Config implementation details aren't finalized
        // This would be implemented based on the actual config structure
        Ok(Vec::new())
    }

    /// Start broadcast discovery
    async fn start_broadcast(&self) -> Result<()> {
        // For now, always enable broadcast discovery
        // This would check config in actual implementation

        // Bind to broadcast port
        let socket = UdpSocket::bind(("0.0.0.0", BROADCAST_PORT)).await?;
        socket.set_broadcast(true)?;

        *self.broadcast_socket.lock().await = Some(socket);

        // Start broadcast task
        let socket = self.broadcast_socket.clone();
        let our_info = self.our_info.clone();
        let peer_manager = self.peer_manager.clone();

        let handle = tokio::spawn(async move {
            Self::broadcast_loop(socket, our_info, peer_manager).await;
        });

        *self.broadcast_task.lock().await = Some(handle);

        Ok(())
    }

    /// Broadcast loop
    async fn broadcast_loop(
        socket: Arc<Mutex<Option<UdpSocket>>>,
        our_info: Arc<RwLock<Option<ServiceInfo>>>,
        peer_manager: PeerManager,
    ) {
        let mut interval = interval(Duration::from_secs(BROADCAST_INTERVAL_SECS));
        let mut recv_buf = vec![0u8; 1024];

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Send broadcast announcement
                    if let Err(e) = Self::send_broadcast(&socket, &our_info).await {
                        tracing::error!("Failed to send broadcast: {}", e);
                    }
                }
                result = Self::receive_broadcast(&socket, &mut recv_buf) => {
                    match result {
                        Ok(Some((peer_info, _addr))) => {
                            // Skip our own broadcasts
                            if let Some(our) = our_info.read().await.as_ref() {
                                if peer_info.id == our.id {
                                    continue;
                                }
                            }

                            // Add discovered peer
                            if let Err(e) = peer_manager.add_peer(
                                peer_info,
                                DiscoveryMethod::Broadcast
                            ).await {
                                tracing::error!("Failed to add broadcast peer: {}", e);
                            }
                        }
                        Ok(None) => {
                            // Timeout, continue
                        }
                        Err(e) => {
                            tracing::error!("Broadcast receive error: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Send broadcast announcement
    async fn send_broadcast(
        socket: &Arc<Mutex<Option<UdpSocket>>>,
        our_info: &Arc<RwLock<Option<ServiceInfo>>>,
    ) -> Result<()> {
        let socket_guard = socket.lock().await;
        let socket = socket_guard
            .as_ref()
            .ok_or_else(|| anyhow!("No broadcast socket"))?;

        let info = our_info.read().await;
        let info = match info.as_ref() {
            Some(info) => info,
            None => {
                // Service info not available yet - this is normal during startup
                // Skip broadcast until announce() is called
                return Ok(());
            }
        };

        // Create broadcast packet
        let packet = Self::create_broadcast_packet(info)?;

        // Send to broadcast address
        socket
            .send_to(&packet, ("255.255.255.255", BROADCAST_PORT))
            .await?;

        Ok(())
    }

    /// Receive broadcast announcement
    async fn receive_broadcast(
        socket: &Arc<Mutex<Option<UdpSocket>>>,
        buf: &mut [u8],
    ) -> Result<Option<(PeerInfo, SocketAddr)>> {
        let socket_guard = socket.lock().await;
        let socket = socket_guard
            .as_ref()
            .ok_or_else(|| anyhow!("No broadcast socket"))?;

        // Set timeout for receive
        match tokio::time::timeout(Duration::from_millis(100), socket.recv_from(buf)).await {
            Ok(Ok((len, addr))) => {
                // Parse broadcast packet
                match Self::parse_broadcast_packet(&buf[..len], addr) {
                    Ok(peer_info) => Ok(Some((peer_info, addr))),
                    Err(e) => {
                        tracing::debug!("Invalid broadcast packet from {}: {}", addr, e);
                        Ok(None)
                    }
                }
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok(None), // Timeout
        }
    }

    /// Create broadcast packet
    fn create_broadcast_packet(info: &ServiceInfo) -> Result<Vec<u8>> {
        let mut packet = Vec::new();

        // Magic bytes
        packet.extend_from_slice(BROADCAST_MAGIC);

        // Version
        packet.push(1);

        // Service info as JSON
        let json = serde_json::to_string(info)?;
        let json_bytes = json.as_bytes();

        // Length prefix
        packet.extend_from_slice(&(json_bytes.len() as u32).to_be_bytes());

        // JSON data
        packet.extend_from_slice(json_bytes);

        Ok(packet)
    }

    /// Parse broadcast packet
    fn parse_broadcast_packet(data: &[u8], from: SocketAddr) -> Result<PeerInfo> {
        if data.len() < BROADCAST_MAGIC.len() + 5 {
            return Err(anyhow!("Packet too small"));
        }

        // Check magic
        if &data[..BROADCAST_MAGIC.len()] != BROADCAST_MAGIC {
            return Err(anyhow!("Invalid magic"));
        }

        let offset = BROADCAST_MAGIC.len();

        // Check version
        if data[offset] != 1 {
            return Err(anyhow!("Unsupported version"));
        }

        // Read length
        let len_bytes: [u8; 4] = data[offset + 1..offset + 5].try_into()?;
        let json_len = u32::from_be_bytes(len_bytes) as usize;

        if data.len() < offset + 5 + json_len {
            return Err(anyhow!("Invalid length"));
        }

        // Parse JSON
        let json_data = &data[offset + 5..offset + 5 + json_len];
        let service_info: ServiceInfo = serde_json::from_slice(json_data)?;

        // Convert to PeerInfo
        let addresses = vec![SocketAddr::new(from.ip(), service_info.port)];
        let txt_data: Vec<(String, String)> = service_info.txt_data;

        Ok(PeerInfo::from_mdns(
            service_info.name,
            addresses,
            service_info.port,
            &txt_data,
        ))
    }

    /// Connect to manual peers
    async fn connect_manual_peers(&self) -> Result<()> {
        let peers = self.manual_peers.read().await;

        for manual_peer in peers.iter() {
            if let Err(e) = self.connect_manual_peer(manual_peer).await {
                tracing::warn!(
                    "Failed to connect to manual peer {}: {}",
                    manual_peer.address,
                    e
                );
            }
        }

        Ok(())
    }

    /// Connect to a single manual peer
    async fn connect_manual_peer(&self, manual_peer: &ManualPeer) -> Result<()> {
        // Resolve address
        let addr_str = format!("{}:{}", manual_peer.address, manual_peer.port);
        let addrs: Vec<SocketAddr> = addr_str.to_socket_addrs()?.collect();

        if addrs.is_empty() {
            return Err(anyhow!("Failed to resolve address"));
        }

        // Create peer info
        let peer_info = PeerInfo {
            id: manual_peer.id.unwrap_or_else(Uuid::new_v4),
            name: manual_peer.address.clone(),
            addresses: addrs,
            port: manual_peer.port,
            version: "unknown".to_string(),
            platform: "unknown".to_string(),
            metadata: PeerMetadata {
                ssh_fingerprint: None,
                ssh_public_key: None,
                capabilities: vec![],
                device_name: Some(manual_peer.address.clone()),
            },
            last_seen: chrono::Utc::now().timestamp(),
        };

        // Add to peer manager
        self.peer_manager
            .add_peer(peer_info, DiscoveryMethod::Manual)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl Discovery for FallbackDiscovery {
    async fn start(&mut self) -> Result<()> {
        // Connect to manual peers
        self.connect_manual_peers().await?;

        // Start broadcast discovery
        if let Err(e) = self.start_broadcast().await {
            tracing::warn!("Failed to start broadcast discovery: {}", e);
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        // Stop broadcast task
        if let Some(handle) = self.broadcast_task.lock().await.take() {
            handle.abort();
        }

        // Close socket
        *self.broadcast_socket.lock().await = None;

        Ok(())
    }

    async fn discover_peers(&mut self) -> Result<Vec<PeerInfo>> {
        // Return peers discovered via fallback methods
        let mut peers = self
            .peer_manager
            .get_peers_by_method(DiscoveryMethod::Manual)
            .await?;

        let broadcast_peers = self
            .peer_manager
            .get_peers_by_method(DiscoveryMethod::Broadcast)
            .await?;

        peers.extend(broadcast_peers);
        Ok(peers)
    }

    async fn announce(&mut self, service_info: ServiceInfo) -> Result<()> {
        // Store service info for broadcast
        *self.our_info.write().await = Some(service_info);
        Ok(())
    }

    fn subscribe_changes(&mut self) -> mpsc::Receiver<DiscoveryEvent> {
        self.peer_manager.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcast_packet_roundtrip() {
        let service_info = ServiceInfo::default();
        let packet = FallbackDiscovery::create_broadcast_packet(&service_info).unwrap();

        let addr = "127.0.0.1:9090".parse().unwrap();
        let peer_info = FallbackDiscovery::parse_broadcast_packet(&packet, addr).unwrap();

        assert_eq!(peer_info.port, service_info.port);
    }

    #[tokio::test]
    async fn test_fallback_discovery_creation() {
        let config = Config::default();
        let peer_manager = PeerManager::new();
        let discovery = FallbackDiscovery::new(peer_manager, &config);
        assert!(discovery.is_ok());
    }
}

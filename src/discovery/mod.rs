//! Service discovery for finding and connecting to ClipSync instances

pub mod types;
pub mod mdns;
pub mod peers;
pub mod fallback;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use anyhow::Result;
use tokio::sync::mpsc::Receiver;

pub use types::{PeerInfo, ServiceInfo, DiscoveryEvent, DiscoveryMethod, PeerMetadata};
pub use mdns::MdnsDiscovery;
pub use peers::PeerManager;
pub use fallback::FallbackDiscovery;

/// Trait for service discovery implementations
#[async_trait]
pub trait Discovery: Send + Sync {
    /// Start the discovery service
    async fn start(&mut self) -> Result<()>;
    
    /// Stop the discovery service
    async fn stop(&mut self) -> Result<()>;
    
    /// Get currently discovered peers
    async fn discover_peers(&mut self) -> Result<Vec<PeerInfo>>;
    
    /// Announce our service
    async fn announce(&mut self, service_info: ServiceInfo) -> Result<()>;
    
    /// Subscribe to discovery events
    fn subscribe_changes(&mut self) -> Receiver<DiscoveryEvent>;
}

/// Combined discovery service using multiple methods
pub struct DiscoveryService {
    mdns: MdnsDiscovery,
    peer_manager: PeerManager,
    fallback: FallbackDiscovery,
    event_rx: Option<Receiver<DiscoveryEvent>>,
}

impl DiscoveryService {
    /// Create a new discovery service
    pub fn new(config: &crate::Config) -> Result<Self> {
        let peer_manager = PeerManager::new();
        let mdns = MdnsDiscovery::new(peer_manager.clone())?;
        let fallback = FallbackDiscovery::new(peer_manager.clone(), config)?;
        
        Ok(Self {
            mdns,
            peer_manager,
            fallback,
            event_rx: None,
        })
    }
    
    /// Get the peer manager
    pub fn peer_manager(&self) -> &PeerManager {
        &self.peer_manager
    }
}

#[async_trait]
impl Discovery for DiscoveryService {
    async fn start(&mut self) -> Result<()> {
        // Start mDNS discovery
        self.mdns.start().await?;
        
        // Start fallback discovery
        self.fallback.start().await?;
        
        // Subscribe to events from peer manager
        self.event_rx = Some(self.peer_manager.subscribe());
        
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        self.mdns.stop().await?;
        self.fallback.stop().await?;
        Ok(())
    }
    
    async fn discover_peers(&mut self) -> Result<Vec<PeerInfo>> {
        self.peer_manager.get_all_peers().await
    }
    
    async fn announce(&mut self, service_info: ServiceInfo) -> Result<()> {
        // Announce via mDNS
        self.mdns.announce(service_info.clone()).await?;
        
        // Also announce via fallback methods if configured
        self.fallback.announce(service_info).await?;
        
        Ok(())
    }
    
    fn subscribe_changes(&mut self) -> Receiver<DiscoveryEvent> {
        self.event_rx.take().unwrap_or_else(|| {
            self.peer_manager.subscribe()
        })
    }
}
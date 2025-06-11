//! Common types for service discovery

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use uuid::Uuid;

/// Information about a discovered peer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PeerInfo {
    /// Unique identifier for the peer
    pub id: Uuid,
    /// Human-readable name (hostname)
    pub name: String,
    /// Network addresses where peer can be reached
    pub addresses: Vec<SocketAddr>,
    /// Service port number
    pub port: u16,
    /// Service version
    pub version: String,
    /// Platform information (macos, linux, etc)
    pub platform: String,
    /// Additional service metadata
    pub metadata: PeerMetadata,
    /// Last time this peer was seen
    pub last_seen: i64,
}

/// Additional metadata about a peer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct PeerMetadata {
    /// SSH public key fingerprint for encryption
    pub ssh_fingerprint: Option<String>,
    /// Supported features/capabilities
    pub capabilities: Vec<String>,
    /// User-defined device name
    pub device_name: Option<String>,
}

/// Service information for announcement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Our peer ID
    pub id: Uuid,
    /// Service name to announce
    pub name: String,
    /// Port we're listening on
    pub port: u16,
    /// Service type for mDNS
    pub service_type: String,
    /// TXT record data
    pub txt_data: Vec<(String, String)>,
}

/// Discovery events
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// New peer discovered
    PeerDiscovered(PeerInfo),
    /// Peer information updated
    PeerUpdated(PeerInfo),
    /// Peer is no longer available
    PeerLost(Uuid),
    /// Discovery error occurred
    Error(String),
}

/// Discovery method used to find a peer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMethod {
    /// Found via mDNS/DNS-SD
    Mdns,
    /// Manually configured
    Manual,
    /// Local broadcast
    Broadcast,
    /// Cloud relay (future)
    CloudRelay,
}

impl Default for ServiceInfo {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: gethostname::gethostname().to_string_lossy().to_string(),
            port: 9090,
            service_type: "_clipsync._tcp".to_string(),
            txt_data: vec![
                ("version".to_string(), env!("CARGO_PKG_VERSION").to_string()),
                ("platform".to_string(), std::env::consts::OS.to_string()),
            ],
        }
    }
}

impl ServiceInfo {
    /// Create service info from configuration
    pub fn from_config(id: Uuid, port: u16) -> Self {
        let mut info = Self::default();
        info.id = id;
        info.port = port;
        info
    }
    
    /// Add SSH fingerprint to TXT data
    pub fn with_ssh_fingerprint(mut self, fingerprint: String) -> Self {
        self.txt_data.push(("ssh_fp".to_string(), fingerprint));
        self
    }
    
    /// Add capabilities to TXT data
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.txt_data.push(("caps".to_string(), capabilities.join(",")));
        self
    }
}

impl PeerInfo {
    /// Create PeerInfo from mDNS service data
    pub fn from_mdns(
        name: String,
        addresses: Vec<SocketAddr>,
        port: u16,
        txt_data: &[(String, String)],
    ) -> Self {
        let id = txt_data
            .iter()
            .find(|(k, _)| k == "id")
            .and_then(|(_, v)| Uuid::parse_str(v).ok())
            .unwrap_or_else(Uuid::new_v4);
            
        let version = txt_data
            .iter()
            .find(|(k, _)| k == "version")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "unknown".to_string());
            
        let platform = txt_data
            .iter()
            .find(|(k, _)| k == "platform")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "unknown".to_string());
            
        let ssh_fingerprint = txt_data
            .iter()
            .find(|(k, _)| k == "ssh_fp")
            .map(|(_, v)| v.clone());
            
        let capabilities = txt_data
            .iter()
            .find(|(k, _)| k == "caps")
            .map(|(_, v)| v.split(',').map(String::from).collect())
            .unwrap_or_default();
            
        let device_name = txt_data
            .iter()
            .find(|(k, _)| k == "device")
            .map(|(_, v)| v.clone());
            
        Self {
            id,
            name,
            addresses,
            port,
            version,
            platform,
            metadata: PeerMetadata {
                ssh_fingerprint,
                capabilities,
                device_name,
            },
            last_seen: chrono::Utc::now().timestamp(),
        }
    }
    
    /// Check if peer supports a specific capability
    pub fn has_capability(&self, capability: &str) -> bool {
        self.metadata.capabilities.iter().any(|c| c == capability)
    }
    
    /// Get the best address to connect to (prefer IPv4)
    pub fn best_address(&self) -> Option<SocketAddr> {
        // First try IPv4
        self.addresses
            .iter()
            .find(|addr| matches!(addr.ip(), IpAddr::V4(_)))
            .or_else(|| self.addresses.first())
            .copied()
    }
}
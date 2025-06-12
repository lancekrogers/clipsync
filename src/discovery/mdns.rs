//! mDNS/DNS-SD service discovery implementation

use crate::discovery::{
    types::DiscoveryMethod, Discovery, DiscoveryEvent, PeerInfo, PeerManager, ServiceInfo,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo as MdnsServiceInfo, TxtProperties};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

const SERVICE_TYPE: &str = "_clipsync._tcp.local.";
const BROWSE_TIMEOUT_MS: u64 = 5000;

/// mDNS-based service discovery
pub struct MdnsDiscovery {
    daemon: Arc<Mutex<Option<ServiceDaemon>>>,
    peer_manager: PeerManager,
    service_handle: Arc<Mutex<Option<String>>>,
    browse_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    our_id: Arc<RwLock<Option<Uuid>>>,
}

impl MdnsDiscovery {
    /// Create a new mDNS discovery instance
    pub fn new(peer_manager: PeerManager) -> Result<Self> {
        Ok(Self {
            daemon: Arc::new(Mutex::new(None)),
            peer_manager,
            service_handle: Arc::new(Mutex::new(None)),
            browse_handle: Arc::new(RwLock::new(None)),
            our_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Start browsing for services
    async fn start_browsing(&self) -> Result<()> {
        let daemon = self.daemon.lock().await;
        let daemon = daemon
            .as_ref()
            .ok_or_else(|| anyhow!("mDNS daemon not started"))?;

        let receiver = daemon.browse(SERVICE_TYPE)?;
        let peer_manager = self.peer_manager.clone();
        let our_id = self.our_id.clone();

        // Spawn browser task
        let handle = tokio::spawn(async move {
            Self::browse_loop(receiver, peer_manager, our_id).await;
        });

        let mut browse_handle = self.browse_handle.write().await;
        *browse_handle = Some(handle);

        Ok(())
    }

    /// Browse loop to handle discovered services
    async fn browse_loop(
        receiver: mdns_sd::Receiver<ServiceEvent>,
        peer_manager: PeerManager,
        our_id: Arc<RwLock<Option<Uuid>>>,
    ) {
        loop {
            match receiver.recv_timeout(std::time::Duration::from_millis(BROWSE_TIMEOUT_MS)) {
                Ok(event) => {
                    if let Err(e) = Self::handle_service_event(event, &peer_manager, &our_id).await
                    {
                        tracing::error!("Error handling mDNS event: {}", e);
                    }
                }
                Err(_) => {
                    // Timeout or other error, continue browsing
                    // The timeout is expected during normal operation
                }
            }
        }
    }

    /// Handle a service discovery event
    async fn handle_service_event(
        event: ServiceEvent,
        peer_manager: &PeerManager,
        our_id: &Arc<RwLock<Option<Uuid>>>,
    ) -> Result<()> {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                // Parse service info
                let peer_info = Self::parse_service_info(&info)?;

                // Skip our own service
                let our_id = our_id.read().await;
                if let Some(id) = our_id.as_ref() {
                    if peer_info.id == *id {
                        return Ok(());
                    }
                }

                // Add peer
                peer_manager
                    .add_peer(peer_info, DiscoveryMethod::Mdns)
                    .await?;
            }
            ServiceEvent::ServiceRemoved(_, full_name) => {
                // Extract peer ID from service name if possible
                if let Some(peer_id) = Self::extract_peer_id(&full_name) {
                    peer_manager.remove_peer(peer_id).await?;
                }
            }
            _ => {
                // Other events we don't need to handle
            }
        }

        Ok(())
    }

    /// Parse mDNS service info into PeerInfo
    fn parse_service_info(info: &MdnsServiceInfo) -> Result<PeerInfo> {
        // Extract TXT record data
        let txt_data = Self::parse_txt_records(info.get_properties());

        // Get addresses
        let addresses: Vec<SocketAddr> = info
            .get_addresses()
            .iter()
            .map(|addr| SocketAddr::new(*addr, info.get_port()))
            .collect();

        if addresses.is_empty() {
            return Err(anyhow!("No addresses found for service"));
        }

        // Extract service name (remove .local. suffix)
        let name = info
            .get_hostname()
            .trim_end_matches(".local.")
            .trim_end_matches('.')
            .to_string();

        Ok(PeerInfo::from_mdns(
            name,
            addresses,
            info.get_port(),
            &txt_data,
        ))
    }

    /// Parse TXT records into key-value pairs
    fn parse_txt_records(properties: &TxtProperties) -> Vec<(String, String)> {
        properties
            .iter()
            .map(|prop| {
                let key = prop.key().to_string();
                let value = if let Some(val) = prop.val() {
                    String::from_utf8_lossy(val).to_string()
                } else {
                    String::new()
                };
                (key, value)
            })
            .collect()
    }

    /// Extract peer ID from service name
    fn extract_peer_id(full_name: &str) -> Option<Uuid> {
        // Service name format: "ClipSync-{uuid}.{service_type}"
        let parts: Vec<&str> = full_name.split('.').collect();
        if let Some(name_part) = parts.first() {
            if let Some(uuid_str) = name_part.strip_prefix("ClipSync-") {
                return Uuid::parse_str(uuid_str).ok();
            }
        }
        None
    }

    /// Create mDNS service info from our ServiceInfo
    fn create_mdns_service_info(service_info: &ServiceInfo) -> Result<MdnsServiceInfo> {
        let service_name = format!("ClipSync-{}", service_info.id);
        let hostname = format!("{}.local.", service_info.name);

        // Convert txt_data to properties
        let mut properties = HashMap::new();
        properties.insert("id".to_string(), service_info.id.to_string());
        for (key, value) in &service_info.txt_data {
            properties.insert(key.clone(), value.clone());
        }

        // Get local IPs
        let addresses = Self::get_local_addresses()?;
        if addresses.is_empty() {
            return Err(anyhow!("No local IP addresses found"));
        }

        Ok(MdnsServiceInfo::new(
            SERVICE_TYPE,
            &service_name,
            &hostname,
            addresses[0],
            service_info.port,
            Some(properties),
        )?)
    }

    /// Get local IP addresses (excluding loopback)
    fn get_local_addresses() -> Result<Vec<IpAddr>> {
        let mut addresses = Vec::new();

        for iface in if_addrs::get_if_addrs()? {
            if !iface.is_loopback() {
                addresses.push(iface.ip());
            }
        }

        Ok(addresses)
    }
}

#[async_trait]
impl Discovery for MdnsDiscovery {
    async fn start(&mut self) -> Result<()> {
        // Create mDNS daemon
        let daemon = ServiceDaemon::new()?;

        *self.daemon.lock().await = Some(daemon);

        // Start browsing for services
        self.start_browsing().await?;

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        // Stop browsing
        if let Some(handle) = self.browse_handle.write().await.take() {
            handle.abort();
        }

        // Unregister our service
        if let Some(service_name) = self.service_handle.lock().await.take() {
            if let Some(daemon) = self.daemon.lock().await.as_ref() {
                daemon.unregister(&service_name)?;
            }
        }

        // Shutdown daemon
        if let Some(daemon) = self.daemon.lock().await.take() {
            daemon.shutdown()?;
        }

        Ok(())
    }

    async fn discover_peers(&mut self) -> Result<Vec<PeerInfo>> {
        // Return peers discovered via mDNS
        self.peer_manager
            .get_peers_by_method(DiscoveryMethod::Mdns)
            .await
    }

    async fn announce(&mut self, service_info: ServiceInfo) -> Result<()> {
        let daemon = self.daemon.lock().await;
        let daemon = daemon
            .as_ref()
            .ok_or_else(|| anyhow!("mDNS daemon not started"))?;

        // Store our ID
        *self.our_id.write().await = Some(service_info.id);

        // Create mDNS service info
        let mdns_info = Self::create_mdns_service_info(&service_info)?;
        let service_name = mdns_info.get_fullname().to_string();

        // Register service
        daemon.register(mdns_info)?;

        // Store handle for cleanup
        *self.service_handle.lock().await = Some(service_name);

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
    fn test_extract_peer_id() {
        let full_name = "ClipSync-550e8400-e29b-41d4-a716-446655440000._clipsync._tcp.local.";
        let peer_id = MdnsDiscovery::extract_peer_id(full_name);
        assert!(peer_id.is_some());
        assert_eq!(
            peer_id.unwrap().to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn test_extract_peer_id_valid() {
        let full_name = "ClipSync-550e8400-e29b-41d4-a716-446655440000._clipsync._tcp.local.";
        let peer_id = MdnsDiscovery::extract_peer_id(full_name);
        assert!(peer_id.is_some());
        assert_eq!(
            peer_id.unwrap().to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[tokio::test]
    async fn test_mdns_lifecycle() {
        let peer_manager = PeerManager::new();
        let mut discovery = MdnsDiscovery::new(peer_manager).unwrap();

        // Start should succeed
        assert!(discovery.start().await.is_ok());

        // Stop should succeed
        assert!(discovery.stop().await.is_ok());
    }
}

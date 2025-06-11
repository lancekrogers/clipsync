//! Unit tests for discovery components

#[cfg(test)]
mod discovery_tests {
    use crate::discovery::*;
    use crate::Config;
    use std::net::SocketAddr;
    
    #[tokio::test]
    async fn test_peer_manager_stats() {
        let manager = PeerManager::new();
        
        // Add different types of peers
        let peer1 = PeerInfo {
            id: uuid::Uuid::new_v4(),
            name: "mdns-peer".to_string(),
            addresses: vec!["192.168.1.1:9090".parse().unwrap()],
            port: 9090,
            version: "1.0.0".to_string(),
            platform: "linux".to_string(),
            metadata: PeerMetadata::default(),
            last_seen: chrono::Utc::now().timestamp(),
        };
        
        let peer2 = PeerInfo {
            id: uuid::Uuid::new_v4(),
            name: "manual-peer".to_string(),
            addresses: vec!["192.168.1.2:9090".parse().unwrap()],
            port: 9090,
            version: "1.0.0".to_string(),
            platform: "macos".to_string(),
            metadata: PeerMetadata::default(),
            last_seen: chrono::Utc::now().timestamp(),
        };
        
        manager.add_peer(peer1, DiscoveryMethod::Mdns).await.unwrap();
        manager.add_peer(peer2, DiscoveryMethod::Manual).await.unwrap();
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.total_peers, 2);
        assert_eq!(stats.mdns_peers, 1);
        assert_eq!(stats.manual_peers, 1);
        assert_eq!(stats.broadcast_peers, 0);
        assert_eq!(stats.failing_peers, 0);
    }
    
    #[tokio::test]
    async fn test_peer_best_address() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        
        let peer = PeerInfo {
            id: uuid::Uuid::new_v4(),
            name: "test".to_string(),
            addresses: vec![
                SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), 9090),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 9090),
                SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)), 9090),
            ],
            port: 9090,
            version: "1.0.0".to_string(),
            platform: "linux".to_string(),
            metadata: PeerMetadata::default(),
            last_seen: chrono::Utc::now().timestamp(),
        };
        
        // Should prefer IPv4
        let best = peer.best_address().unwrap();
        assert!(matches!(best.ip(), IpAddr::V4(_)));
        assert_eq!(best.ip(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)));
    }
    
    #[test]
    fn test_service_info_default() {
        let info = ServiceInfo::default();
        assert_eq!(info.service_type, "_clipsync._tcp");
        assert_eq!(info.port, 9090);
        assert!(!info.txt_data.is_empty());
        
        // Should have version and platform
        let txt_map: std::collections::HashMap<_, _> = info.txt_data
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
            
        assert!(txt_map.contains_key("version"));
        assert!(txt_map.contains_key("platform"));
    }
    
    #[tokio::test]
    async fn test_discovery_service_creation() {
        let config = Config::default();
        let discovery = DiscoveryService::new(&config);
        assert!(discovery.is_ok());
    }
    
    #[tokio::test]
    async fn test_peer_touch() {
        let manager = PeerManager::new();
        let peer_id = uuid::Uuid::new_v4();
        
        let peer = PeerInfo {
            id: peer_id,
            name: "test".to_string(),
            addresses: vec!["192.168.1.1:9090".parse().unwrap()],
            port: 9090,
            version: "1.0.0".to_string(),
            platform: "linux".to_string(),
            metadata: PeerMetadata::default(),
            last_seen: chrono::Utc::now().timestamp() - 1000, // Old timestamp
        };
        
        manager.add_peer(peer, DiscoveryMethod::Mdns).await.unwrap();
        
        // Touch the peer
        manager.touch_peer(peer_id).await.unwrap();
        
        // Check updated timestamp
        let updated = manager.get_peer(peer_id).await.unwrap();
        assert!(updated.last_seen > chrono::Utc::now().timestamp() - 10);
    }
}
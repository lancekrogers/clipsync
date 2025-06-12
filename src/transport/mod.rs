//! Network transport layer for secure clipboard synchronization
//! 
//! This module provides WebSocket-based transport with authentication,
//! streaming support, and automatic reconnection capabilities.

use async_trait::async_trait;
use std::net::SocketAddr;
use thiserror::Error;
use uuid::Uuid;

pub mod protocol;
pub mod websocket;
pub mod stream;
pub mod reconnect;

#[cfg(test)]
mod unit_tests;

// Re-export types from other modules for convenience
pub use crate::auth::{Authenticator, AuthToken};
pub use crate::discovery::PeerInfo;
pub use protocol::{Message, MessageType, MessagePayload, ClipboardData, ConnectionId};
pub use websocket::{WebSocketTransport, WebSocketConnection, WebSocketListener};
pub use stream::{StreamingTransport, StreamChunk, ProgressUpdate};
pub use reconnect::{ReconnectionManager, ReconnectionConfig};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

pub struct TransportManager {
    connections: Arc<RwLock<HashMap<Uuid, Box<dyn Connection>>>>,
    message_sender: broadcast::Sender<Message>,
    config: TransportConfig,
}

impl TransportManager {
    pub fn new(config: TransportConfig) -> Self {
        let (message_sender, _) = broadcast::channel(1000);
        
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            message_sender,
            config,
        }
    }
    
    pub async fn connect(&self, address: &str) -> Result<Box<dyn Connection>> {
        // This would be implemented with actual WebSocket connection logic
        todo!("Implement actual WebSocket connection")
    }
    
    pub async fn send_to_peer(&self, peer_id: Uuid, message: &Message) -> Result<()> {
        let connections = self.connections.read().await;
        if let Some(connection) = connections.get(&peer_id) {
            // This would send the message through the connection
            todo!("Implement message sending")
        } else {
            Err(TransportError::PeerNotFound { 
                peer_id, 
                peer_name: None  // Could be enhanced to look up peer name
            })
        }
    }
    
    pub async fn subscribe(&self) -> Result<broadcast::Receiver<Message>> {
        Ok(self.message_sender.subscribe())
    }
}

/// Transport layer errors with user-friendly messages
#[derive(Debug, Error)]
pub enum TransportError {
    /// WebSocket protocol error
    #[error("CS001: Network connection error: {message}. Please check your network connection and try again.")]
    WebSocket { message: String },
    
    /// Authentication error
    #[error("CS002: Authentication failed: {0}")]
    Authentication(#[from] crate::auth::AuthError),
    
    /// Connection error
    #[error("CS003: Connection failed: {message}. Check if the remote device is online and accessible.")]
    Connection { message: String },
    
    /// Message serialization/deserialization error
    #[error("CS004: Data format error: {0}. The message format may be corrupted or incompatible.")]
    Serialization(#[from] serde_json::Error),
    
    /// IO error
    #[error("CS005: System error: {0}. Check file permissions and available disk space.")]
    Io(#[from] std::io::Error),
    
    /// Streaming error
    #[error("CS006: File transfer error: {message}. Large clipboard content may not have transferred correctly.")]
    Streaming { message: String },
    
    /// Reconnection error
    #[error("CS007: Reconnection failed: {message}. Device may be offline or network may be unstable.")]
    Reconnection { message: String },
    
    /// Peer not found
    #[error("CS008: Cannot find device '{peer_name}' (ID: {peer_id}). Make sure the device is online and discoverable on your network.")]
    PeerNotFound { peer_id: Uuid, peer_name: Option<String> },
    
    /// Connection closed
    #[error("CS009: Connection closed unexpectedly. The remote device may have gone offline or network connectivity was lost.")]
    ConnectionClosed,
    
    /// Timeout error
    #[error("CS010: Operation timed out after waiting too long. Check your network connection and try again.")]
    Timeout,
    
    /// Protocol version mismatch
    #[error("CS011: Incompatible ClipSync versions. This device is running v{expected}, but the remote device is running v{actual}. Please update both devices to the same version.")]
    VersionMismatch { expected: String, actual: String },
    
    /// Configuration error
    #[error("CS012: Configuration error: {message}. Run 'clipsync config validate' to check your settings.")]
    Configuration { message: String },
    
    /// Permission denied
    #[error("CS013: Permission denied: {message}. Check file permissions and security settings.")]
    PermissionDenied { message: String },
    
    /// Network not available
    #[error("CS014: Network unavailable. Please check your network connection and ensure both devices are on the same network.")]
    NetworkUnavailable,
    
    /// Service unavailable
    #[error("CS015: ClipSync service is not running. Start the service with 'clipsync start'.")]
    ServiceUnavailable,
}

/// Result type for transport operations
pub type Result<T> = std::result::Result<T, TransportError>;

/// Main transport trait for peer-to-peer communication
#[async_trait]
pub trait Transport: Send + Sync {
    /// Connect to a remote peer
    async fn connect(
        peer: &PeerInfo, 
        authenticator: &dyn Authenticator
    ) -> Result<Box<dyn Connection>>;
    
    /// Start listening for incoming connections
    async fn listen(
        addr: SocketAddr, 
        authenticator: &dyn Authenticator
    ) -> Result<Box<dyn Listener>>;
    
    /// Send a message through the transport
    async fn send(&mut self, message: Message) -> Result<()>;
    
    /// Receive a message from the transport
    async fn receive(&mut self) -> Result<Message>;
    
    /// Get connection information
    fn connection_info(&self) -> ConnectionInfo;
    
    /// Check if connection is still active
    fn is_connected(&self) -> bool;
    
    /// Close the connection gracefully
    async fn close(&mut self) -> Result<()>;
}

/// Connection trait for active peer connections
#[async_trait]
pub trait Connection: Send + Sync {
    /// Send a message
    async fn send(&mut self, message: Message) -> Result<()>;
    
    /// Receive a message
    async fn receive(&mut self) -> Result<Message>;
    
    /// Get peer information
    fn peer_info(&self) -> &PeerInfo;
    
    /// Get connection metadata
    fn connection_info(&self) -> ConnectionInfo;
    
    /// Check if connection is active
    fn is_connected(&self) -> bool;
    
    /// Close the connection
    async fn close(&mut self) -> Result<()>;
}

/// Listener trait for accepting incoming connections
#[async_trait]
pub trait Listener: Send + Sync {
    /// Accept an incoming connection
    async fn accept(&mut self) -> Result<Box<dyn Connection>>;
    
    /// Get the listening address
    fn local_addr(&self) -> SocketAddr;
    
    /// Close the listener
    async fn close(&mut self) -> Result<()>;
}

/// Connection information and metadata
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// Unique connection identifier
    pub id: ConnectionId,
    
    /// Local address
    pub local_addr: SocketAddr,
    
    /// Remote address
    pub remote_addr: SocketAddr,
    
    /// Connection establishment time
    pub established_at: chrono::DateTime<chrono::Utc>,
    
    /// Total bytes sent
    pub bytes_sent: u64,
    
    /// Total bytes received
    pub bytes_received: u64,
    
    /// Connection state
    pub state: ConnectionState,
    
    /// Protocol version
    pub protocol_version: String,
}

/// Connection state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connection is being established
    Connecting,
    
    /// Connection is active and ready
    Connected,
    
    /// Connection is being authenticated
    Authenticating,
    
    /// Connection is authenticated and ready for data
    Ready,
    
    /// Connection is being closed
    Closing,
    
    /// Connection is closed
    Closed,
    
    /// Connection failed
    Failed,
}

/// Transport events for monitoring and management
#[derive(Debug, Clone)]
pub enum TransportEvent {
    /// New connection established
    ConnectionEstablished(ConnectionInfo),
    
    /// Connection was closed
    ConnectionClosed(ConnectionId),
    
    /// Connection failed
    ConnectionFailed(ConnectionId, String),
    
    /// Message sent successfully
    MessageSent(ConnectionId, MessageType),
    
    /// Message received
    MessageReceived(ConnectionId, MessageType),
    
    /// Authentication completed
    AuthenticationCompleted(ConnectionId),
    
    /// Streaming progress update
    StreamingProgress(ConnectionId, ProgressUpdate),
    
    /// Reconnection attempt
    ReconnectionAttempt(ConnectionId, u32),
}

/// Configuration for transport layer
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Maximum message size (default: 5MB)
    pub max_message_size: usize,
    
    /// Connection timeout (default: 30 seconds)
    pub connect_timeout: std::time::Duration,
    
    /// Keep-alive interval (default: 30 seconds)
    pub keepalive_interval: std::time::Duration,
    
    /// Whether to enable compression
    pub enable_compression: bool,
    
    /// Streaming chunk size for large payloads
    pub stream_chunk_size: usize,
    
    /// Maximum concurrent connections
    pub max_connections: usize,
    
    /// Reconnection configuration
    pub reconnection: ReconnectionConfig,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_message_size: crate::MAX_PAYLOAD_SIZE,
            connect_timeout: std::time::Duration::from_secs(30),
            keepalive_interval: std::time::Duration::from_secs(30),
            enable_compression: true,
            stream_chunk_size: 64 * 1024, // 64KB chunks
            max_connections: 10,
            reconnection: ReconnectionConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_transport_config_default() {
        let config = TransportConfig::default();
        assert_eq!(config.max_message_size, crate::MAX_PAYLOAD_SIZE);
        assert_eq!(config.connect_timeout, std::time::Duration::from_secs(30));
        assert!(config.enable_compression);
        assert_eq!(config.stream_chunk_size, 64 * 1024);
        assert_eq!(config.max_connections, 10);
    }
    
    #[test]
    fn test_connection_state_transitions() {
        let state = ConnectionState::Connecting;
        assert_ne!(state, ConnectionState::Connected);
        assert_ne!(state, ConnectionState::Ready);
    }
}
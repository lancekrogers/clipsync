#![deny(warnings, clippy::all)]
//! Network transport layer for secure clipboard synchronization
//!
//! This module provides WebSocket-based transport with authentication,
//! streaming support, and automatic reconnection capabilities.

use async_trait::async_trait;
use std::net::SocketAddr;
use thiserror::Error;
use uuid::Uuid;

pub mod protocol;
pub mod reconnect;
// Legacy large-payload API is outside the text-sync path.
#[allow(dead_code)]
pub mod stream;
pub mod tls;
pub mod websocket;

#[cfg(test)]
mod unit_tests;

// Re-export types from other modules for convenience
pub use crate::auth::{AuthToken, Authenticator};
pub use crate::discovery::PeerInfo;
pub use protocol::{ClipboardData, ConnectionId, Message, MessagePayload, MessageType};
pub use reconnect::{ReconnectionConfig, ReconnectionManager};
pub use stream::{ProgressUpdate, StreamChunk, StreamingTransport};
pub use websocket::{WebSocketConnection, WebSocketListener, WebSocketTransport};

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionDirection {
    Inbound,
    Outbound,
}

fn connection_direction_rank(local_id: Uuid, peer_id: Uuid, direction: ConnectionDirection) -> u8 {
    let lower_id = if local_id < peer_id {
        local_id
    } else {
        peer_id
    };
    let we_are_lower = local_id == lower_id;
    match (we_are_lower, direction) {
        (true, ConnectionDirection::Outbound) => 2,
        (true, ConnectionDirection::Inbound) => 1,
        (false, ConnectionDirection::Inbound) => 2,
        (false, ConnectionDirection::Outbound) => 1,
    }
}

struct ManagedConnection {
    session: Uuid,
    peer: PeerInfo,
    direction: ConnectionDirection,
    sender: tokio::sync::mpsc::Sender<Message>,
    task: tokio::task::JoinHandle<()>,
}

struct DialLease {
    in_flight: Arc<Mutex<HashSet<Uuid>>>,
    peer_id: Uuid,
    released: bool,
}

impl DialLease {
    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.peer_id);
    }
}

impl Drop for DialLease {
    fn drop(&mut self) {
        self.release();
    }
}
impl Drop for ManagedConnection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct TransportManager {
    connections: RwLock<HashMap<Uuid, ManagedConnection>>,
    dial_in_flight: Arc<Mutex<HashSet<Uuid>>>,
    message_sender: broadcast::Sender<Message>,
    config: TransportConfig,
    auth: Option<Arc<dyn Authenticator>>,
    node_id: Uuid,
}
impl TransportManager {
    pub fn new(config: TransportConfig) -> Self {
        let (message_sender, _) = broadcast::channel(64);
        Self {
            connections: RwLock::new(HashMap::new()),
            dial_in_flight: Arc::new(Mutex::new(HashSet::new())),
            message_sender,
            config,
            auth: None,
            node_id: Uuid::nil(),
        }
    }
    pub fn with_auth(config: TransportConfig, auth: Arc<dyn Authenticator>, node_id: Uuid) -> Self {
        let mut manager = Self::new(config);
        manager.auth = Some(auth);
        manager.node_id = node_id;
        manager
    }
    fn auth(&self) -> Result<Arc<dyn Authenticator>> {
        self.auth
            .clone()
            .ok_or_else(|| TransportError::Configuration {
                message: "Transport identity is not configured".into(),
            })
    }
    fn websocket_config(&self) -> websocket::WebSocketConfig {
        websocket::WebSocketConfig {
            max_message_size: self.config.max_message_size,
            connect_timeout: self.config.connect_timeout,
            keepalive_interval: self.config.keepalive_interval,
            max_connections: self.config.max_connections,
            ..Default::default()
        }
    }
    pub async fn listener(&self, address: SocketAddr) -> Result<WebSocketListener> {
        WebSocketTransport::new(address, self.auth()?, self.websocket_config(), self.node_id)
            .listen()
            .await
    }
    pub async fn serve(&self, mut listener: WebSocketListener) -> Result<()> {
        loop {
            match listener.accept().await {
                Ok(connection) => {
                    let id = connection.peer_info().id;
                    if let Err(e) = self
                        .register_peer_connection(id, connection, ConnectionDirection::Inbound)
                        .await
                    {
                        tracing::warn!("Incoming peer rejected: {e}");
                    }
                }
                Err(e) => tracing::debug!("Incoming handshake rejected: {e}"),
            }
        }
    }
    /// Connect only when discovery provides an identity derived from the peer's key.
    pub async fn connect_peer(&self, peer: &PeerInfo) -> Result<()> {
        if peer.id == self.node_id || self.is_connected(peer.id).await {
            return Ok(());
        }
        let mut lease = match self.try_acquire_dial_lease(peer.id) {
            Some(lease) => lease,
            None => return Ok(()),
        };
        let result = async {
            let connection = WebSocketTransport::connect_to_peer(
                peer,
                self.auth()?,
                self.websocket_config(),
                self.node_id,
            )
            .await?;
            self.register_peer_connection(
                peer.id,
                Box::new(connection),
                ConnectionDirection::Outbound,
            )
            .await
        }
        .await;
        lease.release();
        result
    }
    fn try_acquire_dial_lease(&self, peer_id: Uuid) -> Option<DialLease> {
        let mut in_flight = self
            .dial_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if in_flight.contains(&peer_id) {
            return None;
        }
        in_flight.insert(peer_id);
        Some(DialLease {
            in_flight: self.dial_in_flight.clone(),
            peer_id,
            released: false,
        })
    }
    pub async fn connect(&self, _address: &str) -> Result<Box<dyn Connection>> {
        Err(TransportError::Configuration {
            message: "A pinned peer identity is required; use connect_peer".into(),
        })
    }
    pub async fn is_connected(&self, id: Uuid) -> bool {
        self.connections
            .read()
            .await
            .get(&id)
            .is_some_and(|c| !c.task.is_finished() && !c.sender.is_closed())
    }
    pub async fn sessions(&self) -> Vec<(Uuid, Uuid)> {
        self.connections
            .read()
            .await
            .iter()
            .filter(|(_, c)| !c.task.is_finished() && !c.sender.is_closed())
            .map(|(id, c)| (*id, c.session))
            .collect()
    }
    pub async fn connected_peers(&self) -> Vec<PeerInfo> {
        self.connections
            .read()
            .await
            .values()
            .filter(|c| !c.task.is_finished() && !c.sender.is_closed())
            .map(|c| c.peer.clone())
            .collect()
    }
    pub async fn send_to_peer(&self, peer_id: Uuid, message: &Message) -> Result<()> {
        let sender = self
            .connections
            .read()
            .await
            .get(&peer_id)
            .map(|c| c.sender.clone())
            .ok_or(TransportError::PeerNotFound {
                peer_id,
                peer_name: None,
            })?;
        sender
            .try_send(message.clone())
            .map_err(|e| TransportError::Connection {
                message: e.to_string(),
            })
    }
    pub async fn subscribe(&self) -> Result<broadcast::Receiver<Message>> {
        Ok(self.message_sender.subscribe())
    }
    pub(crate) async fn register_peer_connection(
        &self,
        peer_id: Uuid,
        connection: Box<dyn Connection>,
        direction: ConnectionDirection,
    ) -> Result<()> {
        let incoming_rank = connection_direction_rank(self.node_id, peer_id, direction);
        let mut connections = self.connections.write().await;
        connections.retain(|_, c| !c.task.is_finished() && !c.sender.is_closed());
        if let Some(existing) = connections.get(&peer_id) {
            let existing_rank =
                connection_direction_rank(self.node_id, peer_id, existing.direction);
            if existing_rank >= incoming_rank {
                // Closing a socket may wait on I/O; release the registry first.
                drop(connections);
                let mut connection = connection;
                let _ = connection.close().await;
                return Ok(());
            }
        } else if connections.len() >= self.config.max_connections {
            return Err(TransportError::Connection {
                message: "Connection limit reached".into(),
            });
        }
        // Arbitration and insertion must share one lock so concurrent inbound/outbound
        // registrations cannot overwrite the preferred connection after the decision.
        let replacement = self.spawn_managed_connection(connection, direction);
        let old = connections.insert(peer_id, replacement);
        drop(connections);
        drop(old);
        Ok(())
    }

    fn spawn_managed_connection(
        &self,
        connection: Box<dyn Connection>,
        direction: ConnectionDirection,
    ) -> ManagedConnection {
        let peer = connection.peer_info().clone();
        let (sender, mut commands) = tokio::sync::mpsc::channel(16);
        let messages = self.message_sender.clone();
        let task = tokio::spawn(async move {
            let mut connection = connection;
            loop {
                tokio::select! {
                    message = commands.recv() => match message { Some(m) => if connection.send(m).await.is_err() { break; }, None => break },
                    message = connection.receive() => match message { Ok(m) => { let _ = messages.send(m); }, Err(_) => break },
                }
            }
            let _ = connection.close().await;
        });
        ManagedConnection {
            session: Uuid::new_v4(),
            peer,
            direction,
            sender,
            task,
        }
    }
    pub async fn shutdown(&self) {
        self.connections.write().await.clear();
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
    #[error(
        "CS003: Connection failed: {message}. Check if the remote device is online and accessible."
    )]
    Connection { message: String },

    /// Message serialization/deserialization error
    #[error("CS004: Data format error: {0}. The message format may be corrupted or incompatible.")]
    Serialization(#[from] serde_json::Error),

    /// IO error
    #[error("CS005: System I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Streaming error
    #[error("CS006: File transfer error: {message}. Large clipboard content may not have transferred correctly.")]
    Streaming { message: String },

    /// Reconnection error
    #[error(
        "CS007: Reconnection failed: {message}. Device may be offline or network may be unstable."
    )]
    Reconnection { message: String },

    /// Peer not found
    #[error("CS008: Cannot find device{} (ID: {peer_id}). Make sure the device is online and discoverable on your network.", peer_name.as_ref().map(|n| format!(" '{}'", n)).unwrap_or_default())]
    PeerNotFound {
        peer_id: Uuid,
        peer_name: Option<String>,
    },

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
        authenticator: &dyn Authenticator,
    ) -> Result<Box<dyn Connection>>;

    /// Start listening for incoming connections
    async fn listen(
        addr: SocketAddr,
        authenticator: &dyn Authenticator,
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

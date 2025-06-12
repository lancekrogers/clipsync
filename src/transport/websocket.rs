//! WebSocket transport implementation for ClipSync
//!
//! This module provides WebSocket-based transport with authentication,
//! TLS support, and message framing for secure peer communication.

use crate::transport::{
    protocol::*, Connection, Listener, Result, TransportError,
    ConnectionInfo, ConnectionState, PeerInfo,
};
use crate::auth::{Authenticator, PeerId};
use crate::progress::ConnectionProgress;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_tungstenite::{
    accept_async, connect_async, tungstenite::Message as WsMessage,
    WebSocketStream, MaybeTlsStream,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tracing::{debug, info, warn, error, instrument};
use uuid::Uuid;

/// WebSocket transport implementation
pub struct WebSocketTransport {
    /// Local bind address
    bind_addr: SocketAddr,
    
    /// Authenticator for peer authentication
    authenticator: Arc<dyn Authenticator>,
    
    /// Active connections
    connections: Arc<RwLock<HashMap<ConnectionId, Arc<Mutex<WebSocketConnection>>>>>,
    
    /// Connection sequence counter
    sequence_counter: Arc<AtomicU64>,
    
    /// Transport configuration
    config: WebSocketConfig,
}

/// WebSocket transport configuration
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// Maximum message size
    pub max_message_size: usize,
    
    /// Connection timeout
    pub connect_timeout: std::time::Duration,
    
    /// Keep-alive interval
    pub keepalive_interval: std::time::Duration,
    
    /// Enable compression
    pub enable_compression: bool,
    
    /// Maximum concurrent connections
    pub max_connections: usize,
    
    /// Buffer sizes
    pub send_buffer_size: usize,
    pub recv_buffer_size: usize,
    
    /// Enable TLS
    pub enable_tls: bool,
}

/// WebSocket connection wrapper
pub struct WebSocketConnection {
    /// Connection identifier
    id: ConnectionId,
    
    /// WebSocket stream (store as Option to allow moving into tasks)
    ws_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    
    /// Peer information
    peer_info: PeerInfo,
    
    /// Connection information
    connection_info: ConnectionInfo,
    
    /// Message sequence counter
    sequence_counter: AtomicU64,
    
    /// Connection state
    state: ConnectionState,
    
    /// Authenticated peer ID
    authenticated_peer: Option<PeerId>,
    
    /// Send channel for outgoing messages
    send_tx: mpsc::UnboundedSender<Message>,
    
    /// Receive channel for incoming messages
    recv_rx: Arc<Mutex<mpsc::UnboundedReceiver<Message>>>,
    
    /// Close notification
    close_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// WebSocket listener for accepting connections
pub struct WebSocketListener {
    /// TCP listener
    tcp_listener: TcpListener,
    
    /// Authenticator for incoming connections
    authenticator: Arc<dyn Authenticator>,
    
    /// Configuration
    config: WebSocketConfig,
    
    /// Connection counter
    sequence_counter: Arc<AtomicU64>,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            max_message_size: crate::MAX_PAYLOAD_SIZE,
            connect_timeout: std::time::Duration::from_secs(30),
            keepalive_interval: std::time::Duration::from_secs(30),
            enable_compression: true,
            max_connections: 100,
            send_buffer_size: 64 * 1024,
            recv_buffer_size: 64 * 1024,
            enable_tls: false, // TLS disabled for initial implementation
        }
    }
}

impl WebSocketTransport {
    /// Create a new WebSocket transport
    pub fn new(
        bind_addr: SocketAddr,
        authenticator: Arc<dyn Authenticator>,
        config: WebSocketConfig,
    ) -> Self {
        Self {
            bind_addr,
            authenticator,
            connections: Arc::new(RwLock::new(HashMap::new())),
            sequence_counter: Arc::new(AtomicU64::new(1)),
            config,
        }
    }
    
    /// Connect to a remote peer
    #[instrument(skip(authenticator))]
    pub async fn connect_to_peer(
        peer: &PeerInfo,
        authenticator: Arc<dyn Authenticator>,
        config: WebSocketConfig,
    ) -> Result<WebSocketConnection> {
        let mut progress = ConnectionProgress::new();
        progress.start_connecting(&peer.name);

        let addr = peer.best_address()
            .ok_or_else(|| {
                progress.error("No network address available");
                TransportError::Connection { 
                    message: format!("Device '{}' has no available network address. Check network discovery settings.", peer.name)
                }
            })?;
        
        info!("Connecting to peer {} at {}", peer.id, addr);
        
        // Create WebSocket URL
        let url = format!("ws://{}/clipsync", addr);
        
        // Connect with WebSocket using the high-level API
        let (ws_stream, _response) = tokio::time::timeout(
            config.connect_timeout,
            connect_async(&url)
        ).await
        .map_err(|_| {
            progress.error("Connection timed out");
            TransportError::Timeout
        })?
        .map_err(|e| {
            progress.error(&format!("WebSocket connection failed: {}", e));
            TransportError::WebSocket { 
                message: format!("Failed to establish WebSocket connection to {}: {}", addr, e) 
            }
        })?;
        
        // Create connection
        let connection_id = Uuid::new_v4();
        let mut connection = WebSocketConnection::new(
            connection_id,
            ws_stream,
            peer.clone(),
            addr,
            config,
        ).await?;
        
        // Perform handshake and authentication
        progress.start_handshake();
        connection.perform_handshake(&*authenticator).await.map_err(|e| {
            progress.error("Handshake failed");
            e
        })?;
        
        progress.start_authentication();
        connection.authenticate(&*authenticator).await.map_err(|e| {
            progress.error("Authentication failed");
            e
        })?;
        
        progress.finalizing_connection();
        
        info!("Successfully connected and authenticated with peer {}", peer.id);
        progress.success(&peer.name);
        
        Ok(connection)
    }
    
    /// Start listening for incoming connections
    #[instrument(skip(self))]
    pub async fn start_listener(&self) -> Result<WebSocketListener> {
        let tcp_listener = TcpListener::bind(&self.bind_addr).await
            .map_err(|e| TransportError::Io(e))?;
        
        info!("WebSocket listener started on {}", self.bind_addr);
        
        Ok(WebSocketListener {
            tcp_listener,
            authenticator: self.authenticator.clone(),
            config: self.config.clone(),
            sequence_counter: self.sequence_counter.clone(),
        })
    }
}

impl WebSocketConnection {
    /// Create a new WebSocket connection
    async fn new(
        id: ConnectionId,
        ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
        peer_info: PeerInfo,
        remote_addr: SocketAddr,
        _config: WebSocketConfig,
    ) -> Result<Self> {
        let local_addr = match ws_stream.get_ref() {
            MaybeTlsStream::Plain(tcp) => tcp.local_addr()
                .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap()),
            _ => "0.0.0.0:0".parse().unwrap(), // For TLS or other stream types
        };
        
        let connection_info = ConnectionInfo {
            id,
            local_addr,
            remote_addr,
            established_at: chrono::Utc::now(),
            bytes_sent: 0,
            bytes_received: 0,
            state: ConnectionState::Connecting,
            protocol_version: PROTOCOL_VERSION.to_string(),
        };
        
        // Create message channels
        let (send_tx, send_rx) = mpsc::unbounded_channel::<Message>();
        let (recv_tx, recv_rx) = mpsc::unbounded_channel::<Message>();
        
        let mut connection = Self {
            id,
            ws_stream: Some(ws_stream),
            peer_info,
            connection_info,
            sequence_counter: AtomicU64::new(1),
            state: ConnectionState::Connecting,
            authenticated_peer: None,
            send_tx,
            recv_rx: Arc::new(Mutex::new(recv_rx)),
            close_tx: None,
        };
        
        // Start message processing tasks
        connection.start_message_tasks(send_rx, recv_tx).await?;
        
        Ok(connection)
    }
    
    /// Start background tasks for message processing
    async fn start_message_tasks(
        &mut self,
        mut send_rx: mpsc::UnboundedReceiver<Message>,
        recv_tx: mpsc::UnboundedSender<Message>,
    ) -> Result<()> {
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        self.close_tx = Some(close_tx);
        
        // Take the WebSocket stream from the Option
        let ws_stream = self.ws_stream.take()
            .ok_or_else(|| TransportError::Connection { 
                message: "Internal error: network connection already in use. Please try reconnecting.".to_string() 
            })?;
        
        // Split the WebSocket stream for concurrent read/write
        let (mut ws_sink, mut ws_stream) = ws_stream.split();
        
        // Outbound message task
        let connection_id = self.id;
        tokio::spawn(async move {
            debug!("Starting outbound message task for connection {}", connection_id);
            
            let mut close_rx = close_rx;
            loop {
                tokio::select! {
                    // Process outbound messages
                    msg = send_rx.recv() => {
                        match msg {
                            Some(message) => {
                                let serialized = match serde_json::to_string(&message) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        error!("Failed to serialize message: {}", e);
                                        continue;
                                    }
                                };
                                
                                if let Err(e) = ws_sink.send(WsMessage::Text(serialized)).await {
                                    error!("Failed to send WebSocket message: {}", e);
                                    break;
                                }
                                
                                debug!("Sent {} message", message.message_type);
                            }
                            None => {
                                debug!("Send channel closed");
                                break;
                            }
                        }
                    }
                    
                    // Handle close signal
                    _ = &mut close_rx => {
                        debug!("Received close signal");
                        let _ = ws_sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                }
            }
            
            debug!("Outbound message task ended for connection {}", connection_id);
        });
        
        // Inbound message task
        let connection_id = self.id;
        tokio::spawn(async move {
            debug!("Starting inbound message task for connection {}", connection_id);
            
            while let Some(ws_msg) = ws_stream.next().await {
                match ws_msg {
                    Ok(WsMessage::Text(text)) => {
                        match serde_json::from_str::<Message>(&text) {
                            Ok(message) => {
                                debug!("Received {} message", message.message_type);
                                
                                if recv_tx.send(message).is_err() {
                                    debug!("Receive channel closed");
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to deserialize message: {}", e);
                            }
                        }
                    }
                    Ok(WsMessage::Binary(data)) => {
                        warn!("Received unexpected binary message of {} bytes", data.len());
                    }
                    Ok(WsMessage::Close(_)) => {
                        info!("WebSocket connection closed by peer");
                        break;
                    }
                    Ok(WsMessage::Ping(_)) => {
                        debug!("Received ping, pong handled automatically");
                        // Pong is handled automatically by tokio-tungstenite
                    }
                    Ok(WsMessage::Pong(_)) => {
                        debug!("Received pong");
                    }
                    Ok(WsMessage::Frame(_)) => {
                        // Raw frames are not expected in normal operation
                        warn!("Received unexpected raw frame");
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                }
            }
            
            debug!("Inbound message task ended for connection {}", connection_id);
        });
        
        info!("WebSocket connection {} message tasks started", self.id);
        Ok(())
    }
    
    /// Perform connection handshake
    async fn perform_handshake(&mut self, authenticator: &dyn Authenticator) -> Result<()> {
        self.state = ConnectionState::Connecting;
        
        // Get our public key for the handshake
        let _public_key = authenticator.get_public_key().await
            .map_err(|e| TransportError::Authentication(e))?;
        
        // Create handshake payload
        let handshake_payload = HandshakePayload {
            version: PROTOCOL_VERSION.to_string(),
            peer_id: Uuid::new_v4(), // This should be our peer ID
            capabilities: vec![
                "clipboard_sync".to_string(),
                "streaming".to_string(),
                "compression".to_string(),
            ],
            parameters: [
                ("max_message_size".to_string(), crate::MAX_PAYLOAD_SIZE.to_string()),
                ("protocol_version".to_string(), PROTOCOL_VERSION.to_string()),
            ].into_iter().collect(),
        };
        
        // Send handshake
        let handshake_msg = Message::new(
            MessageType::Handshake,
            MessagePayload::Handshake(handshake_payload),
        );
        
        self.send_tx.send(handshake_msg)
            .map_err(|_| TransportError::Connection { 
                message: "Failed to send connection setup message. The connection may be closed.".to_string() 
            })?;
        
        // Wait for handshake response
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.receive_message()
        ).await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|e| TransportError::Connection { 
            message: format!("Connection setup failed: {}. The remote device may be incompatible or offline.", e) 
        })?;
        
        // Verify handshake response
        match response.message_type {
            MessageType::HandshakeResponse => {
                if let MessagePayload::Handshake(payload) = response.payload {
                    if payload.version != PROTOCOL_VERSION {
                        return Err(TransportError::VersionMismatch {
                            expected: PROTOCOL_VERSION.to_string(),
                            actual: payload.version,
                        });
                    }
                    
                    info!("Handshake completed with peer {}", payload.peer_id);
                    self.state = ConnectionState::Connected;
                    Ok(())
                } else {
                    Err(TransportError::Connection { 
                        message: "Received invalid connection setup response. The remote device may be incompatible.".to_string() 
                    })
                }
            }
            MessageType::Error => {
                Err(TransportError::Connection { 
                    message: "Connection rejected by remote device. Check if this device is authorized.".to_string() 
                })
            }
            _ => {
                Err(TransportError::Connection { 
                    message: "Received unexpected response during connection setup. The remote device may be incompatible.".to_string() 
                })
            }
        }
    }
    
    /// Perform authentication
    async fn authenticate(&mut self, authenticator: &dyn Authenticator) -> Result<()> {
        self.state = ConnectionState::Authenticating;
        
        // Get our public key
        let public_key = authenticator.get_public_key().await
            .map_err(|e| TransportError::Authentication(e))?;
        
        // Create authentication payload
        let auth_payload = AuthPayload {
            method: "ssh_public_key".to_string(),
            data: BASE64.encode(public_key.to_openssh_format()),
            step: 1,
            result: None,
        };
        
        // Send authentication request
        let auth_msg = Message::new(
            MessageType::AuthChallenge,
            MessagePayload::Auth(auth_payload),
        );
        
        self.send_tx.send(auth_msg)
            .map_err(|_| TransportError::Connection { 
                message: "Failed to send authentication request. The connection may be closed.".to_string() 
            })?;
        
        // Wait for authentication result
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.receive_message()
        ).await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|e| TransportError::Connection { 
            message: format!("Authentication failed: {}. Check your SSH keys and permissions.", e) 
        })?;
        
        // Process authentication result
        match response.message_type {
            MessageType::AuthResult => {
                if let MessagePayload::Auth(payload) = response.payload {
                    match payload.result {
                        Some(AuthResult::Success { token: _, peer_id }) => {
                            info!("Authentication successful with peer {}", peer_id);
                            // Convert Uuid to PeerId - in real implementation this would be properly handled
                            self.authenticated_peer = Some(PeerId {
                                fingerprint: peer_id.to_string(),
                                name: None,
                            });
                            self.state = ConnectionState::Ready;
                            Ok(())
                        }
                        Some(AuthResult::Failed { reason }) => {
                            Err(TransportError::Authentication(
                                crate::auth::AuthError::AuthenticationFailed(reason)
                            ))
                        }
                        Some(AuthResult::Continue) => {
                            Err(TransportError::Authentication(
                                crate::auth::AuthError::AuthenticationFailed(
                                    "Multi-step auth not yet supported".to_string()
                                )
                            ))
                        }
                        None => {
                            Err(TransportError::Connection { 
                                message: "Authentication response missing result. The remote device may have an error.".to_string() 
                            })
                        }
                    }
                } else {
                    Err(TransportError::Connection { 
                        message: "Received invalid authentication response. The remote device may be incompatible.".to_string() 
                    })
                }
            }
            MessageType::Error => {
                Err(TransportError::Authentication(
                    crate::auth::AuthError::AuthenticationFailed("Authentication rejected by remote device. Verify your SSH key is authorized.".to_string())
                ))
            }
            _ => {
                Err(TransportError::Connection { 
                    message: "Received unexpected response during authentication. The remote device may be incompatible.".to_string() 
                })
            }
        }
    }
    
    /// Receive a message from the connection
    async fn receive_message(&mut self) -> Result<Message> {
        let mut recv_rx = self.recv_rx.lock().await;
        recv_rx.recv().await
            .ok_or_else(|| TransportError::ConnectionClosed)
    }
}

#[async_trait]
impl Connection for WebSocketConnection {
    async fn send(&mut self, mut message: Message) -> Result<()> {
        // Set sequence number
        message.sequence = self.sequence_counter.fetch_add(1, Ordering::SeqCst);
        
        self.send_tx.send(message)
            .map_err(|_| TransportError::ConnectionClosed)
    }
    
    async fn receive(&mut self) -> Result<Message> {
        self.receive_message().await
    }
    
    fn peer_info(&self) -> &PeerInfo {
        &self.peer_info
    }
    
    fn connection_info(&self) -> ConnectionInfo {
        self.connection_info.clone()
    }
    
    fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Ready | ConnectionState::Connected)
    }
    
    async fn close(&mut self) -> Result<()> {
        info!("Closing connection {}", self.id);
        
        // Send close notification
        if let Some(close_tx) = self.close_tx.take() {
            let _ = close_tx.send(());
        }
        
        self.state = ConnectionState::Closed;
        Ok(())
    }
}

#[async_trait]
impl Listener for WebSocketListener {
    async fn accept(&mut self) -> Result<Box<dyn Connection>> {
        // Accept TCP connection
        let (tcp_stream, addr) = self.tcp_listener.accept().await
            .map_err(|e| TransportError::Io(e))?;
        
        info!("Accepting WebSocket connection from {}", addr);
        
        // Wrap TcpStream in MaybeTlsStream and upgrade to WebSocket
        let maybe_tls_stream = MaybeTlsStream::Plain(tcp_stream);
        let ws_stream = accept_async(maybe_tls_stream).await
            .map_err(|e| TransportError::WebSocket { 
                message: format!("Failed to accept WebSocket connection from {}: {}", addr, e) 
            })?;
        
        // Create peer info (will be updated during handshake)
        let peer_info = PeerInfo {
            id: Uuid::new_v4(), // Temporary, will be updated
            name: addr.to_string(),
            addresses: vec![addr],
            port: addr.port(),
            version: "unknown".to_string(),
            platform: "unknown".to_string(),
            metadata: Default::default(),
            last_seen: chrono::Utc::now().timestamp(),
        };
        
        // Create connection
        let connection_id = self.sequence_counter.fetch_add(1, Ordering::SeqCst);
        let connection_id = Uuid::from_u128(connection_id as u128);
        
        let mut connection = WebSocketConnection::new(
            connection_id,
            ws_stream,
            peer_info,
            addr,
            self.config.clone(),
        ).await?;
        
        // Handle incoming handshake and authentication
        connection.handle_incoming_handshake(&*self.authenticator).await?;
        connection.handle_incoming_authentication(&*self.authenticator).await?;
        
        info!("Successfully accepted and authenticated connection from {}", addr);
        
        Ok(Box::new(connection))
    }
    
    fn local_addr(&self) -> SocketAddr {
        self.tcp_listener.local_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
    }
    
    async fn close(&mut self) -> Result<()> {
        // Closing the listener is handled when it's dropped
        Ok(())
    }
}

impl WebSocketConnection {
    /// Handle incoming handshake from a client
    async fn handle_incoming_handshake(&mut self, _authenticator: &dyn Authenticator) -> Result<()> {
        // Wait for handshake
        let handshake = self.receive_message().await?;
        
        if handshake.message_type != MessageType::Handshake {
            return Err(TransportError::Connection { 
                message: "Expected connection setup message but received something else. The client may be incompatible.".to_string() 
            });
        }
        
        // Process handshake
        if let MessagePayload::Handshake(payload) = handshake.payload {
            if payload.version != PROTOCOL_VERSION {
                // Send error response
                let error_payload = ErrorPayload {
                    code: ErrorCode::ProtocolError,
                    message: format!("Protocol version mismatch: expected {}, got {}", 
                                   PROTOCOL_VERSION, payload.version),
                    details: None,
                };
                
                let error_msg = Message::new(
                    MessageType::Error,
                    MessagePayload::Error(error_payload),
                );
                
                let _ = self.send_tx.send(error_msg);
                
                return Err(TransportError::VersionMismatch {
                    expected: PROTOCOL_VERSION.to_string(),
                    actual: payload.version,
                });
            }
            
            // Send handshake response
            let response_payload = HandshakePayload {
                version: PROTOCOL_VERSION.to_string(),
                peer_id: Uuid::new_v4(), // Our peer ID
                capabilities: vec![
                    "clipboard_sync".to_string(),
                    "streaming".to_string(),
                    "compression".to_string(),
                ],
                parameters: [
                    ("max_message_size".to_string(), crate::MAX_PAYLOAD_SIZE.to_string()),
                    ("protocol_version".to_string(), PROTOCOL_VERSION.to_string()),
                ].into_iter().collect(),
            };
            
            let response_msg = Message::new(
                MessageType::HandshakeResponse,
                MessagePayload::Handshake(response_payload),
            );
            
            self.send_tx.send(response_msg)
                .map_err(|_| TransportError::Connection { 
                    message: "Failed to send connection setup response. The connection may be closed.".to_string() 
                })?;
            
            self.state = ConnectionState::Connected;
            info!("Handshake completed with client");
            
            Ok(())
        } else {
            Err(TransportError::Connection { 
                message: "Received invalid connection setup data. The client may be incompatible.".to_string() 
            })
        }
    }
    
    /// Handle incoming authentication from a client
    async fn handle_incoming_authentication(&mut self, authenticator: &dyn Authenticator) -> Result<()> {
        // Wait for authentication request
        let auth_msg = self.receive_message().await?;
        
        if auth_msg.message_type != MessageType::AuthChallenge {
            return Err(TransportError::Connection { 
                message: "Expected authentication request but received something else. The client may be incompatible.".to_string() 
            });
        }
        
        // Process authentication
        if let MessagePayload::Auth(payload) = auth_msg.payload {
            // Decode public key
            let key_data = BASE64.decode(&payload.data)
                .map_err(|_| TransportError::Authentication(
                    crate::auth::AuthError::InvalidKeyFormat("Invalid base64".to_string())
                ))?;
            
            // Parse public key (simplified - would use proper SSH key parsing)
            let public_key = crate::auth::PublicKey::from_openssh_format(&key_data)
                .map_err(|e| TransportError::Authentication(e))?;
            
            // Authenticate the peer
            match authenticator.authenticate_peer(&public_key).await {
                Ok(auth_token) => {
                    // Verify token to get peer ID
                    match authenticator.verify_token(&auth_token).await {
                        Ok(peer_id) => {
                            // Send success response
                            let auth_response = AuthPayload {
                                method: "ssh_public_key".to_string(),
                                data: "".to_string(),
                                step: 2,
                                result: Some(AuthResult::Success {
                                    token: auth_token.to_string(),
                                    peer_id: peer_id.clone(),
                                }),
                            };
                            
                            let response_msg = Message::new(
                                MessageType::AuthResult,
                                MessagePayload::Auth(auth_response),
                            );
                            
                            self.send_tx.send(response_msg)
                                .map_err(|_| TransportError::Connection { 
                                    message: "Failed to send authentication result. The connection may be closed.".to_string() 
                                })?;
                            
                            self.authenticated_peer = Some(peer_id.clone());
                            self.state = ConnectionState::Ready;
                            
                            info!("Authentication successful for peer {}", peer_id);
                            Ok(())
                        }
                        Err(e) => {
                            self.send_auth_error("Token verification failed").await?;
                            Err(TransportError::Authentication(e))
                        }
                    }
                }
                Err(e) => {
                    self.send_auth_error("Authentication failed").await?;
                    Err(TransportError::Authentication(e))
                }
            }
        } else {
            Err(TransportError::Connection { 
                message: "Received invalid authentication data. The client may be incompatible.".to_string() 
            })
        }
    }
    
    /// Send authentication error
    async fn send_auth_error(&mut self, reason: &str) -> Result<()> {
        let auth_response = AuthPayload {
            method: "ssh_public_key".to_string(),
            data: "".to_string(),
            step: 2,
            result: Some(AuthResult::Failed {
                reason: reason.to_string(),
            }),
        };
        
        let response_msg = Message::new(
            MessageType::AuthResult,
            MessagePayload::Auth(auth_response),
        );
        
        self.send_tx.send(response_msg)
            .map_err(|_| TransportError::Connection { 
                message: "Failed to send authentication error response. The connection may be closed.".to_string() 
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_websocket_config_default() {
        let config = WebSocketConfig::default();
        assert_eq!(config.max_message_size, crate::MAX_PAYLOAD_SIZE);
        assert_eq!(config.connect_timeout, std::time::Duration::from_secs(30));
        assert!(config.enable_compression);
        assert!(!config.enable_tls); // TLS disabled for now
    }
    
    #[test]
    fn test_connection_id_generation() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        assert_ne!(id1, id2);
    }
    
    #[test]
    fn test_connection_state_transitions() {
        let mut state = ConnectionState::Connecting;
        assert_eq!(state, ConnectionState::Connecting);
        
        state = ConnectionState::Connected;
        assert_eq!(state, ConnectionState::Connected);
        
        state = ConnectionState::Ready;
        assert_eq!(state, ConnectionState::Ready);
    }
}
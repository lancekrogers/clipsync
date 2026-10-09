#![deny(warnings, clippy::all)]
//! Bounded WebSocket framing over mutually authenticated TLS 1.3.
use super::{
    protocol::*, tls, Connection, ConnectionInfo, ConnectionState, Listener, PeerInfo, Result,
    TransportError,
};
use crate::auth::{Authenticator, PublicKey};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};
use tokio_tungstenite::{
    tungstenite::{protocol::WebSocketConfig as FrameConfig, Message as Frame},
    WebSocketStream,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    pub max_message_size: usize,
    pub connect_timeout: Duration,
    pub keepalive_interval: Duration,
    pub enable_compression: bool,
    pub max_connections: usize,
    pub send_buffer_size: usize,
    pub recv_buffer_size: usize,
    pub enable_tls: bool,
}
impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            max_message_size: crate::MAX_PAYLOAD_SIZE,
            connect_timeout: Duration::from_secs(10),
            keepalive_interval: Duration::from_secs(15),
            enable_compression: false,
            max_connections: 10,
            send_buffer_size: 16,
            recv_buffer_size: 16,
            enable_tls: true,
        }
    }
}
pub struct WebSocketTransport {
    bind_addr: SocketAddr,
    authenticator: Arc<dyn Authenticator>,
    config: WebSocketConfig,
}
pub struct WebSocketListener {
    tcp: TcpListener,
    authenticator: Arc<dyn Authenticator>,
    config: WebSocketConfig,
}
pub struct WebSocketConnection {
    peer: PeerInfo,
    info: ConnectionInfo,
    send: mpsc::Sender<Message>,
    receive: mpsc::Receiver<Message>,
    task: JoinHandle<()>,
    max_size: usize,
}
fn failure(e: impl std::fmt::Display) -> TransportError {
    TransportError::Connection {
        message: e.to_string(),
    }
}
fn endpoint_failure(addr: SocketAddr, e: impl std::fmt::Display) -> TransportError {
    TransportError::Connection {
        message: format!("Could not connect to {addr}: {e}"),
    }
}
fn endpoint_io_failure(addr: SocketAddr, e: std::io::Error) -> TransportError {
    TransportError::Connection {
        message: format!("Could not connect to {addr}: {e} ({})", e.kind()),
    }
}
fn frame_config(config: &WebSocketConfig) -> FrameConfig {
    // JSON byte arrays can occupy four bytes per payload byte, plus metadata.
    let max = config
        .max_message_size
        .saturating_mul(4)
        .saturating_add(65536);
    FrameConfig::default()
        .max_message_size(Some(max))
        .max_frame_size(Some(max))
}
fn validate(message: &Message, max: usize) -> Result<()> {
    if message.version != PROTOCOL_VERSION {
        return Err(failure("Incompatible protocol version"));
    }
    if let MessagePayload::Clipboard(data) = &message.payload {
        if data.data.len() > max || data.compression.is_some() {
            return Err(failure("Invalid or oversized clipboard payload"));
        }
    }
    Ok(())
}
impl WebSocketTransport {
    pub fn new(
        bind_addr: SocketAddr,
        authenticator: Arc<dyn Authenticator>,
        config: WebSocketConfig,
        _node_id: Uuid,
    ) -> Self {
        Self {
            bind_addr,
            authenticator,
            config,
        }
    }
    pub async fn listen(&self) -> Result<WebSocketListener> {
        if !self.config.enable_tls {
            return Err(failure("Plaintext transport is disabled"));
        }
        // Validate identity before advertising readiness.
        tls::configs(self.authenticator.as_ref())
            .await
            .map_err(failure)?;
        Ok(WebSocketListener {
            tcp: TcpListener::bind(self.bind_addr).await?,
            authenticator: self.authenticator.clone(),
            config: self.config.clone(),
        })
    }
    pub async fn connect_to_peer(
        peer: &PeerInfo,
        auth: Arc<dyn Authenticator>,
        config: WebSocketConfig,
        _node_id: Uuid,
    ) -> Result<WebSocketConnection> {
        if !config.enable_tls {
            return Err(failure("Plaintext transport is disabled"));
        }
        let candidates = peer.connect_candidates();
        if candidates.is_empty() {
            return Err(failure("Peer has no reachable address"));
        }
        let deadline = tokio::time::Instant::now() + config.connect_timeout;
        let (client, _) = tokio::time::timeout_at(deadline, tls::configs(auth.as_ref()))
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(failure)?;
        let connector = TlsConnector::from(client);
        let mut errors = Vec::new();
        let count = candidates.len();
        for (index, addr) in candidates.into_iter().enumerate() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            // Reserve time for every remaining endpoint, including when the caller
            // supplies a short total timeout. The last candidate gets the balance.
            let slice = remaining / (count - index) as u32;
            match timeout(
                slice,
                Self::connect_endpoint(peer, &connector, addr, &config, auth.clone()),
            )
            .await
            {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(e)) => errors.push(format!("{addr}: {e}")),
                Err(_) => errors.push(format!("{addr}: timed out after {slice:?}")),
            }
        }
        if errors.is_empty() {
            Err(TransportError::Timeout)
        } else {
            Err(failure(format!(
                "All connection attempts failed ({})",
                errors.join("; ")
            )))
        }
    }

    async fn connect_endpoint(
        peer: &PeerInfo,
        connector: &TlsConnector,
        addr: SocketAddr,
        config: &WebSocketConfig,
        auth: Arc<dyn Authenticator>,
    ) -> Result<WebSocketConnection> {
        let tcp = TcpStream::connect(addr)
            .await
            .map_err(|e| endpoint_io_failure(addr, e))?;
        let local = tcp.local_addr().map_err(|e| endpoint_io_failure(addr, e))?;
        let stream = connector
            .connect("clipsync.local".try_into().map_err(failure)?, tcp)
            .await
            .map_err(|e| endpoint_failure(addr, e))?;
        let stream = TlsStream::Client(stream);
        let key = session_key(&stream)?;
        if tls::node_id(&key) != peer.id {
            return Err(failure(
                "Discovered identity does not match authenticated peer",
            ));
        }
        let (ws, _) = tokio_tungstenite::client_async_with_config(
            format!("wss://{addr}/clipsync"),
            stream,
            Some(frame_config(config)),
        )
        .await
        .map_err(|e| endpoint_failure(addr, e))?;
        Ok(WebSocketConnection::new(
            ws,
            key,
            addr,
            local,
            config.clone(),
            auth,
        ))
    }
}
fn session_key(stream: &TlsStream<TcpStream>) -> Result<PublicKey> {
    let (_, session) = stream.get_ref();
    if session.alpn_protocol() != Some(b"clipsync/2".as_slice()) {
        return Err(failure("Missing ClipSync protocol negotiation"));
    }
    let cert = session
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or_else(|| failure("Missing peer identity"))?;
    tls::public_key(cert.as_ref()).map_err(failure)
}
#[async_trait]
impl Listener for WebSocketListener {
    async fn accept(&mut self) -> Result<Box<dyn Connection>> {
        let (tcp, addr) = self.tcp.accept().await?;
        let local = tcp.local_addr()?;
        timeout(self.config.connect_timeout, async {
            let (_, server) = tls::configs(self.authenticator.as_ref())
                .await
                .map_err(failure)?;
            let stream = TlsStream::Server(TlsAcceptor::from(server).accept(tcp).await?);
            let key = session_key(&stream)?;
            let ws = tokio_tungstenite::accept_async_with_config(
                stream,
                Some(frame_config(&self.config)),
            )
            .await
            .map_err(failure)?;
            Ok(Box::new(WebSocketConnection::new(
                ws,
                key,
                addr,
                local,
                self.config.clone(),
                self.authenticator.clone(),
            )) as Box<dyn Connection>)
        })
        .await
        .map_err(|_| TransportError::Timeout)?
    }
    fn local_addr(&self) -> SocketAddr {
        self.tcp.local_addr().expect("bound listener")
    }
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}
impl WebSocketConnection {
    fn new(
        ws: WebSocketStream<TlsStream<TcpStream>>,
        key: PublicKey,
        remote: SocketAddr,
        local: SocketAddr,
        config: WebSocketConfig,
        auth: Arc<dyn Authenticator>,
    ) -> Self {
        let peer_id = tls::node_id(&key);
        let peer = PeerInfo {
            id: peer_id,
            name: key.fingerprint(),
            addresses: vec![remote],
            port: remote.port(),
            version: PROTOCOL_VERSION.into(),
            platform: "unknown".into(),
            metadata: Default::default(),
            last_seen: chrono::Utc::now().timestamp(),
        };
        let info = ConnectionInfo {
            id: Uuid::new_v4(),
            local_addr: local,
            remote_addr: remote,
            established_at: chrono::Utc::now(),
            bytes_sent: 0,
            bytes_received: 0,
            state: ConnectionState::Ready,
            protocol_version: PROTOCOL_VERSION.into(),
        };
        let (send, mut outbound) = mpsc::channel::<Message>(16);
        let (inbound, receive) = mpsc::channel(16);
        let max_size = config.max_message_size;
        let task = tokio::spawn(async move {
            let (mut writer, mut reader) = ws.split();
            let mut heartbeat = tokio::time::interval(config.keepalive_interval);
            let mut last_received = tokio::time::Instant::now();
            loop {
                let result: Result<()> = async {
                    tokio::select! {
                        item = outbound.recv() => {
                            let Some(message) = item else { return Err(TransportError::ConnectionClosed); };
                            if !auth.is_authorized(&key).await? { return Err(failure("Peer authorization revoked")); }
                            validate(&message, max_size)?;
                            timeout(config.connect_timeout, writer.send(Frame::Text(serde_json::to_string(&message)?.into()))).await.map_err(|_| TransportError::Timeout)?.map_err(failure)?;
                        }
                        item = reader.next() => {
                            last_received = tokio::time::Instant::now();
                            match item {
                                Some(Ok(Frame::Text(text))) => {
                                    if !auth.is_authorized(&key).await? { return Err(failure("Peer authorization revoked")); }
                                    let mut message: Message = serde_json::from_str(&text)?;
                                    validate(&message, max_size)?;
                                    message.source_peer_id = Some(peer_id);
                                    inbound.try_send(message).map_err(failure)?;
                                }
                                Some(Ok(Frame::Ping(data))) => { writer.send(Frame::Pong(data)).await.map_err(failure)?; }
                                Some(Ok(Frame::Pong(_))) => {}
                                _ => return Err(TransportError::ConnectionClosed),
                            }
                        }
                        _ = heartbeat.tick() => {
                            if !auth.is_authorized(&key).await? { return Err(failure("Peer authorization revoked")); }
                            if last_received.elapsed() > config.keepalive_interval * 3 { return Err(TransportError::Timeout); }
                            timeout(config.connect_timeout, writer.send(Frame::Ping(Vec::new().into()))).await.map_err(|_| TransportError::Timeout)?.map_err(failure)?;
                        }
                    }
                    Ok(())
                }.await;
                if let Err(e) = result {
                    tracing::debug!("Connection {peer_id} closed: {e}");
                    break;
                }
            }
        });
        Self {
            peer,
            info,
            send,
            receive,
            task,
            max_size,
        }
    }
}
impl Drop for WebSocketConnection {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[async_trait]
impl Connection for WebSocketConnection {
    async fn send(&mut self, message: Message) -> Result<()> {
        validate(&message, self.max_size)?;
        self.send.try_send(message).map_err(failure)
    }
    async fn receive(&mut self) -> Result<Message> {
        self.receive
            .recv()
            .await
            .ok_or(TransportError::ConnectionClosed)
    }
    fn peer_info(&self) -> &PeerInfo {
        &self.peer
    }
    fn connection_info(&self) -> ConnectionInfo {
        let mut info = self.info.clone();
        if self.task.is_finished() {
            info.state = ConnectionState::Closed;
        }
        info
    }
    fn is_connected(&self) -> bool {
        !self.task.is_finished()
    }
    async fn close(&mut self) -> Result<()> {
        self.task.abort();
        Ok(())
    }
}

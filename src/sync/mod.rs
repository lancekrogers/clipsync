pub mod trust_sync;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, RwLock};
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub use trust_sync::{setup_trust_sync, TrustAwareSyncEngine};

use crate::adapters::{
    ClipboardData, ClipboardEntry, ClipboardProviderWrapper, HistoryManager, Peer, PeerDiscovery,
};
use crate::config::Config;
use crate::transport::protocol::ClipboardFormat;
use crate::transport::{
    ClipboardData as TransportClipboardData, Message, MessagePayload, MessageType, TransportManager,
};

#[derive(Debug, Clone)]
pub struct SyncEvent {
    pub timestamp: DateTime<Utc>,
    pub source_peer: Uuid,
    pub entry: ClipboardEntry,
}

pub struct SyncEngine {
    config: Arc<Config>,
    clipboard: Arc<ClipboardProviderWrapper>,
    history: Arc<HistoryManager>,
    discovery: Arc<PeerDiscovery>,
    transport: Arc<TransportManager>,
    peers: Arc<RwLock<HashMap<Uuid, Peer>>>,
    event_sender: broadcast::Sender<SyncEvent>,
    last_local_update: Arc<RwLock<SystemTime>>,
    sync_interval: Duration,
}

impl SyncEngine {
    pub fn new(
        config: Arc<Config>,
        clipboard: Arc<ClipboardProviderWrapper>,
        history: Arc<HistoryManager>,
        discovery: Arc<PeerDiscovery>,
        transport: Arc<TransportManager>,
    ) -> Self {
        let (event_sender, _) = broadcast::channel(100);

        Self {
            config: Arc::clone(&config),
            clipboard,
            history,
            discovery,
            transport,
            peers: Arc::new(RwLock::new(HashMap::new())),
            event_sender,
            last_local_update: Arc::new(RwLock::new(UNIX_EPOCH)),
            sync_interval: Duration::from_millis(config.sync_interval_ms()),
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting sync engine");

        let discovery_task = self.start_discovery();
        let clipboard_monitor_task = self.start_clipboard_monitor();
        let sync_task = self.start_sync_loop();
        let transport_handler_task = self.start_transport_handler();

        tokio::try_join!(
            discovery_task,
            clipboard_monitor_task,
            sync_task,
            transport_handler_task
        )?;

        Ok(())
    }

    async fn start_discovery(&self) -> Result<()> {
        let discovery = Arc::clone(&self.discovery);
        let peers = Arc::clone(&self.peers);
        let transport = Arc::clone(&self.transport);

        discovery.start().await?;

        let mut peer_updates = discovery.subscribe().await?;

        loop {
            match peer_updates.recv().await {
                Ok(peer) => {
                    info!("Discovered peer: {} at {}", peer.id, peer.address);

                    if let Err(e) = self.connect_to_peer(&peer).await {
                        warn!("Failed to connect to peer {}: {}", peer.id, e);
                        continue;
                    }

                    peers.write().await.insert(peer.id, peer);
                }
                Err(e) => {
                    error!("Error receiving peer update: {}", e);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn connect_to_peer(&self, peer: &Peer) -> Result<()> {
        // Store the connection with proper authentication
        let connection = self.transport.connect(&peer.address).await?;

        // The transport layer now handles authentication automatically
        // through the WebSocket handshake and SSH key verification
        info!(
            "Connected and authenticated with peer {} at {}",
            peer.id, peer.address
        );

        // Store the authenticated connection for message routing
        self.transport
            .register_peer_connection(peer.id, connection)
            .await?;

        Ok(())
    }

    async fn start_clipboard_monitor(&self) -> Result<()> {
        let clipboard = Arc::clone(&self.clipboard);
        let history = Arc::clone(&self.history);
        let event_sender = self.event_sender.clone();
        let last_update = Arc::clone(&self.last_local_update);
        let config = Arc::clone(&self.config);

        // Use a reasonable interval of 1 second instead of 200ms
        // This prevents excessive CPU usage and potential interference with password managers
        let mut interval = interval(Duration::from_secs(1));
        let mut last_content_hash = None;

        loop {
            interval.tick().await;

            match clipboard.get_text().await {
                Ok(content) => {
                    // Safety check: Skip potentially sensitive content
                    if crate::clipboard::safety::is_potentially_sensitive(&content) {
                        debug!("Skipping potentially sensitive clipboard content");
                        continue;
                    }
                    
                    // Safety check: Skip if in sensitive context
                    if crate::clipboard::safety::is_sensitive_context() {
                        debug!("Skipping clipboard sync in sensitive context");
                        continue;
                    }
                    
                    let content_hash = format!("{:x}", md5::compute(&content));

                    if Some(&content_hash) != last_content_hash.as_ref() {
                        debug!("Clipboard content changed locally");

                        let entry = ClipboardEntry {
                            id: Uuid::new_v4(),
                            content: ClipboardData::Text(content),
                            timestamp: Utc::now(),
                            source: config.node_id(),
                            checksum: content_hash.clone(),
                        };

                        if let Err(e) = history.add_entry(&entry).await {
                            error!("Failed to save clipboard entry: {}", e);
                        }

                        let sync_event = SyncEvent {
                            timestamp: entry.timestamp,
                            source_peer: config.node_id(),
                            entry,
                        };

                        if let Err(e) = event_sender.send(sync_event) {
                            warn!("Failed to broadcast sync event: {}", e);
                        }

                        *last_update.write().await = SystemTime::now();
                        last_content_hash = Some(content_hash);
                    }
                }
                Err(e) => {
                    warn!("Failed to read clipboard: {}", e);
                }
            }
        }
    }

    async fn start_sync_loop(&self) -> Result<()> {
        let _transport = Arc::clone(&self.transport);
        let _peers = Arc::clone(&self.peers);
        let mut event_receiver = self.event_sender.subscribe();

        loop {
            match event_receiver.recv().await {
                Ok(sync_event) => {
                    if sync_event.source_peer == self.config.node_id() {
                        self.broadcast_to_peers(&sync_event).await;
                    } else {
                        self.handle_remote_sync_event(&sync_event).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    warn!("Sync loop lagged by {} events", count);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("Sync event channel closed");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn broadcast_to_peers(&self, event: &SyncEvent) {
        let peers = self.peers.read().await;

        let clipboard_data = match &event.entry.content {
            ClipboardData::Text(text) => TransportClipboardData {
                format: ClipboardFormat::Text,
                data: text.as_bytes().to_vec(),
                compression: None,
                checksum: event.entry.checksum.clone(),
                metadata: std::collections::HashMap::new(),
            },
        };

        let message = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(clipboard_data),
        );

        for peer in peers.values() {
            if let Err(e) = self.transport.send_to_peer(peer.id, &message).await {
                warn!("Failed to send sync event to peer {}: {}", peer.id, e);
            }
        }
    }

    async fn handle_remote_sync_event(&self, event: &SyncEvent) {
        debug!("Handling remote sync event from peer {}", event.source_peer);

        if let Err(e) = self.resolve_conflict(event).await {
            error!("Failed to resolve sync conflict: {}", e);
            return;
        }

        match &event.entry.content {
            ClipboardData::Text(text) => {
                if let Err(e) = self.clipboard.set_text(text).await {
                    error!("Failed to update local clipboard: {}", e);
                }
            }
        }

        if let Err(e) = self.history.add_entry(&event.entry).await {
            error!("Failed to save remote clipboard entry: {}", e);
        }
    }

    async fn resolve_conflict(&self, remote_event: &SyncEvent) -> Result<()> {
        let last_local = *self.last_local_update.read().await;
        let remote_timestamp = remote_event.timestamp.timestamp() as u64;
        let local_timestamp = last_local.duration_since(UNIX_EPOCH)?.as_secs();

        if remote_timestamp <= local_timestamp {
            debug!("Remote event is older than local, ignoring");
            return Ok(());
        }

        if let Some(existing) = self
            .history
            .get_by_checksum(&remote_event.entry.checksum)
            .await?
        {
            if existing.timestamp >= remote_event.entry.timestamp {
                debug!("Already have newer version of this content");
                return Ok(());
            }
        }

        Ok(())
    }

    async fn start_transport_handler(&self) -> Result<()> {
        let transport = Arc::clone(&self.transport);
        let event_sender = self.event_sender.clone();

        let mut message_receiver = transport.subscribe().await?;

        loop {
            match message_receiver.recv().await {
                Ok(message) => {
                    match message.payload {
                        MessagePayload::Clipboard(clipboard_data) => {
                            let content = match clipboard_data.format {
                                ClipboardFormat::Text => {
                                    match String::from_utf8(clipboard_data.data) {
                                        Ok(text) => ClipboardData::Text(text),
                                        Err(e) => {
                                            warn!("Failed to decode text clipboard data: {}", e);
                                            continue;
                                        }
                                    }
                                }
                                _ => {
                                    debug!(
                                        "Unsupported clipboard format: {:?}",
                                        clipboard_data.format
                                    );
                                    continue;
                                }
                            };

                            // Extract the source peer ID from the message
                            let source_peer_id = message.source_peer_id.unwrap_or_else(|| {
                                warn!("Message missing source_peer_id, using random UUID");
                                Uuid::new_v4()
                            });

                            let entry = ClipboardEntry {
                                id: Uuid::new_v4(),
                                content,
                                timestamp: message.timestamp,
                                source: source_peer_id,
                                checksum: clipboard_data.checksum,
                            };

                            let sync_event = SyncEvent {
                                timestamp: message.timestamp,
                                source_peer: entry.source,
                                entry,
                            };

                            if let Err(e) = event_sender.send(sync_event) {
                                warn!("Failed to broadcast received sync event: {}", e);
                            }
                        }
                        _ => {
                            debug!("Received non-sync message: {:?}", message.message_type);
                        }
                    }
                }
                Err(e) => {
                    error!("Error receiving transport message: {}", e);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.event_sender.subscribe()
    }

    pub async fn get_connected_peers(&self) -> Vec<Peer> {
        self.peers.read().await.values().cloned().collect()
    }

    pub async fn force_sync(&self) -> Result<()> {
        info!("Forcing clipboard sync");

        if let Ok(content) = self.clipboard.get_text().await {
            let checksum = format!("{:x}", md5::compute(&content));
            let entry = ClipboardEntry {
                id: Uuid::new_v4(),
                content: ClipboardData::Text(content),
                timestamp: Utc::now(),
                source: self.config.node_id(),
                checksum,
            };

            let sync_event = SyncEvent {
                timestamp: entry.timestamp,
                source_peer: self.config.node_id(),
                entry,
            };

            self.event_sender.send(sync_event)?;
        }

        Ok(())
    }
}

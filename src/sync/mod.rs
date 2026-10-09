#![deny(warnings, clippy::all)]
pub mod trust_sync;
use crate::{
    adapters::{
        ClipboardData, ClipboardEntry, ClipboardProviderWrapper, HistoryManager, Peer,
        PeerDiscovery,
    },
    config::Config,
    history::encryption::Encryptor,
    transport::{
        protocol::{ClipboardFormat, MessagePayload, MessageType},
        ClipboardData as WireData, Message, TransportManager, WebSocketListener,
    },
};
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{broadcast, Mutex};
pub use trust_sync::{setup_trust_sync, TrustAwareSyncEngine};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SyncEvent {
    pub timestamp: DateTime<Utc>,
    pub source_peer: Uuid,
    pub entry: ClipboardEntry,
}
#[derive(Default)]
struct SyncState {
    clock: u64,
    last_order: Option<(u64, Uuid, Uuid)>,
    observed: Option<String>,
    pending: Option<Message>,
    queued_sessions: HashMap<Uuid, Uuid>,
}
pub struct SyncEngine {
    config: Arc<Config>,
    clipboard: Arc<ClipboardProviderWrapper>,
    history: Arc<HistoryManager>,
    discovery: Option<Arc<PeerDiscovery>>,
    transport: Arc<TransportManager>,
    state: Mutex<SyncState>,
    events: broadcast::Sender<SyncEvent>,
}
impl SyncEngine {
    pub fn new(
        config: Arc<Config>,
        clipboard: Arc<ClipboardProviderWrapper>,
        history: Arc<HistoryManager>,
        discovery: Arc<PeerDiscovery>,
        transport: Arc<TransportManager>,
    ) -> Self {
        let mut engine = Self::without_discovery(config, clipboard, history, transport);
        engine.discovery = Some(discovery);
        engine
    }
    /// Use explicit peer connections, e.g. an isolated loopback test harness.
    pub fn without_discovery(
        config: Arc<Config>,
        clipboard: Arc<ClipboardProviderWrapper>,
        history: Arc<HistoryManager>,
        transport: Arc<TransportManager>,
    ) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            config,
            clipboard,
            history,
            discovery: None,
            transport,
            state: Mutex::new(SyncState::default()),
            events,
        }
    }
    pub async fn bind_listener(&self) -> Result<WebSocketListener> {
        Ok(self.transport.listener(self.config.socket_addr()?).await?)
    }
    pub async fn start(&self) -> Result<()> {
        self.run(self.bind_listener().await?).await
    }
    pub async fn run(&self, listener: WebSocketListener) -> Result<()> {
        let incoming = self.transport.subscribe().await?;
        // Baseline the pre-existing clipboard. Starting a daemon is not a copy action.
        self.state.lock().await.observed = self
            .clipboard
            .get_text()
            .await
            .ok()
            .map(|s| Encryptor::compute_checksum(s.as_bytes()));
        let result = tokio::try_join!(
            async {
                self.transport
                    .serve(listener)
                    .await
                    .map_err(anyhow::Error::from)
            },
            self.sync_loop(incoming),
            self.discovery_loop(),
        );
        self.shutdown().await;
        result.map(|_| ())
    }
    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
        if let Some(discovery) = &self.discovery {
            if let Err(e) = discovery.stop().await {
                tracing::warn!("Discovery shutdown: {e}");
            }
        }
    }
    async fn discovery_loop(&self) -> Result<()> {
        let Some(discovery) = &self.discovery else {
            return std::future::pending().await;
        };
        discovery.start().await?;
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let mut retry: HashMap<Uuid, (tokio::time::Instant, u32)> = HashMap::new();
        loop {
            interval.tick().await;
            let peers = discovery.snapshot().await?;
            retry.retain(|id, _| peers.iter().any(|p| p.id == *id));
            for peer in peers {
                // One initiator per pair prevents simultaneous duplicate connections.
                if peer.id <= self.config.node_id() || self.transport.is_connected(peer.id).await {
                    continue;
                }
                if retry
                    .get(&peer.id)
                    .is_some_and(|(when, _)| *when > tokio::time::Instant::now())
                {
                    continue;
                }
                match self.transport.connect_peer(&peer).await {
                    Ok(()) => {
                        retry.remove(&peer.id);
                    }
                    Err(e) => {
                        tracing::debug!("Peer {} is unavailable: {e}", peer.id);
                        let attempt = retry
                            .get(&peer.id)
                            .map_or(0, |(_, n)| *n)
                            .saturating_add(1)
                            .min(5);
                        retry.insert(
                            peer.id,
                            (
                                tokio::time::Instant::now() + Duration::from_secs(1 << attempt),
                                attempt,
                            ),
                        );
                    }
                }
            }
        }
    }
    async fn sync_loop(&self, mut incoming: broadcast::Receiver<Message>) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = interval.tick() => { if let Err(e) = self.capture(false).await { tracing::debug!("Clipboard capture: {e}"); } }
                message = incoming.recv() => match message {
                    Ok(m) => if let Err(e) = self.apply_remote(m).await { tracing::warn!("Remote clipboard rejected: {e}"); },
                    Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!("Dropped {n} clipboard messages"),
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    fn admit(&self, text: &str) -> Result<()> {
        if text.len() > self.config.clipboard.max_size {
            bail!("Clipboard exceeds configured size limit");
        }
        if crate::clipboard::safety::is_potentially_sensitive(text)
            || crate::clipboard::safety::is_sensitive_context()
        {
            bail!("Clipboard suppressed by sensitive-content policy");
        }
        Ok(())
    }
    async fn capture(&self, force: bool) -> Result<usize> {
        let mut state = self.state.lock().await;
        let text = self.clipboard.get_text().await?;
        self.admit(&text)?;
        let checksum = Encryptor::compute_checksum(text.as_bytes());
        if !force && state.observed.as_ref() == Some(&checksum) {
            return self.queue_pending(&mut state).await;
        }
        let peers = self.transport.connected_peers().await;
        if force && peers.is_empty() {
            bail!("No authenticated peers connected");
        }
        let id = Uuid::new_v4();
        let clock = state
            .clock
            .max(now_millis())
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Event clock exhausted"))?;
        let entry = ClipboardEntry {
            id,
            content: ClipboardData::Text(text.clone()),
            timestamp: Utc::now(),
            source: self.config.node_id(),
            checksum: checksum.clone(),
        };
        self.history.add_entry(&entry).await?;
        state.clock = clock;
        state.last_order = Some((clock, self.config.node_id(), id));
        state.observed = Some(checksum.clone());
        let mut message = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(WireData {
                format: ClipboardFormat::Text,
                data: text.into_bytes(),
                compression: None,
                checksum,
                metadata: HashMap::new(),
            }),
        );
        message.sequence = state.clock;
        message.correlation_id = Some(id);
        message.timestamp = entry.timestamp;
        state.pending = Some(message);
        state.queued_sessions.clear();
        let queued = self.queue_pending(&mut state).await?;
        let _ = self.events.send(SyncEvent {
            timestamp: entry.timestamp,
            source_peer: entry.source,
            entry,
        });
        if force && queued == 0 {
            bail!("Could not queue update to any connected peer");
        }
        Ok(queued)
    }
    async fn queue_pending(&self, state: &mut SyncState) -> Result<usize> {
        let Some(message) = &state.pending else {
            return Ok(0);
        };
        let sessions = self.transport.sessions().await;
        state
            .queued_sessions
            .retain(|peer, session| sessions.contains(&(*peer, *session)));
        let mut queued = 0;
        for (peer, session) in sessions {
            if state.queued_sessions.get(&peer) == Some(&session) {
                continue;
            }
            if self.transport.send_to_peer(peer, message).await.is_ok() {
                state.queued_sessions.insert(peer, session);
                queued += 1;
            }
        }
        Ok(queued)
    }
    pub async fn apply_remote(&self, message: Message) -> Result<()> {
        if message.message_type != MessageType::ClipboardData {
            return Ok(());
        }
        let MessagePayload::Clipboard(data) = message.payload else {
            bail!("Invalid clipboard message");
        };
        if data.format != ClipboardFormat::Text || data.compression.is_some() {
            bail!("Unsupported clipboard format");
        }
        let origin = message
            .source_peer_id
            .ok_or_else(|| anyhow::anyhow!("Missing authenticated origin"))?;
        let id = message
            .correlation_id
            .ok_or_else(|| anyhow::anyhow!("Missing event identity"))?;
        if origin == self.config.node_id() {
            bail!("Reflected local event");
        }
        if message.sequence > now_millis().saturating_add(300_000) {
            bail!("Peer clock is more than five minutes ahead");
        }
        let text = String::from_utf8(data.data)?;
        self.admit(&text)?;
        let checksum = Encryptor::compute_checksum(text.as_bytes());
        if checksum != data.checksum {
            bail!("Clipboard checksum mismatch");
        }
        // Capture a local change that happened between monitor ticks before admitting remote data.
        let _ = self.capture(false).await;
        let mut state = self.state.lock().await;
        let order = (message.sequence, origin, id);
        if state.last_order.is_some_and(|last| order <= last) {
            return Ok(());
        }
        self.clipboard.set_text(&text).await?;
        state.clock = state.clock.max(message.sequence);
        state.last_order = Some(order);
        state.pending = None;
        state.queued_sessions.clear();
        state.observed = Some(checksum.clone());
        let entry = ClipboardEntry {
            id,
            content: ClipboardData::Text(text),
            timestamp: message.timestamp,
            source: origin,
            checksum,
        };
        self.history.add_entry(&entry).await?;
        let _ = self.events.send(SyncEvent {
            timestamp: entry.timestamp,
            source_peer: origin,
            entry,
        });
        Ok(())
    }
    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.events.subscribe()
    }
    pub async fn get_connected_peers(&self) -> Vec<Peer> {
        self.transport
            .connected_peers()
            .await
            .into_iter()
            .map(|p| Peer {
                id: p.id,
                hostname: p.name.clone(),
                address: p.best_address().map(|a| a.to_string()).unwrap_or_default(),
            })
            .collect()
    }
    pub async fn force_sync(&self) -> Result<usize> {
        self.capture(true).await
    }
    /// Apply an explicit local copy through the daemon-owned clipboard provider.
    pub async fn copy_local(&self, text: String) -> Result<usize> {
        if text.len() > self.config.clipboard.max_size {
            bail!("Clipboard exceeds configured size limit");
        }
        let forward = !crate::clipboard::safety::is_potentially_sensitive(&text)
            && !crate::clipboard::safety::is_sensitive_context();
        let checksum = Encryptor::compute_checksum(text.as_bytes());
        let mut state = self.state.lock().await;
        self.clipboard.set_text(&text).await?;
        if !forward {
            state.observed = Some(checksum.clone());
            state.pending = None;
            state.queued_sessions.clear();
            return Ok(0);
        }
        let id = Uuid::new_v4();
        let clock = state
            .clock
            .max(now_millis())
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Event clock exhausted"))?;
        let entry = ClipboardEntry {
            id,
            content: ClipboardData::Text(text.clone()),
            timestamp: Utc::now(),
            source: self.config.node_id(),
            checksum: checksum.clone(),
        };
        self.history.add_entry(&entry).await?;
        state.observed = Some(checksum.clone());
        state.clock = clock;
        state.last_order = Some((clock, self.config.node_id(), id));
        let mut message = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(WireData {
                format: ClipboardFormat::Text,
                data: text.into_bytes(),
                compression: None,
                checksum: entry.checksum.clone(),
                metadata: HashMap::new(),
            }),
        );
        message.sequence = state.clock;
        message.correlation_id = Some(id);
        message.timestamp = entry.timestamp;
        state.pending = Some(message);
        state.queued_sessions.clear();
        let queued = self.queue_pending(&mut state).await?;
        let _ = self.events.send(SyncEvent {
            timestamp: entry.timestamp,
            source_peer: entry.source,
            entry,
        });
        Ok(queued)
    }
}
fn now_millis() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

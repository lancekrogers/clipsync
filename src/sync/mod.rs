#![deny(warnings, clippy::all)]
pub mod connection_policy;
pub mod discovery_connect;
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
use discovery_connect::{discovery_connect_peers, PeerDialState};
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
    ambiguous_clipboard: bool,
    uncertain_write: Option<UncertainWrite>,
    pending: Option<Message>,
    pending_image_dimensions: Option<(u32, u32)>,
    queued_sessions: HashMap<Uuid, Uuid>,
    rejected_sessions: HashMap<Uuid, (Uuid, crate::transport::ImageQueueOutcome)>,
    /// Session-scoped; daemon start always leaves this false.
    paused: bool,
    /// Monotonic for clients within this `instance_id` only.
    generation: u64,
    /// Last peer set advertised via control (for generation bumps).
    last_peer_ids: Vec<Uuid>,
}

struct UncertainWrite {
    content: crate::clipboard::ClipboardContent,
    observed: String,
    order: (u64, Uuid, Uuid),
    entry: ClipboardEntry,
}

/// Runtime snapshot for the control plane (not a wire DTO).
#[derive(Debug, Clone)]
pub struct EngineRuntime {
    pub instance_id: Uuid,
    pub state_generation: u64,
    pub paused: bool,
    pub outbound_pending: bool,
    pub node_id: Uuid,
    pub max_clipboard_bytes: usize,
    pub history_capacity: u32,
}

pub struct SyncEngine {
    config: Arc<Config>,
    clipboard: Arc<ClipboardProviderWrapper>,
    history: Arc<HistoryManager>,
    discovery: Option<Arc<PeerDiscovery>>,
    transport: Arc<TransportManager>,
    state: Mutex<SyncState>,
    events: broadcast::Sender<SyncEvent>,
    instance_id: Uuid,
    image_support: Option<(crate::transport::image_capability::ImageLimits, u64)>,
}

fn image_decode_gate() -> &'static tokio::sync::Semaphore {
    static GATE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Semaphore::new(1))
}

struct PreparedContent {
    entry: ClipboardData,
    wire_format: ClipboardFormat,
    data: Vec<u8>,
    checksum: String,
    observed: String,
    dimensions: Option<(u32, u32)>,
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
        let image_support = if clipboard.enable_image_capture() {
            let limits = transport.enable_image_capability(
                crate::transport::image_capability::ImageLimits {
                    max_bytes: config.clipboard.max_size,
                    max_width: 8192,
                    max_height: 8192,
                    max_pixels: 16_777_216,
                },
            );
            Some((limits, 384 * 1024 * 1024))
        } else {
            None
        };
        Self {
            config,
            clipboard,
            history,
            discovery: None,
            transport,
            state: Mutex::new(SyncState::default()),
            events,
            instance_id: Uuid::new_v4(),
            image_support,
        }
    }

    /// Override image limits in isolated integration harnesses.
    #[cfg(feature = "integration-tests")]
    pub fn enable_image_support_for_testing(
        &mut self,
        limits: crate::transport::image_capability::ImageLimits,
        decoder_budget: u64,
    ) {
        self.clipboard.enable_image_capture();
        let mut limits = limits;
        limits.max_bytes = limits.max_bytes.min(self.config.clipboard.max_size);
        let limits = self.transport.enable_image_capability(limits);
        self.image_support = Some((limits, decoder_budget));
    }

    async fn prepare_content(
        &self,
        content: crate::clipboard::ClipboardContent,
        enforce_policy: bool,
    ) -> Result<PreparedContent> {
        if content.mime_type == "text/plain" {
            let text = String::from_utf8(content.data)?;
            if enforce_policy {
                self.admit(&text)?;
            }
            let data = text.as_bytes().to_vec();
            let checksum = Encryptor::compute_checksum(&data);
            return Ok(PreparedContent {
                entry: ClipboardData::Text(text),
                wire_format: ClipboardFormat::Text,
                observed: checksum.clone(),
                checksum,
                data,
                dimensions: None,
            });
        }
        if content.mime_type != "image/png" && content.mime_type != "image/tiff" {
            bail!("Unsupported clipboard format");
        }
        let (mut limits, budget) = self
            .image_support
            .ok_or_else(|| anyhow::anyhow!("Image sync unavailable"))?;
        limits.max_bytes = limits.max_bytes.min(self.config.clipboard.max_size);
        if enforce_policy && crate::clipboard::safety::is_sensitive_context() {
            bail!("Clipboard suppressed by sensitive-content policy");
        }
        // Bound concurrent decoder allocations across all engines in this process.
        let _permit = image_decode_gate()
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("Image decoder unavailable"))?;
        let data = if content.mime_type == "image/tiff" {
            crate::clipboard::png::from_tiff(&content.data, limits, budget)?
        } else {
            content.data
        };
        let decoded = crate::clipboard::png::decode(&data, limits, budget)?;
        let observed = crate::clipboard::image_identity::rgba8_identity(
            decoded.width,
            decoded.height,
            &decoded.rgba,
        )?;
        let checksum = Encryptor::compute_checksum(&data);
        Ok(PreparedContent {
            entry: ClipboardData::Image(data.clone()),
            wire_format: ClipboardFormat::Image {
                mime_type: "image/png".into(),
            },
            data,
            checksum,
            observed,
            dimensions: Some((decoded.width, decoded.height)),
        })
    }

    async fn write_content_with_reconciliation(
        &self,
        content: &crate::clipboard::ClipboardContent,
    ) -> Result<()> {
        match self.clipboard.set_content(content).await {
            Ok(()) => Ok(()),
            Err(err) => {
                let unknown = matches!(
                    err.downcast_ref::<crate::clipboard::ClipboardError>(),
                    Some(crate::clipboard::ClipboardError::WriteOutcomeUnknown(_))
                );
                if content.mime_type == "image/png"
                    && unknown
                    && self
                        .clipboard
                        .reconcile_unknown_image(content, content)
                        .await
                        .unwrap_or(crate::clipboard::ImageAttribution::Unknown)
                        == crate::clipboard::ImageAttribution::Owned
                {
                    return Ok(());
                }
                Err(err)
            }
        }
    }

    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    pub fn history_manager(&self) -> Arc<HistoryManager> {
        Arc::clone(&self.history)
    }

    pub fn config_ref(&self) -> &Config {
        &self.config
    }

    /// Authoritative pause flag and generation for control Status/SetPause.
    pub async fn runtime(&self) -> EngineRuntime {
        let state = self.state.lock().await;
        EngineRuntime {
            instance_id: self.instance_id,
            state_generation: state.generation,
            paused: state.paused,
            outbound_pending: state.pending.is_some(),
            node_id: self.config.node_id(),
            max_clipboard_bytes: self.config.clipboard.max_size,
            history_capacity: HistoryManager::retention_capacity(),
        }
    }

    /// Note peer-set changes for `state_generation` (same instance only).
    pub async fn note_peer_set(&self, peer_ids: &[Uuid]) -> u64 {
        let mut state = self.state.lock().await;
        let mut sorted = peer_ids.to_vec();
        sorted.sort_unstable();
        if sorted != state.last_peer_ids {
            state.last_peer_ids = sorted;
            state.generation = state.generation.saturating_add(1);
        }
        state.generation
    }

    /// Enter or leave pause. Resume baselines `observed` under the state lock before unpausing.
    /// Failed clipboard read on resume leaves the daemon paused.
    ///
    /// Clearing `pending` / `queued_sessions` invalidates not-yet-sent outbound queue only.
    /// Traffic already handed to the OS clipboard or peer transport cannot be recalled.
    pub async fn set_pause(&self, paused: bool) -> Result<EngineRuntime> {
        if paused {
            let mut state = self.state.lock().await;
            if !state.paused {
                state.paused = true;
                state.uncertain_write = None;
                state.pending = None;
                state.queued_sessions.clear();
                state.rejected_sessions.clear();
                state.generation = state.generation.saturating_add(1);
            }
        } else {
            let mut state = self.state.lock().await;
            if state.paused {
                let content = self.clipboard.get_content().await.map_err(|e| {
                    anyhow::anyhow!("Resume failed: could not read clipboard ({e})")
                })?;
                let content = self.prepare_content(content, false).await.map_err(|e| {
                    anyhow::anyhow!("Resume failed: could not read clipboard ({e})")
                })?;
                state.observed = Some(content.observed);
                state.uncertain_write = None;
                state.paused = false;
                state.generation = state.generation.saturating_add(1);
            }
        }
        Ok(self.runtime_locked(&*self.state.lock().await))
    }

    fn runtime_locked(&self, state: &SyncState) -> EngineRuntime {
        EngineRuntime {
            instance_id: self.instance_id,
            state_generation: state.generation,
            paused: state.paused,
            outbound_pending: state.pending.is_some(),
            node_id: self.config.node_id(),
            max_clipboard_bytes: self.config.clipboard.max_size,
            history_capacity: HistoryManager::retention_capacity(),
        }
    }

    fn bump_generation(state: &mut SyncState) {
        state.generation = state.generation.saturating_add(1);
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
        let baseline = match self.clipboard.get_content().await {
            Ok(content) => self
                .prepare_content(content, false)
                .await
                .ok()
                .map(|content| content.observed),
            Err(_) => None,
        };
        self.state.lock().await.observed = baseline;
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
        let mut retry: HashMap<Uuid, PeerDialState> = HashMap::new();
        loop {
            interval.tick().await;
            let now = std::time::Instant::now();
            let peers = discovery.snapshot().await?;
            discovery_connect_peers(
                &self.transport,
                self.config.node_id(),
                &peers,
                &mut retry,
                now,
            )
            .await;
        }
    }
    async fn sync_loop(&self, mut incoming: broadcast::Receiver<Message>) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = interval.tick() => { if let Err(e) = self.capture(false).await { tracing::debug!("Clipboard capture: {e}"); } }
                message = incoming.recv() => match message {
                    Ok(m) => if let Err(e) = self.apply_remote(m).await { tracing::warn!("Remote clipboard processing: {e}"); },
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
        if state.paused {
            if force {
                bail!("Sync paused");
            }
            return Ok(0);
        }
        let content = match self.clipboard.get_content().await {
            Ok(content) => content,
            Err(err) => {
                if matches!(
                    err.downcast_ref::<crate::clipboard::ClipboardError>(),
                    Some(crate::clipboard::ClipboardError::Ambiguous)
                ) && !state.ambiguous_clipboard
                {
                    tracing::warn!("Clipboard sync paused for an ambiguous mixed image copy; copy again to resume");
                    state.ambiguous_clipboard = true;
                }
                return Err(err);
            }
        };
        let prepared = self.prepare_content(content.clone(), true).await?;
        if let Some(uncertain) = state.uncertain_write.take() {
            if prepared.observed == uncertain.observed {
                let attribution = self
                    .clipboard
                    .reconcile_unknown_image(&uncertain.content, &content)
                    .await
                    .unwrap_or(crate::clipboard::ImageAttribution::Unknown);
                if attribution == crate::clipboard::ImageAttribution::Owned {
                    if state.ambiguous_clipboard {
                        tracing::info!("Clipboard sync resumed after an attributable image");
                        state.ambiguous_clipboard = false;
                    }
                    state.clock = state.clock.max(uncertain.order.0);
                    state.last_order = Some(uncertain.order);
                    state.pending = None;
                    state.pending_image_dimensions = None;
                    state.queued_sessions.clear();
                    state.rejected_sessions.clear();
                    state.observed = Some(uncertain.observed);
                    Self::bump_generation(&mut state);
                    self.history
                        .add_entry(&uncertain.entry)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!("Clipboard applied; history save failed: {e}")
                        })?;
                    let mut queued = 0;
                    if uncertain.entry.source == self.config.node_id() {
                        let mut message = Message::new(
                            MessageType::ClipboardData,
                            MessagePayload::Clipboard(
                                WireData {
                                    format: ClipboardFormat::Image {
                                        mime_type: "image/png".into(),
                                    },
                                    data: uncertain.content.data,
                                    compression: None,
                                    checksum: uncertain.entry.checksum.clone(),
                                    metadata: HashMap::new(),
                                }
                                .into(),
                            ),
                        );
                        message.sequence = uncertain.order.0;
                        message.correlation_id = Some(uncertain.entry.id);
                        message.timestamp = uncertain.entry.timestamp;
                        state.pending = Some(message);
                        state.pending_image_dimensions = prepared.dimensions;
                        queued = self.queue_pending(&mut state).await?;
                    }
                    let _ = self.events.send(SyncEvent {
                        timestamp: uncertain.entry.timestamp,
                        source_peer: uncertain.entry.source,
                        entry: uncertain.entry,
                    });
                    return Ok(queued);
                }
                if attribution == crate::clipboard::ImageAttribution::Unknown {
                    state.uncertain_write = Some(uncertain);
                    if !state.ambiguous_clipboard {
                        tracing::warn!("Clipboard sync paused for an unattributed image write; copy again to resume");
                        state.ambiguous_clipboard = true;
                    }
                    return Err(crate::clipboard::ClipboardError::Ambiguous.into());
                }
                // A confirmed new owner is a new copy even when pixels are identical.
                state.observed = None;
            }
            // A different, unambiguous copy wins; it must be captured normally.
        }
        if state.ambiguous_clipboard {
            tracing::info!("Clipboard sync resumed after an unambiguous copy");
            state.ambiguous_clipboard = false;
        }
        if !force && state.observed.as_ref() == Some(&prepared.observed) {
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
            content: prepared.entry,
            timestamp: Utc::now(),
            source: self.config.node_id(),
            checksum: prepared.checksum.clone(),
        };
        self.history.add_entry(&entry).await?;
        Self::bump_generation(&mut state);
        state.clock = clock;
        state.last_order = Some((clock, self.config.node_id(), id));
        state.observed = Some(prepared.observed);
        state.pending_image_dimensions = prepared.dimensions;
        let mut message = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(
                WireData {
                    format: prepared.wire_format,
                    data: prepared.data,
                    compression: None,
                    checksum: prepared.checksum,
                    metadata: HashMap::new(),
                }
                .into(),
            ),
        );
        message.sequence = state.clock;
        message.correlation_id = Some(id);
        message.timestamp = entry.timestamp;
        state.pending = Some(message);
        state.queued_sessions.clear();
        state.rejected_sessions.clear();
        let queued = self.queue_pending(&mut state).await?;
        let _ = self.events.send(SyncEvent {
            timestamp: entry.timestamp,
            source_peer: entry.source,
            entry,
        });
        if force && queued == 0 {
            if state.pending_image_dimensions.is_some() {
                use crate::transport::ImageQueueOutcome;
                let mut outcomes = state
                    .rejected_sessions
                    .values()
                    .map(|(_, outcome)| *outcome);
                let first = outcomes.next();
                let same = first.is_some() && outcomes.all(|outcome| Some(outcome) == first);
                if same {
                    match first {
                        Some(ImageQueueOutcome::Unsupported) => {
                            bail!("Image not queued: peers do not support image sync")
                        }
                        Some(ImageQueueOutcome::OverLimit) => {
                            bail!("Image not queued: image exceeds peer limits")
                        }
                        Some(ImageQueueOutcome::QueueFull) => {
                            bail!("Image not queued: image queue is full")
                        }
                        _ => {}
                    }
                }
                bail!("Image not queued: no eligible peer accepted it");
            }
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
        state
            .rejected_sessions
            .retain(|peer, (session, _)| sessions.contains(&(*peer, *session)));
        let mut queued = 0;
        for (peer, session) in sessions {
            if state.queued_sessions.get(&peer) == Some(&session) {
                continue;
            }
            let sent = if let Some((width, height)) = state.pending_image_dimensions {
                use crate::transport::ImageQueueOutcome;
                let MessagePayload::Clipboard(data) = &message.payload else {
                    continue;
                };
                let outcome = self
                    .transport
                    .send_image_to_session(peer, session, message, data.data.len(), width, height)
                    .await;
                if outcome != ImageQueueOutcome::Queued
                    && outcome != ImageQueueOutcome::StaleSession
                    && state.rejected_sessions.get(&peer) != Some(&(session, outcome))
                {
                    match outcome {
                        ImageQueueOutcome::Unsupported => tracing::warn!(
                            "Image not queued for peer {peer}: image capability unavailable"
                        ),
                        ImageQueueOutcome::OverLimit => tracing::warn!(
                            "Image not queued for peer {peer}: image exceeds peer limits"
                        ),
                        ImageQueueOutcome::QueueFull => {
                            tracing::warn!("Image not queued for peer {peer}: image queue is full")
                        }
                        _ => {}
                    }
                    state.rejected_sessions.insert(peer, (session, outcome));
                }
                if outcome == ImageQueueOutcome::Queued {
                    state.rejected_sessions.remove(&peer);
                }
                outcome == ImageQueueOutcome::Queued
            } else {
                self.transport.send_to_peer(peer, message).await.is_ok()
            };
            if sent {
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
        // While paused: drop apply-side effects (no clipboard, history, or order update).
        // Transport may still have accepted the frame for session health; it cannot be recalled.
        if self.state.lock().await.paused {
            return Ok(());
        }
        let MessagePayload::Clipboard(data) = message.payload else {
            bail!("Invalid clipboard message");
        };
        if data.compression.is_some() {
            bail!("Unsupported clipboard compression");
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
        if Encryptor::compute_checksum(&data.data) != data.checksum {
            bail!("Clipboard checksum mismatch");
        }
        let mime_type = match data.format {
            ClipboardFormat::Text => "text/plain",
            ClipboardFormat::Image { ref mime_type } if mime_type == "image/png" => "image/png",
            _ => bail!("Unsupported clipboard format"),
        };
        let content = crate::clipboard::ClipboardContent {
            mime_type: mime_type.into(),
            data: data.data.clone(),
            timestamp: message.timestamp.timestamp(),
        };
        let prepared = self.prepare_content(content.clone(), true).await?;
        // Capture a local change that happened between monitor ticks before admitting remote data.
        if let Err(err) = self.capture(false).await {
            if matches!(
                err.downcast_ref::<crate::clipboard::ClipboardError>(),
                Some(crate::clipboard::ClipboardError::Ambiguous)
            ) {
                return Err(err);
            }
        }
        let mut state = self.state.lock().await;
        if state.paused {
            return Ok(());
        }
        let order = (message.sequence, origin, id);
        if state.last_order.is_some_and(|last| order <= last) {
            return Ok(());
        }
        if let Err(err) = self.write_content_with_reconciliation(&content).await {
            if content.mime_type == "image/png"
                && matches!(
                    err.downcast_ref::<crate::clipboard::ClipboardError>(),
                    Some(crate::clipboard::ClipboardError::WriteOutcomeUnknown(_))
                )
            {
                state.uncertain_write = Some(UncertainWrite {
                    content,
                    observed: prepared.observed,
                    order,
                    entry: ClipboardEntry {
                        id,
                        content: prepared.entry,
                        timestamp: message.timestamp,
                        source: origin,
                        checksum: prepared.checksum,
                    },
                });
            }
            return Err(err);
        }
        state.uncertain_write = None;
        state.clock = state.clock.max(message.sequence);
        state.last_order = Some(order);
        state.pending = None;
        state.pending_image_dimensions = None;
        state.queued_sessions.clear();
        state.rejected_sessions.clear();
        state.observed = Some(prepared.observed);
        let entry = ClipboardEntry {
            id,
            content: prepared.entry,
            timestamp: message.timestamp,
            source: origin,
            checksum: prepared.checksum,
        };
        Self::bump_generation(&mut state);
        self.history
            .add_entry(&entry)
            .await
            .map_err(|e| anyhow::anyhow!("Clipboard applied; history save failed: {e}"))?;
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
    /// While paused: local pasteboard only (no history append, no outbound).
    pub async fn copy_local(&self, text: String) -> Result<usize> {
        if text.len() > self.config.clipboard.max_size {
            bail!("Clipboard exceeds configured size limit");
        }
        let forward = !crate::clipboard::safety::is_potentially_sensitive(&text)
            && !crate::clipboard::safety::is_sensitive_context();
        let checksum = Encryptor::compute_checksum(text.as_bytes());
        let mut state = self.state.lock().await;
        self.clipboard.set_text(&text).await?;
        state.uncertain_write = None;
        if state.paused || !forward {
            state.observed = Some(checksum);
            state.pending = None;
            state.queued_sessions.clear();
            state.rejected_sessions.clear();
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
        Self::bump_generation(&mut state);
        state.observed = Some(checksum.clone());
        state.clock = clock;
        state.last_order = Some((clock, self.config.node_id(), id));
        let mut message = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(
                WireData {
                    format: ClipboardFormat::Text,
                    data: text.into_bytes(),
                    compression: None,
                    checksum: entry.checksum.clone(),
                    metadata: HashMap::new(),
                }
                .into(),
            ),
        );
        message.sequence = state.clock;
        message.correlation_id = Some(id);
        message.timestamp = entry.timestamp;
        state.pending = Some(message);
        state.pending_image_dimensions = None;
        state.queued_sessions.clear();
        state.rejected_sessions.clear();
        let queued = self.queue_pending(&mut state).await?;
        let _ = self.events.send(SyncEvent {
            timestamp: entry.timestamp,
            source_peer: entry.source,
            entry,
        });
        Ok(queued)
    }

    async fn copy_image(&self, data: Vec<u8>) -> Result<usize> {
        let content = crate::clipboard::ClipboardContent::image(data, "png");
        let prepared = self.prepare_content(content.clone(), true).await?;
        let mut state = self.state.lock().await;
        let id = Uuid::new_v4();
        let clock = state
            .clock
            .max(now_millis())
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Event clock exhausted"))?;
        let entry = ClipboardEntry {
            id,
            content: prepared.entry.clone(),
            timestamp: Utc::now(),
            source: self.config.node_id(),
            checksum: prepared.checksum.clone(),
        };
        if let Err(err) = self.write_content_with_reconciliation(&content).await {
            if !state.paused
                && matches!(
                    err.downcast_ref::<crate::clipboard::ClipboardError>(),
                    Some(crate::clipboard::ClipboardError::WriteOutcomeUnknown(_))
                )
            {
                state.uncertain_write = Some(UncertainWrite {
                    content,
                    observed: prepared.observed,
                    order: (clock, self.config.node_id(), id),
                    entry,
                });
            }
            return Err(err);
        }
        state.uncertain_write = None;
        state.observed = Some(prepared.observed);
        state.pending = None;
        state.pending_image_dimensions = None;
        state.queued_sessions.clear();
        state.rejected_sessions.clear();
        if state.paused {
            return Ok(0);
        }
        state.clock = clock;
        state.last_order = Some((clock, self.config.node_id(), id));
        Self::bump_generation(&mut state);
        self.history
            .add_entry(&entry)
            .await
            .map_err(|e| anyhow::anyhow!("Clipboard applied; history save failed: {e}"))?;
        let mut message = Message::new(
            MessageType::ClipboardData,
            MessagePayload::Clipboard(
                WireData {
                    format: prepared.wire_format,
                    data: prepared.data,
                    compression: None,
                    checksum: prepared.checksum,
                    metadata: HashMap::new(),
                }
                .into(),
            ),
        );
        message.sequence = clock;
        message.correlation_id = Some(id);
        message.timestamp = entry.timestamp;
        state.pending = Some(message);
        state.pending_image_dimensions = prepared.dimensions;
        let queued = self.queue_pending(&mut state).await?;
        let _ = self.events.send(SyncEvent {
            timestamp: entry.timestamp,
            source_peer: entry.source,
            entry,
        });
        Ok(queued)
    }

    /// Load history by retained UUID and run the same path as an explicit local copy.
    pub async fn history_copy(&self, id: Uuid) -> Result<usize> {
        let entry = self
            .history
            .get_entry(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("History entry not found"))?;
        match entry.content {
            ClipboardData::Text(text) => self.copy_local(text).await,
            ClipboardData::Image(data) => self.copy_image(data).await,
        }
    }

    pub async fn is_paused(&self) -> bool {
        self.state.lock().await.paused
    }
}
fn now_millis() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

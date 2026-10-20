//! Control Target surface. Default methods preserve phase-0 mock impls.
use crate::{
    adapters::{ClipboardData, Peer},
    control::{
        diagnostics, error_codes, history_view,
        schema::{
            ControlSnapshot, DaemonStatus, DiagnosticsReport, HistoryEntryDetail, HistoryPage,
        },
        FEATURE_ENROLLMENT,
    },
    enrollment::EnrollmentManager,
    sync::{SyncEngine, TrustAwareSyncEngine},
};
use anyhow::{bail, Result};
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait Target: Send + Sync {
    async fn peers(&self) -> Vec<Peer>;
    async fn sync(&self) -> Result<usize>;
    async fn copy_text(&self, text: String) -> Result<usize>;

    /// Enriched Status. Defaults to peers-only with empty features (phase-0 mocks).
    async fn control_snapshot(&self) -> Result<ControlSnapshot> {
        let peers = self.peers().await;
        Ok(ControlSnapshot {
            features: Vec::new(),
            instance_id: Uuid::nil(),
            state_generation: 0,
            status: DaemonStatus {
                paused: false,
                observed_at: Utc::now().to_rfc3339(),
                connected_peer_count: peers.len() as u32,
                history_available: false,
                node_id: Uuid::nil().to_string(),
                max_clipboard_bytes: 0,
                history_capacity: 20,
                outbound_pending: false,
            },
            peers,
        })
    }

    async fn set_pause(&self, _paused: bool) -> Result<ControlSnapshot> {
        bail!("Unsupported control command");
    }

    async fn history_list(&self, _limit: u32) -> Result<HistoryPage> {
        bail!("Unsupported control command");
    }

    async fn history_search(&self, _query: String, _limit: u32) -> Result<HistoryPage> {
        bail!("Unsupported control command");
    }

    async fn history_get(&self, _id: Uuid) -> Result<HistoryEntryDetail> {
        bail!("Unsupported control command");
    }

    async fn history_copy(&self, _id: Uuid) -> Result<(usize, bool)> {
        bail!("Unsupported control command");
    }

    async fn diagnostics_export(&self) -> Result<DiagnosticsReport> {
        bail!("Unsupported control command");
    }

    /// Whether sync is currently paused (Copy/HistoryCopy report via pause_applied).
    async fn is_paused(&self) -> bool {
        false
    }

    /// Present only when daemon was constructed with an EnrollmentManager.
    fn enrollment_manager(&self) -> Option<Arc<EnrollmentManager>> {
        None
    }
}

/// Wraps an existing control Target and advertises enrollment when a manager is attached.
/// Root can pass this to `Server::run` without changing SyncEngine.
pub struct EnrollmentTarget {
    inner: Arc<dyn Target>,
    manager: Arc<EnrollmentManager>,
}

impl EnrollmentTarget {
    pub fn new(inner: Arc<dyn Target>, manager: Arc<EnrollmentManager>) -> Self {
        Self { inner, manager }
    }
}

#[async_trait::async_trait]
impl Target for EnrollmentTarget {
    async fn peers(&self) -> Vec<Peer> {
        self.inner.peers().await
    }
    async fn sync(&self) -> Result<usize> {
        self.inner.sync().await
    }
    async fn copy_text(&self, text: String) -> Result<usize> {
        self.inner.copy_text(text).await
    }
    async fn control_snapshot(&self) -> Result<ControlSnapshot> {
        let mut snap = self.inner.control_snapshot().await?;
        if !snap.features.iter().any(|f| f == FEATURE_ENROLLMENT) {
            snap.features.push(FEATURE_ENROLLMENT.into());
        }
        Ok(snap)
    }
    async fn set_pause(&self, paused: bool) -> Result<ControlSnapshot> {
        let mut snap = self.inner.set_pause(paused).await?;
        if !snap.features.iter().any(|f| f == FEATURE_ENROLLMENT) {
            snap.features.push(FEATURE_ENROLLMENT.into());
        }
        Ok(snap)
    }
    async fn history_list(&self, limit: u32) -> Result<HistoryPage> {
        self.inner.history_list(limit).await
    }
    async fn history_search(&self, query: String, limit: u32) -> Result<HistoryPage> {
        self.inner.history_search(query, limit).await
    }
    async fn history_get(&self, id: Uuid) -> Result<HistoryEntryDetail> {
        self.inner.history_get(id).await
    }
    async fn history_copy(&self, id: Uuid) -> Result<(usize, bool)> {
        self.inner.history_copy(id).await
    }
    async fn diagnostics_export(&self) -> Result<DiagnosticsReport> {
        self.inner.diagnostics_export().await
    }
    async fn is_paused(&self) -> bool {
        self.inner.is_paused().await
    }
    fn enrollment_manager(&self) -> Option<Arc<EnrollmentManager>> {
        Some(self.manager.clone())
    }
}

async fn engine_snapshot(engine: &SyncEngine) -> Result<ControlSnapshot> {
    let peers = engine.get_connected_peers().await;
    let peer_ids: Vec<Uuid> = peers.iter().map(|p| p.id).collect();
    let generation = engine.note_peer_set(&peer_ids).await;
    let runtime = engine.runtime().await;
    Ok(ControlSnapshot {
        features: error_codes::engine_features(),
        instance_id: runtime.instance_id,
        state_generation: generation,
        status: DaemonStatus {
            paused: runtime.paused,
            observed_at: Utc::now().to_rfc3339(),
            connected_peer_count: peers.len() as u32,
            history_available: true,
            node_id: runtime.node_id.to_string(),
            max_clipboard_bytes: runtime.max_clipboard_bytes,
            history_capacity: runtime.history_capacity,
            outbound_pending: runtime.outbound_pending,
        },
        peers,
    })
}

async fn engine_history_list(engine: &SyncEngine, limit: u32) -> Result<HistoryPage> {
    let limit = history_view::clamp_limit(limit);
    let entries = engine
        .history_manager()
        .get_recent_entries(limit as usize)
        .await
        .map_err(|e| anyhow::anyhow!("History unavailable: {e}"))?;
    Ok(history_view::page_from_entries(&entries, None, limit))
}

async fn engine_history_search(
    engine: &SyncEngine,
    query: String,
    limit: u32,
) -> Result<HistoryPage> {
    history_view::validate_query(&query)?;
    let limit = history_view::clamp_limit(limit);
    let entries = engine
        .history_manager()
        .search_entries(&query, limit as usize)
        .await
        .map_err(|e| anyhow::anyhow!("History unavailable: {e}"))?;
    Ok(history_view::page_from_entries(
        &entries,
        Some(query),
        limit,
    ))
}

async fn engine_history_get(engine: &SyncEngine, id: Uuid) -> Result<HistoryEntryDetail> {
    let max_bytes = engine.config_ref().clipboard.max_size;
    let entry = engine
        .history_manager()
        .get_entry(id)
        .await
        .map_err(|e| anyhow::anyhow!("History unavailable: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("{}", history_view::missing_id_message(id)))?;
    match &entry.content {
        ClipboardData::Text(text) if text.len() > max_bytes => {
            bail!("Clipboard exceeds configured size limit");
        }
        _ => {}
    }
    history_view::to_detail(&entry, max_bytes)
}

#[async_trait::async_trait]
impl Target for SyncEngine {
    async fn peers(&self) -> Vec<Peer> {
        self.get_connected_peers().await
    }
    async fn sync(&self) -> Result<usize> {
        self.force_sync().await
    }
    async fn copy_text(&self, text: String) -> Result<usize> {
        self.copy_local(text).await
    }
    async fn control_snapshot(&self) -> Result<ControlSnapshot> {
        engine_snapshot(self).await
    }
    async fn set_pause(&self, paused: bool) -> Result<ControlSnapshot> {
        SyncEngine::set_pause(self, paused).await?;
        engine_snapshot(self).await
    }
    async fn history_list(&self, limit: u32) -> Result<HistoryPage> {
        engine_history_list(self, limit).await
    }
    async fn history_search(&self, query: String, limit: u32) -> Result<HistoryPage> {
        engine_history_search(self, query, limit).await
    }
    async fn history_get(&self, id: Uuid) -> Result<HistoryEntryDetail> {
        engine_history_get(self, id).await
    }
    async fn history_copy(&self, id: Uuid) -> Result<(usize, bool)> {
        let paused = SyncEngine::is_paused(self).await;
        let queued = SyncEngine::history_copy(self, id).await?;
        Ok((queued, paused))
    }
    async fn diagnostics_export(&self) -> Result<DiagnosticsReport> {
        let snap = engine_snapshot(self).await?;
        Ok(diagnostics::build_report(&snap))
    }
    async fn is_paused(&self) -> bool {
        SyncEngine::is_paused(self).await
    }
}

#[async_trait::async_trait]
impl Target for TrustAwareSyncEngine {
    async fn peers(&self) -> Vec<Peer> {
        self.get_connected_peers().await
    }
    async fn sync(&self) -> Result<usize> {
        self.force_sync().await
    }
    async fn copy_text(&self, text: String) -> Result<usize> {
        self.copy_local(text).await
    }
    async fn control_snapshot(&self) -> Result<ControlSnapshot> {
        engine_snapshot(self.sync_engine().as_ref()).await
    }
    async fn set_pause(&self, paused: bool) -> Result<ControlSnapshot> {
        TrustAwareSyncEngine::set_pause(self, paused).await?;
        engine_snapshot(self.sync_engine().as_ref()).await
    }
    async fn history_list(&self, limit: u32) -> Result<HistoryPage> {
        engine_history_list(self.sync_engine().as_ref(), limit).await
    }
    async fn history_search(&self, query: String, limit: u32) -> Result<HistoryPage> {
        engine_history_search(self.sync_engine().as_ref(), query, limit).await
    }
    async fn history_get(&self, id: Uuid) -> Result<HistoryEntryDetail> {
        engine_history_get(self.sync_engine().as_ref(), id).await
    }
    async fn history_copy(&self, id: Uuid) -> Result<(usize, bool)> {
        let paused = TrustAwareSyncEngine::is_paused(self).await;
        let queued = TrustAwareSyncEngine::history_copy(self, id).await?;
        Ok((queued, paused))
    }
    async fn diagnostics_export(&self) -> Result<DiagnosticsReport> {
        let snap = engine_snapshot(self.sync_engine().as_ref()).await?;
        Ok(diagnostics::build_report(&snap))
    }
    async fn is_paused(&self) -> bool {
        TrustAwareSyncEngine::is_paused(self).await
    }
}

//! Enrollment manager: listener lifecycle, sessions, confirm/revoke.
//!
//! Session TLS I/O is owned by [`session_io::SessionIo`] actors — confirm/cancel
//! never share a duplex Mutex with a blocking reader.

use super::{
    channel::{self, EnrollConn},
    commit_gate::{CommitAbort, CommitGate},
    limits::{
        RateLimits, MAX_CONCURRENT_SESSIONS, MAX_CONSUMED_IDS, MAX_TERMINAL_SESSIONS, SESSION_TTL,
    },
    mdns::EnrollMdns,
    protocol::{self, binding_hex, sanitize_display_name, EnrollMessage, PERSIST_ACK_TIMEOUT_SECS},
    session_io::{IoCommand, SessionEvent, SessionIo},
};
use crate::{
    auth::{Authenticator, PublicKey, SshAuthenticator},
    config::Config,
    control::{
        EnrollmentCandidateView, EnrollmentOutcomeView, EnrollmentSessionView, EnrollmentSnapshot,
        TrustedDeviceView,
    },
    transport::tls,
};
use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, sync::RwLock, task::JoinHandle};
use uuid::Uuid;

/// Bound waiting for local PersistAck write flush after durable pin.
const IO_ACK_FLUSH_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Acceptor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Dialing,
    /// Reserved for post-TLS pre-hello UI (API surface); dial goes Dialing→AwaitingLocalConfirm.
    #[allow(dead_code)]
    Connected,
    AwaitingLocalConfirm,
    AwaitingPeerConfirm,
    Persisting,
    WaitingPeerAuth,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl SessionState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Dialing => "Dialing",
            Self::Connected => "Connected",
            Self::AwaitingLocalConfirm => "AwaitingLocalConfirm",
            Self::AwaitingPeerConfirm => "AwaitingPeerConfirm",
            Self::Persisting => "Persisting",
            Self::WaitingPeerAuth => "WaitingPeerAuth",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Expired => "Expired",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Cancelled | Self::Expired | Self::Succeeded
        )
    }

    fn is_active(&self) -> bool {
        !self.is_terminal() && !matches!(self, Self::WaitingPeerAuth)
    }
}

struct LiveSession {
    session_id: Uuid,
    role: Role,
    state: SessionState,
    local_fp: String,
    local_nonce: [u8; 32],
    peer_session_id: Option<Uuid>,
    peer_fp: Option<String>,
    peer_key: Option<PublicKey>,
    peer_nonce: Option<[u8; 32]>,
    peer_node_id: Option<Uuid>,
    peer_display_name: Option<String>,
    dialed_addr: Option<SocketAddr>,
    local_confirmed: bool,
    peer_confirmed: bool,
    local_persisted: bool,
    local_ack_flushed: bool,
    peer_persisted_ack: bool,
    /// Linearizes cancel vs durable commit; never revoke via best-effort remove.
    commit_gate: Arc<CommitGate>,
    expires_at: Instant,
    error: Option<String>,
    io: Option<SessionIo>,
    event_task: Option<JoinHandle<()>>,
}

struct Inner {
    config: Arc<Config>,
    auth: Arc<SshAuthenticator>,
    auth_dyn: Arc<dyn Authenticator>,
    listening: bool,
    listen_addr: Option<SocketAddr>,
    listen_port_override: Option<u16>,
    /// Test hook: inject short TTL (no wall-clock 5-minute sleeps).
    ttl_override: Option<Duration>,
    window_expires: Option<Instant>,
    sessions: HashMap<Uuid, LiveSession>,
    /// Manual / injected candidates (merged with mDNS on discover).
    manual_candidates: Vec<EnrollmentCandidateView>,
    consumed_sessions: VecDeque<(Uuid, Instant)>,
    rates: RateLimits,
    /// In-flight TLS handshakes (reserved before spawn, released after bind/fail).
    pending_handshakes: usize,
    accept_task: Option<JoinHandle<()>>,
    expiry_task: Option<JoinHandle<()>>,
    listener: Option<Arc<TcpListener>>,
    display_name: String,
    last_outcome: Option<EnrollmentOutcomeView>,
    mdns: EnrollMdns,
}

/// Daemon-owned enrollment controller.
pub struct EnrollmentManager {
    inner: Arc<RwLock<Inner>>,
}

impl Drop for EnrollmentManager {
    fn drop(&mut self) {
        // Abort background accept/expiry/session tasks so listeners do not leak
        // across tests (tasks otherwise keep `Inner` alive via cloned Arcs).
        if let Ok(mut g) = self.inner.try_write() {
            if let Some(t) = g.accept_task.take() {
                t.abort();
            }
            if let Some(t) = g.expiry_task.take() {
                t.abort();
            }
            g.listening = false;
            g.listener = None;
            for s in g.sessions.values_mut() {
                Self::abort_session_io(s);
            }
        }
    }
}

impl EnrollmentManager {
    pub fn new(config: Arc<Config>, auth: Arc<SshAuthenticator>) -> Result<Self> {
        let display_name =
            sanitize_display_name(&config.advertise_name).unwrap_or_else(|_| "clipsync".into());
        let our_node_id = config.node_id;
        let auth_dyn: Arc<dyn Authenticator> = auth.clone();
        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                config,
                auth,
                auth_dyn,
                listening: false,
                listen_addr: None,
                listen_port_override: None,
                ttl_override: None,
                window_expires: None,
                sessions: HashMap::new(),
                manual_candidates: Vec::new(),
                consumed_sessions: VecDeque::new(),
                rates: RateLimits::default(),
                pending_handshakes: 0,
                accept_task: None,
                expiry_task: None,
                listener: None,
                display_name,
                last_outcome: None,
                mdns: EnrollMdns::new(our_node_id),
            })),
        })
    }

    /// Tests: bind enrollment listener to ephemeral port on the config IP.
    pub async fn set_listen_port_override(&self, port: u16) {
        self.inner.write().await.listen_port_override = Some(port);
    }

    /// Tests: inject a short enrollment TTL (virtual long-idle / expiry).
    pub async fn set_ttl_override(&self, ttl: Duration) {
        self.inner.write().await.ttl_override = Some(ttl);
    }

    /// Inject an untrusted discovery hint (tests / manual UI).
    pub async fn inject_candidate(&self, candidate: EnrollmentCandidateView) {
        self.inner.write().await.manual_candidates.push(candidate);
    }

    fn session_ttl(g: &Inner) -> Duration {
        g.ttl_override.unwrap_or(SESSION_TTL)
    }

    pub async fn open(&self) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        let bind = Self::resolve_listen_addr(&g)?;
        let ttl = Self::session_ttl(&g);
        if g.listening {
            g.window_expires = Some(Instant::now() + ttl);
            Self::spawn_expiry_watcher(self.inner.clone(), &mut g);
            return Ok(Self::snapshot_locked(&g));
        }
        let listener = TcpListener::bind(bind)
            .await
            .context("Enrollment bind failed")?;
        let local = listener.local_addr()?;
        let listener = Arc::new(listener);
        g.listener = Some(listener.clone());
        g.listen_addr = Some(local);
        g.listening = true;
        g.window_expires = Some(Instant::now() + ttl);
        g.last_outcome = None;

        let local_fp = g.auth.get_public_key().await?.fingerprint();
        let display = g.display_name.clone();
        // mDNS advertise is best-effort; loopback/ephemeral tests often have no
        // usable iface — discovery still works via inject_candidate / manual dial.
        match g.mdns.start_and_advertise(&display, local, &local_fp).await {
            Ok(()) => {}
            Err(e) => {
                tracing::debug!("Enrollment mDNS advertise skipped: {e}");
            }
        }

        let mgr = self.inner.clone();
        let auth = g.auth_dyn.clone();
        g.accept_task = Some(tokio::spawn(async move {
            loop {
                let tcp = {
                    let guard = mgr.read().await;
                    let Some(listener) = guard.listener.clone() else {
                        break;
                    };
                    drop(guard);
                    match listener.accept().await {
                        Ok((tcp, _)) => tcp,
                        Err(_) => break,
                    }
                };
                let mgr2 = mgr.clone();
                let auth2 = auth.clone();
                // Reserve handshake capacity before spawn.
                {
                    let mut g = mgr2.write().await;
                    Self::expire_locked(&mut g).await;
                    if !g.listening {
                        continue;
                    }
                    if !Self::can_start_handshake(&g) {
                        continue;
                    }
                    if !g.rates.allow_accept(Instant::now()) {
                        continue;
                    }
                    g.pending_handshakes += 1;
                }
                tokio::spawn(async move {
                    let result = Self::handle_accept(mgr2.clone(), auth2, tcp).await;
                    {
                        let mut g = mgr2.write().await;
                        g.pending_handshakes = g.pending_handshakes.saturating_sub(1);
                    }
                    if let Err(e) = result {
                        tracing::debug!("Enrollment accept failed: {e}");
                    }
                });
            }
        }));
        Self::spawn_expiry_watcher(self.inner.clone(), &mut g);
        Ok(Self::snapshot_locked(&g))
    }

    pub async fn discover(&self) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        // Closed window still returns snapshot (no hard error) so UI can show prior state.
        if g.listening {
            let mdns = g.mdns.snapshot_candidates().await;
            // Merge: mDNS first, then manual/injected not already present by address.
            let mut merged = mdns;
            for c in &g.manual_candidates {
                if !merged.iter().any(|m| m.address == c.address) {
                    merged.push(c.clone());
                }
            }
            // Store merged view into a temporary field via snapshot building.
            let snap = Self::snapshot_with_candidates(&g, merged);
            return Ok(snap);
        }
        Ok(Self::snapshot_locked(&g))
    }

    /// Start dial in the background and return a snapshot promptly (IPC-safe).
    pub async fn dial(
        &self,
        address: &str,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    ) -> Result<EnrollmentSnapshot> {
        let addr: SocketAddr = address.parse().context("Enrollment dial address invalid")?;
        let (auth, local_fp, display_name, session_id, nonce, expires_at) = {
            let mut g = self.inner.write().await;
            Self::expire_locked(&mut g).await;
            if !g.listening {
                bail!("Enrollment closed");
            }
            if !Self::can_start_handshake(&g) {
                bail!("Enrollment busy");
            }
            let now = Instant::now();
            if !g.rates.allow_accept(now) {
                bail!("Enrollment rate limited");
            }
            let local_fp = g.auth.get_public_key().await?.fingerprint();
            let session_id = Uuid::new_v4();
            let nonce = protocol::random_nonce();
            let expires_at = g.window_expires.unwrap_or(now + Self::session_ttl(&g));
            let session = LiveSession {
                session_id,
                role: Role::Initiator,
                state: SessionState::Dialing,
                local_fp: local_fp.clone(),
                local_nonce: nonce,
                peer_session_id: None,
                peer_fp: None,
                peer_key: None,
                peer_nonce: None,
                peer_node_id: None,
                peer_display_name: None,
                dialed_addr: Some(addr),
                local_confirmed: false,
                peer_confirmed: false,
                local_persisted: false,
                local_ack_flushed: false,
                peer_persisted_ack: false,
                commit_gate: Arc::new(CommitGate::new()),
                expires_at,
                error: None,
                io: None,
                event_task: None,
            };
            // Dialing session itself counts toward active capacity (no pending bump).
            g.sessions.insert(session_id, session);
            (
                g.auth_dyn.clone(),
                local_fp,
                g.display_name.clone(),
                session_id,
                nonce,
                expires_at,
            )
        };

        let mgr = self.inner.clone();
        let expected_fingerprint = expected_fingerprint.clone();
        tokio::spawn(async move {
            let result = Self::dial_handshake(
                mgr.clone(),
                auth,
                addr,
                session_id,
                nonce,
                local_fp,
                display_name,
                expected_fingerprint,
                expected_node_id,
                expires_at,
            )
            .await;
            if let Err(e) = result {
                let mut g = mgr.write().await;
                if let Some(s) = g.sessions.get_mut(&session_id) {
                    if !s.state.is_terminal() {
                        s.state = SessionState::Failed;
                        s.error = Some(e.to_string());
                        Self::abort_session_io(s);
                    }
                }
            }
        });

        Ok(self.snapshot().await)
    }

    pub async fn inspect(&self, session_id: Option<Uuid>) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        // Closed enrollment still returns a snapshot (GUI Devices polls Inspect).
        if let Some(id) = session_id {
            if !g.sessions.contains_key(&id) && g.listening {
                bail!("Enrollment session not found");
            }
        }
        Ok(Self::snapshot_locked(&g))
    }

    pub async fn confirm(&self, session_id: Uuid) -> Result<EnrollmentSnapshot> {
        let (msg, io_tx) = {
            let mut g = self.inner.write().await;
            Self::expire_locked(&mut g).await;
            if !g.listening {
                bail!("Enrollment closed");
            }
            if Self::is_consumed(&g, session_id) {
                bail!("Enrollment replay");
            }
            let s = g
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| anyhow::anyhow!("Enrollment session not found"))?;
            if s.state.is_terminal() {
                bail!("Enrollment cancelled");
            }
            if Instant::now() >= s.expires_at {
                s.state = SessionState::Expired;
                Self::abort_session_io(s);
                bail!("Enrollment expired");
            }
            let peer_fp = s
                .peer_fp
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch"))?;
            let peer_nonce = s
                .peer_nonce
                .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch"))?;
            let peer_session_id = s
                .peer_session_id
                .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch"))?;
            let io_tx =
                s.io.as_ref()
                    .map(|io| io.cmd_tx.clone())
                    .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch: TLS disconnected"))?;
            let binding = binding_hex(&s.local_nonce, &peer_nonce, &s.local_fp, &peer_fp);
            s.local_confirmed = true;
            if s.peer_confirmed {
                s.state = SessionState::Persisting;
            } else {
                s.state = SessionState::AwaitingPeerConfirm;
            }
            (
                EnrollMessage::Confirm {
                    session_id,
                    peer_session_id,
                    binding_hex: binding,
                    confirmed: true,
                },
                io_tx,
            )
        };

        // Non-blocking send into session actor — does not wait on peer network.
        if io_tx.send(IoCommand::send(msg)).await.is_err() {
            let mut g = self.inner.write().await;
            if let Some(s) = g.sessions.get_mut(&session_id) {
                if !s.state.is_terminal() {
                    s.state = SessionState::Failed;
                    s.error = Some("Enrollment TLS disconnected".into());
                    Self::abort_session_io(s);
                }
            }
            bail!("Enrollment mismatch: TLS disconnected");
        }

        // If peer already confirmed, begin durable persist (serialized with cancel).
        let should_persist = {
            let g = self.inner.read().await;
            g.sessions
                .get(&session_id)
                .map(|s| s.local_confirmed && s.peer_confirmed && !s.local_persisted)
                .unwrap_or(false)
        };
        if should_persist {
            let mgr = self.inner.clone();
            tokio::spawn(async move {
                let _ = Self::persist_mutual_static(mgr, session_id).await;
            });
        }

        Ok(self.snapshot().await)
    }

    pub async fn cancel(&self, session_id: Option<Uuid>) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        let ids: Vec<Uuid> = match session_id {
            Some(id) => vec![id],
            None => g.sessions.keys().copied().collect(),
        };
        for id in ids {
            let action = g.sessions.get(&id).map(|s| {
                if matches!(s.state, SessionState::Persisting) {
                    1
                } else if matches!(
                    s.state,
                    SessionState::WaitingPeerAuth | SessionState::Succeeded
                ) {
                    // Commit won or already authorized — never revoke / Cancelled.
                    0
                } else if s.commit_gate.is_committed() {
                    0
                } else if !s.state.is_terminal() {
                    2
                } else {
                    0
                }
            });
            match action {
                Some(1) => {
                    // Race with durable publish: cancel wins only if still Open.
                    if let Some(s) = g.sessions.get_mut(&id) {
                        if s.commit_gate.try_cancel() {
                            s.state = SessionState::Cancelled;
                            Self::abort_session_io(s);
                            Self::mark_consumed(&mut g, id);
                        }
                        // else: commit in progress or done — leave pin / state alone
                    }
                }
                Some(2) => {
                    if let Some(s) = g.sessions.get_mut(&id) {
                        if s.commit_gate.is_committed() {
                            // Point of no return — never Cancelled / never revoke pin.
                            continue;
                        }
                        let _ = s.commit_gate.try_cancel();
                        s.state = SessionState::Cancelled;
                        if let Some(io) = s.io.take() {
                            let _ = io.cmd_tx.try_send(IoCommand::CancelNotify(id));
                            io.abort();
                        }
                        if let Some(t) = s.event_task.take() {
                            t.abort();
                        }
                        Self::mark_consumed(&mut g, id);
                    }
                }
                _ => {}
            }
        }
        if session_id.is_none() {
            Self::close_listener_locked(&mut g).await;
            g.last_outcome = Some(EnrollmentOutcomeView {
                status: "cancelled".into(),
                peer_fingerprint: None,
                detail: Some("Enrollment cancelled".into()),
            });
        }
        Ok(Self::snapshot_locked(&g))
    }

    pub async fn revoke(&self, fingerprint: &str) -> Result<Vec<TrustedDeviceView>> {
        let auth = self.inner.read().await.auth.clone();
        auth.remove_peer(fingerprint).await?;
        self.trusted_devices().await
    }

    pub async fn trusted_devices(&self) -> Result<Vec<TrustedDeviceView>> {
        let auth = self.inner.read().await.auth.clone();
        let devices = auth.list_trusted_devices().await?;
        Ok(devices
            .into_iter()
            .map(|(fingerprint, comment)| TrustedDeviceView {
                fingerprint,
                comment,
            })
            .collect())
    }

    pub async fn snapshot(&self) -> EnrollmentSnapshot {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        Self::snapshot_locked(&g)
    }

    pub async fn is_authorized(&self, key: &PublicKey) -> Result<bool> {
        let auth = self.inner.read().await.auth.clone();
        auth.is_authorized(key).await.map_err(Into::into)
    }

    // --- internals ---

    fn can_start_handshake(g: &Inner) -> bool {
        let active = g.sessions.values().filter(|s| s.state.is_active()).count();
        active + g.pending_handshakes < MAX_CONCURRENT_SESSIONS
    }

    fn resolve_listen_addr(g: &Inner) -> Result<SocketAddr> {
        let configured: SocketAddr = g.config.socket_addr().map_err(|e| anyhow::anyhow!("{e}"))?;
        let ip = configured.ip();
        let port = if let Some(p) = g.listen_port_override {
            p
        } else {
            let sync = configured.port();
            let candidate = sync.saturating_add(1);
            if candidate == 0 || candidate == sync {
                bail!("Enrollment listen port unavailable");
            }
            candidate
        };
        let ip = if matches!(ip, IpAddr::V4(v) if v.is_unspecified())
            && g.listen_port_override == Some(0)
        {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            ip
        };
        Ok(SocketAddr::new(ip, port))
    }

    fn bind_checks(
        g: &Inner,
        session_id: Uuid,
        peer_fp: &str,
        peer_node: Uuid,
        expected_fp: Option<&str>,
        expected_node: Option<Uuid>,
    ) -> Result<(), String> {
        let local_fp = g
            .sessions
            .get(&session_id)
            .map(|s| s.local_fp.clone())
            .unwrap_or_default();
        if peer_fp == local_fp {
            return Err("Enrollment reflection".into());
        }
        if let Some(exp) = expected_fp {
            if exp != peer_fp {
                return Err("Enrollment mismatch".into());
            }
        }
        if let Some(exp) = expected_node {
            if exp != peer_node {
                return Err("Enrollment mismatch".into());
            }
        }
        if let Some(s) = g.sessions.get(&session_id) {
            if let Some(addr) = s.dialed_addr {
                for c in &g.manual_candidates {
                    if c.address == addr.to_string() {
                        if let Some(afp) = &c.advertised_fingerprint {
                            if afp != peer_fp {
                                return Err("Enrollment mismatch".into());
                            }
                        }
                        if let Some(aid) = c.advertised_node_id {
                            if aid != peer_node {
                                return Err("Enrollment mismatch".into());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dial_handshake(
        mgr: Arc<RwLock<Inner>>,
        auth: Arc<dyn Authenticator>,
        addr: SocketAddr,
        session_id: Uuid,
        nonce: [u8; 32],
        local_fp: String,
        display_name: String,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
        _expires_at: Instant,
    ) -> Result<()> {
        // Abort if cancelled while dialing.
        {
            let g = mgr.read().await;
            if g.sessions
                .get(&session_id)
                .map(|s| s.state.is_terminal())
                .unwrap_or(true)
            {
                bail!("Enrollment cancelled");
            }
        }

        let mut conn = channel::dial(addr, auth).await?;
        let peer_key = conn.peer_key.clone();
        let peer_fp = peer_key.fingerprint();
        let peer_node = tls::node_id(&peer_key);

        {
            let mut g = mgr.write().await;
            // Window / capacity recheck after TLS.
            if !g.listening {
                bail!("Enrollment closed");
            }
            if let Err(msg) = Self::bind_checks(
                &g,
                session_id,
                &peer_fp,
                peer_node,
                expected_fingerprint.as_deref(),
                expected_node_id,
            ) {
                g.rates.record_failure(&peer_fp, Instant::now());
                if let Some(s) = g.sessions.get_mut(&session_id) {
                    s.state = SessionState::Failed;
                    s.error = Some(msg.clone());
                    Self::abort_session_io(s);
                }
                bail!("{msg}");
            }
        }

        let hello = EnrollMessage::Hello {
            session_id,
            nonce_hex: hex::encode(nonce),
            local_fingerprint: local_fp.clone(),
            display_name: display_name.clone(),
        };
        conn.write_msg(&hello).await?;
        let ack = conn.read_msg().await?;
        let (peer_session_id, peer_nonce, peer_name) = match ack {
            EnrollMessage::HelloAck {
                session_id: their_id,
                peer_session_id,
                nonce_hex,
                local_fingerprint,
                display_name,
            } => {
                if peer_session_id != session_id {
                    bail!("Enrollment mismatch");
                }
                if local_fingerprint != peer_fp {
                    bail!("Enrollment mismatch");
                }
                let peer_nonce = decode_nonce(&nonce_hex)?;
                let peer_name = sanitize_display_name(&display_name)?;
                (their_id, peer_nonce, peer_name)
            }
            EnrollMessage::Error { message, .. } => bail!("{message}"),
            _ => bail!("Enrollment mismatch"),
        };

        Self::attach_session_io(
            mgr,
            session_id,
            conn,
            Some(peer_key),
            Some(peer_fp),
            Some(peer_node),
            Some(peer_session_id),
            Some(peer_nonce),
            Some(peer_name),
            SessionState::AwaitingLocalConfirm,
        )
        .await?;
        Ok(())
    }

    async fn handle_accept(
        mgr: Arc<RwLock<Inner>>,
        auth: Arc<dyn Authenticator>,
        tcp: tokio::net::TcpStream,
    ) -> Result<()> {
        {
            let g = mgr.read().await;
            if !g.listening {
                bail!("Enrollment closed");
            }
        }

        let mut conn = channel::accept(tcp, auth).await?;
        let peer_key = conn.peer_key.clone();
        let peer_fp = peer_key.fingerprint();
        let peer_node = tls::node_id(&peer_key);

        // Recheck window/capacity after TLS handshake.
        {
            let mut g = mgr.write().await;
            Self::expire_locked(&mut g).await;
            if !g.listening {
                bail!("Enrollment closed");
            }
            let active = g.sessions.values().filter(|s| s.state.is_active()).count();
            // pending_handshakes includes us; allow if active < max.
            if active >= MAX_CONCURRENT_SESSIONS {
                bail!("Enrollment busy");
            }
        }

        let hello = conn.read_msg().await?;
        let (peer_session_id, peer_nonce, peer_name, peer_hello_fp) = match hello {
            EnrollMessage::Hello {
                session_id,
                nonce_hex,
                local_fingerprint,
                display_name,
            } => (
                session_id,
                decode_nonce(&nonce_hex)?,
                sanitize_display_name(&display_name)?,
                local_fingerprint,
            ),
            _ => bail!("Enrollment mismatch"),
        };
        if peer_hello_fp != peer_fp {
            let mut g = mgr.write().await;
            g.rates.record_failure(&peer_fp, Instant::now());
            bail!("Enrollment mismatch");
        }

        let (session_id, local_fp, nonce, display_name) = {
            let mut g = mgr.write().await;
            let local_fp = g.auth.get_public_key().await?.fingerprint();
            if peer_fp == local_fp {
                g.rates.record_failure(&peer_fp, Instant::now());
                bail!("Enrollment reflection");
            }
            if !g.rates.allow_failure_budget(&peer_fp, Instant::now()) {
                bail!("Enrollment rate limited");
            }
            let session_id = Uuid::new_v4();
            let nonce = protocol::random_nonce();
            let expires = g
                .window_expires
                .unwrap_or(Instant::now() + Self::session_ttl(&g));
            let session = LiveSession {
                session_id,
                role: Role::Acceptor,
                state: SessionState::AwaitingLocalConfirm,
                local_fp: local_fp.clone(),
                local_nonce: nonce,
                peer_session_id: Some(peer_session_id),
                peer_fp: Some(peer_fp.clone()),
                peer_key: Some(peer_key.clone()),
                peer_nonce: Some(peer_nonce),
                peer_node_id: Some(peer_node),
                peer_display_name: Some(peer_name.clone()),
                dialed_addr: Some(conn.peer_addr),
                local_confirmed: false,
                peer_confirmed: false,
                local_persisted: false,
                local_ack_flushed: false,
                peer_persisted_ack: false,
                commit_gate: Arc::new(CommitGate::new()),
                expires_at: expires,
                error: None,
                io: None,
                event_task: None,
            };
            g.sessions.insert(session_id, session);
            (session_id, local_fp, nonce, g.display_name.clone())
        };

        conn.write_msg(&EnrollMessage::HelloAck {
            session_id,
            peer_session_id,
            nonce_hex: hex::encode(nonce),
            local_fingerprint: local_fp,
            display_name,
        })
        .await?;

        Self::attach_session_io(
            mgr,
            session_id,
            conn,
            Some(peer_key),
            Some(peer_fp),
            Some(peer_node),
            Some(peer_session_id),
            Some(peer_nonce),
            Some(peer_name),
            SessionState::AwaitingLocalConfirm,
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn attach_session_io(
        mgr: Arc<RwLock<Inner>>,
        session_id: Uuid,
        conn: EnrollConn,
        peer_key: Option<PublicKey>,
        peer_fp: Option<String>,
        peer_node: Option<Uuid>,
        peer_session_id: Option<Uuid>,
        peer_nonce: Option<[u8; 32]>,
        peer_name: Option<String>,
        state: SessionState,
    ) -> Result<()> {
        let (reader, writer, _, _) = conn.into_split();
        let (io, mut evt_rx) = SessionIo::spawn(reader, writer);
        {
            let mut g = mgr.write().await;
            let s = g
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| anyhow::anyhow!("Enrollment session not found"))?;
            if s.state.is_terminal() {
                io.abort();
                bail!("Enrollment cancelled");
            }
            if let Some(k) = peer_key {
                s.peer_key = Some(k);
            }
            if let Some(fp) = peer_fp {
                s.peer_fp = Some(fp);
            }
            if let Some(n) = peer_node {
                s.peer_node_id = Some(n);
            }
            if let Some(id) = peer_session_id {
                s.peer_session_id = Some(id);
            }
            if let Some(n) = peer_nonce {
                s.peer_nonce = Some(n);
            }
            if let Some(name) = peer_name {
                s.peer_display_name = Some(name);
            }
            s.state = state;
            s.io = Some(io);
        }

        let mgr2 = mgr.clone();
        let event_task = tokio::spawn(async move {
            while let Some(evt) = evt_rx.recv().await {
                match evt {
                    SessionEvent::Message(msg) => {
                        Self::on_session_message(mgr2.clone(), session_id, msg).await;
                    }
                    SessionEvent::Disconnected | SessionEvent::WriteFailed => {
                        let mut g = mgr2.write().await;
                        if let Some(s) = g.sessions.get_mut(&session_id) {
                            Self::abort_session_io(s);
                            if s.state.is_active()
                                && !s.local_confirmed
                                && !matches!(s.state, SessionState::WaitingPeerAuth)
                            {
                                s.state = SessionState::Failed;
                                s.error = Some("Enrollment TLS disconnected".into());
                            }
                        }
                        break;
                    }
                }
            }
        });
        {
            let mut g = mgr.write().await;
            if let Some(s) = g.sessions.get_mut(&session_id) {
                s.event_task = Some(event_task);
            }
        }
        Ok(())
    }

    async fn on_session_message(mgr: Arc<RwLock<Inner>>, session_id: Uuid, msg: EnrollMessage) {
        match msg {
            EnrollMessage::Confirm {
                session_id: their_sid,
                peer_session_id,
                binding_hex: their_binding,
                confirmed,
            } => {
                let should_persist = Self::handle_peer_confirm(
                    &mgr,
                    session_id,
                    their_sid,
                    peer_session_id,
                    &their_binding,
                    confirmed,
                )
                .await;
                if should_persist {
                    let _ = Self::persist_mutual_static(mgr, session_id).await;
                }
            }
            EnrollMessage::PersistAck {
                session_id: their_sid,
                peer_session_id,
                authorized_fingerprint,
            } => {
                Self::handle_persist_ack(
                    &mgr,
                    session_id,
                    their_sid,
                    peer_session_id,
                    &authorized_fingerprint,
                )
                .await;
            }
            EnrollMessage::Cancel { .. } => {
                let mut g = mgr.write().await;
                let mut mark = false;
                if let Some(s) = g.sessions.get_mut(&session_id) {
                    if s.commit_gate.is_committed()
                        || matches!(
                            s.state,
                            SessionState::WaitingPeerAuth | SessionState::Succeeded
                        )
                    {
                        // Commit already published — ignore cancel for trust.
                    } else if matches!(s.state, SessionState::Persisting) {
                        if s.commit_gate.try_cancel() {
                            s.state = SessionState::Cancelled;
                            Self::abort_session_io(s);
                            mark = true;
                        }
                    } else if !s.state.is_terminal() {
                        let _ = s.commit_gate.try_cancel();
                        s.state = SessionState::Cancelled;
                        Self::abort_session_io(s);
                        mark = true;
                    }
                }
                if mark {
                    Self::mark_consumed(&mut g, session_id);
                }
            }
            EnrollMessage::Error { message, .. } => {
                let mut g = mgr.write().await;
                if let Some(s) = g.sessions.get_mut(&session_id) {
                    if !s.state.is_terminal() {
                        s.state = SessionState::Failed;
                        s.error = Some(message);
                        Self::abort_session_io(s);
                    }
                }
            }
            _ => {}
        }
    }

    async fn handle_peer_confirm(
        mgr: &Arc<RwLock<Inner>>,
        session_id: Uuid,
        their_sid: Uuid,
        peer_session_id: Uuid,
        their_binding: &str,
        confirmed: bool,
    ) -> bool {
        let mut g = mgr.write().await;
        let outcome = {
            let s = match g.sessions.get_mut(&session_id) {
                Some(s) => s,
                None => return false,
            };
            if s.state.is_terminal()
                || matches!(
                    s.state,
                    SessionState::WaitingPeerAuth | SessionState::Persisting
                )
            {
                return false;
            }
            if peer_session_id != session_id || their_sid != s.peer_session_id.unwrap_or_default() {
                s.state = SessionState::Failed;
                s.error = Some("Enrollment mismatch".into());
                Self::abort_session_io(s);
                return false;
            }
            let Some(peer_nonce) = s.peer_nonce else {
                return false;
            };
            let Some(peer_fp) = s.peer_fp.clone() else {
                return false;
            };
            let ours = binding_hex(&s.local_nonce, &peer_nonce, &s.local_fp, &peer_fp);
            let alt = binding_hex(&peer_nonce, &s.local_nonce, &peer_fp, &s.local_fp);
            if their_binding != ours && their_binding != alt {
                s.state = SessionState::Failed;
                s.error = Some("Enrollment mismatch".into());
                Self::abort_session_io(s);
                Some((peer_fp, false, true))
            } else if confirmed {
                s.peer_confirmed = true;
                let both = s.local_confirmed;
                if both {
                    s.state = SessionState::Persisting;
                } else {
                    s.state = SessionState::AwaitingLocalConfirm;
                }
                Some((peer_fp, both, false))
            } else {
                s.state = SessionState::Cancelled;
                Self::abort_session_io(s);
                Some((peer_fp, false, false))
            }
        };
        match outcome {
            Some((peer_fp, both, binding_bad)) => {
                if binding_bad {
                    g.rates.record_failure(&peer_fp, Instant::now());
                }
                both
            }
            None => false,
        }
    }

    async fn handle_persist_ack(
        mgr: &Arc<RwLock<Inner>>,
        session_id: Uuid,
        their_sid: Uuid,
        peer_session_id: Uuid,
        authorized_fingerprint: &str,
    ) {
        let mut g = mgr.write().await;
        let local_done = {
            let s = match g.sessions.get_mut(&session_id) {
                Some(s) => s,
                None => return,
            };
            if s.state.is_terminal() && !matches!(s.state, SessionState::Succeeded) {
                return;
            }
            if peer_session_id != session_id || their_sid != s.peer_session_id.unwrap_or_default() {
                return;
            }
            if authorized_fingerprint != s.local_fp {
                return;
            }
            s.peer_persisted_ack = true;
            let local_done = s.local_persisted && s.local_ack_flushed;
            // A peer ack can arrive between our publish and our ack write.
            // Keep the writer alive until its own flush is acknowledged.
            if local_done {
                s.state = SessionState::Succeeded;
                Self::abort_session_io(s);
            }
            local_done.then(|| s.peer_fp.clone())
        };
        if let Some(peer_fp) = local_done {
            Self::mark_consumed(&mut g, session_id);
            g.last_outcome = Some(EnrollmentOutcomeView {
                status: "succeeded".into(),
                peer_fingerprint: peer_fp,
                detail: Some("Mutual durable authorization acknowledged".into()),
            });
        }
    }

    async fn persist_mutual_static(mgr: Arc<RwLock<Inner>>, session_id: Uuid) -> Result<()> {
        let (auth, peer_key, peer_fp, comment, io_tx, peer_session_id, gate, expires_at) = {
            let mut g = mgr.write().await;
            let s = g
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| anyhow::anyhow!("Enrollment session not found"))?;
            if s.state.is_terminal() {
                bail!("Enrollment cancelled");
            }
            if s.commit_gate.is_cancelled() {
                s.state = SessionState::Cancelled;
                Self::abort_session_io(s);
                bail!("Enrollment cancelled");
            }
            if Instant::now() >= s.expires_at {
                let _ = s.commit_gate.try_cancel();
                s.state = SessionState::Expired;
                Self::abort_session_io(s);
                bail!("Enrollment expired");
            }
            if !(s.local_confirmed && s.peer_confirmed) {
                bail!("Enrollment incomplete");
            }
            if s.local_persisted {
                return Ok(());
            }
            s.state = SessionState::Persisting;
            let peer_key = s
                .peer_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch"))?;
            let peer_fp = s
                .peer_fp
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch"))?;
            let peer_session_id = s
                .peer_session_id
                .ok_or_else(|| anyhow::anyhow!("Enrollment mismatch"))?;
            let comment = s
                .peer_display_name
                .clone()
                .map(|n| format!("{n} {peer_fp}"));
            let io_tx = s.io.as_ref().map(|io| io.cmd_tx.clone());
            let gate = s.commit_gate.clone();
            let expires_at = s.expires_at;
            (
                g.auth.clone(),
                peer_key,
                peer_fp,
                comment,
                io_tx,
                peer_session_id,
                gate,
                expires_at,
            )
        };

        // Precommit under auth flock: cancel/deadline checked before publish.
        // Never best-effort remove_peer after write (would delete preexisting pins).
        let gate_before = gate.clone();
        let gate_after = gate.clone();
        let gate_fail = gate.clone();
        let write_result = auth
            .add_trusted_peer_with_commit_hooks(
                &peer_key.to_openssh(),
                comment,
                move || match gate_before.begin_commit(expires_at) {
                    Ok(()) => Ok(()),
                    Err(CommitAbort::Cancelled) => Err(crate::auth::AuthError::CommitAborted(
                        "Enrollment cancelled".into(),
                    )),
                    Err(CommitAbort::Expired) => Err(crate::auth::AuthError::CommitAborted(
                        "Enrollment expired".into(),
                    )),
                },
                move || gate_after.finish_commit(),
                move || gate_fail.abort_commit_attempt(),
            )
            .await;

        let post =
            Self::finish_persist_locked(&mgr, session_id, &peer_fp, &write_result, &gate).await?;
        if post == "aborted" {
            // Cancel/expiry won before publish — no pin written, no revoke.
            return Ok(());
        }

        // Flush PersistAck BEFORE any Succeeded abort so the peer can reach Succeeded too.
        let flushed = if let Some(tx) = io_tx {
            let (cmd, ack_rx) = IoCommand::send_acked(EnrollMessage::PersistAck {
                session_id,
                peer_session_id,
                authorized_fingerprint: peer_fp.clone(),
            });
            tx.send(cmd).await.is_ok()
                && matches!(
                    tokio::time::timeout(Duration::from_secs(IO_ACK_FLUSH_SECS), ack_rx).await,
                    Ok(Ok(Ok(())))
                )
        } else {
            false
        };

        Self::complete_after_ack_flush(&mgr, session_id, &peer_fp, flushed).await;

        let still_waiting = {
            let g = mgr.read().await;
            g.sessions
                .get(&session_id)
                .map(|s| matches!(s.state, SessionState::WaitingPeerAuth) && !s.peer_persisted_ack)
                .unwrap_or(false)
        };
        if still_waiting {
            let mgr2 = mgr.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(PERSIST_ACK_TIMEOUT_SECS)).await;
                let mut g = mgr2.write().await;
                let incomplete = g
                    .sessions
                    .get(&session_id)
                    .map(|s| {
                        matches!(s.state, SessionState::WaitingPeerAuth) && !s.peer_persisted_ack
                    })
                    .unwrap_or(false);
                if incomplete {
                    let fp = g.sessions.get(&session_id).and_then(|s| s.peer_fp.clone());
                    if let Some(s) = g.sessions.get_mut(&session_id) {
                        Self::abort_session_io(s);
                    }
                    Self::mark_consumed(&mut g, session_id);
                    g.last_outcome = Some(EnrollmentOutcomeView {
                        status: "incomplete".into(),
                        peer_fingerprint: fp,
                        detail: Some("Local pin written; peer durable ack not observed".into()),
                    });
                }
            });
        }
        Ok(())
    }

    async fn finish_persist_locked(
        mgr: &Arc<RwLock<Inner>>,
        session_id: Uuid,
        peer_fp: &str,
        write_result: &Result<(), crate::auth::AuthError>,
        gate: &CommitGate,
    ) -> Result<&'static str> {
        let mut g = mgr.write().await;
        enum Step {
            Fail(String),
            PublishedButUncertain(String),
            Aborted,
            /// Local pin durable; ack not flushed yet — keep IO alive.
            WaitingFlush,
        }
        let step = {
            let Some(s) = g.sessions.get_mut(&session_id) else {
                return Ok("done");
            };
            match write_result {
                Err(e) if gate.is_committed() => {
                    // Publish is irreversible even if the final directory sync fails.
                    // Keep the pin, report incomplete, and never acknowledge durability.
                    s.local_persisted = true;
                    s.state = SessionState::WaitingPeerAuth;
                    let message = format!(
                        "Authorization was saved here, but completion could not be confirmed: {e}"
                    );
                    s.error = Some(message.clone());
                    Self::abort_session_io(s);
                    Step::PublishedButUncertain(message)
                }
                Err(e) => {
                    let aborted = matches!(e, crate::auth::AuthError::CommitAborted(_))
                        || gate.is_cancelled();
                    if aborted {
                        if !s.state.is_terminal() {
                            s.state = if Instant::now() >= s.expires_at {
                                SessionState::Expired
                            } else {
                                SessionState::Cancelled
                            };
                            Self::abort_session_io(s);
                        }
                        Step::Aborted
                    } else if !s.state.is_terminal() {
                        s.state = SessionState::Failed;
                        s.error = Some(e.to_string());
                        Self::abort_session_io(s);
                        Step::Fail(e.to_string())
                    } else {
                        Step::Fail(e.to_string())
                    }
                }
                Ok(()) => {
                    // Committed: never Cancelled, never revoke.
                    s.local_persisted = true;
                    s.state = SessionState::WaitingPeerAuth;
                    Step::WaitingFlush
                }
            }
        };
        match step {
            Step::PublishedButUncertain(message) => {
                Self::mark_consumed(&mut g, session_id);
                g.last_outcome = Some(EnrollmentOutcomeView {
                    status: "incomplete".into(),
                    peer_fingerprint: Some(peer_fp.to_string()),
                    detail: Some(message),
                });
                Ok("aborted")
            }
            Step::Fail(msg) => {
                g.last_outcome = Some(EnrollmentOutcomeView {
                    status: "failed".into(),
                    peer_fingerprint: Some(peer_fp.to_string()),
                    detail: Some("Local trust write failed".into()),
                });
                Err(anyhow::anyhow!("{msg}"))
            }
            Step::Aborted => {
                Self::mark_consumed(&mut g, session_id);
                g.last_outcome = Some(EnrollmentOutcomeView {
                    status: "cancelled".into(),
                    peer_fingerprint: Some(peer_fp.to_string()),
                    detail: Some("Cancelled before durable commit".into()),
                });
                Ok("aborted")
            }
            Step::WaitingFlush => {
                g.last_outcome = Some(EnrollmentOutcomeView {
                    status: "waiting_peer".into(),
                    peer_fingerprint: Some(peer_fp.to_string()),
                    detail: Some("Authorized locally — waiting for peer durable ack".into()),
                });
                Ok("waiting")
            }
        }
    }

    /// After our PersistAck flush: if peer already acked, mark Succeeded and close.
    async fn complete_after_ack_flush(
        mgr: &Arc<RwLock<Inner>>,
        session_id: Uuid,
        peer_fp: &str,
        flushed: bool,
    ) {
        let mut g = mgr.write().await;
        let done = {
            let Some(s) = g.sessions.get_mut(&session_id) else {
                return;
            };
            if !flushed {
                let message = "Authorized locally, but the final acknowledgment could not be sent. Check the other computer or remove authorization and retry.";
                s.error = Some(message.into());
                Self::abort_session_io(s);
                g.last_outcome = Some(EnrollmentOutcomeView {
                    status: "incomplete".into(),
                    peer_fingerprint: Some(peer_fp.to_string()),
                    detail: Some(message.into()),
                });
                return;
            }
            s.local_ack_flushed = true;
            if s.local_persisted && s.peer_persisted_ack {
                s.state = SessionState::Succeeded;
                Self::abort_session_io(s);
                true
            } else {
                false
            }
        };
        if done {
            Self::mark_consumed(&mut g, session_id);
            g.last_outcome = Some(EnrollmentOutcomeView {
                status: "succeeded".into(),
                peer_fingerprint: Some(peer_fp.to_string()),
                detail: Some("Mutual durable authorization acknowledged".into()),
            });
        }
    }

    fn abort_session_io(s: &mut LiveSession) {
        if let Some(io) = s.io.take() {
            let _ = io.cmd_tx.try_send(IoCommand::Shutdown);
            io.abort();
        }
        if let Some(t) = s.event_task.take() {
            t.abort();
        }
    }

    fn is_consumed(g: &Inner, id: Uuid) -> bool {
        g.consumed_sessions.iter().any(|(x, _)| *x == id)
    }

    fn mark_consumed(g: &mut Inner, id: Uuid) {
        g.consumed_sessions.push_back((id, Instant::now()));
        while g.consumed_sessions.len() > MAX_CONSUMED_IDS {
            g.consumed_sessions.pop_front();
        }
    }

    fn spawn_expiry_watcher(mgr: Arc<RwLock<Inner>>, g: &mut Inner) {
        if let Some(t) = g.expiry_task.take() {
            t.abort();
        }
        let deadline = g.window_expires;
        g.expiry_task = Some(tokio::spawn(async move {
            if let Some(exp) = deadline {
                let wait = exp.saturating_duration_since(Instant::now());
                tokio::time::sleep(wait).await;
                let mut g = mgr.write().await;
                Self::expire_locked(&mut g).await;
            }
        }));
    }

    async fn expire_locked(g: &mut Inner) {
        let now = Instant::now();
        // Prune consumed IDs older than TTL.
        while g
            .consumed_sessions
            .front()
            .is_some_and(|(_, t)| now.duration_since(*t) > SESSION_TTL)
        {
            g.consumed_sessions.pop_front();
        }

        if let Some(exp) = g.window_expires {
            if now >= exp {
                let ids: Vec<Uuid> = g.sessions.keys().copied().collect();
                for id in ids {
                    if let Some(s) = g.sessions.get_mut(&id) {
                        if s.commit_gate.is_committed()
                            || matches!(
                                s.state,
                                SessionState::WaitingPeerAuth | SessionState::Succeeded
                            )
                        {
                            continue;
                        }
                        if !s.state.is_terminal() {
                            if matches!(s.state, SessionState::Persisting) {
                                if s.commit_gate.try_cancel() {
                                    s.state = SessionState::Expired;
                                    Self::abort_session_io(s);
                                }
                            } else {
                                let _ = s.commit_gate.try_cancel();
                                s.state = SessionState::Expired;
                                Self::abort_session_io(s);
                            }
                        }
                    }
                }
                Self::close_listener_locked(g).await;
                g.last_outcome = Some(EnrollmentOutcomeView {
                    status: "expired".into(),
                    peer_fingerprint: None,
                    detail: Some("Enrollment expired".into()),
                });
            }
        }
        let ids: Vec<Uuid> = g.sessions.keys().copied().collect();
        for id in ids {
            if let Some(s) = g.sessions.get_mut(&id) {
                if now >= s.expires_at
                    && !s.commit_gate.is_committed()
                    && !s.state.is_terminal()
                    && !matches!(
                        s.state,
                        SessionState::WaitingPeerAuth | SessionState::Succeeded
                    )
                {
                    if matches!(s.state, SessionState::Persisting) {
                        if s.commit_gate.try_cancel() {
                            s.state = SessionState::Expired;
                            Self::abort_session_io(s);
                        }
                    } else {
                        let _ = s.commit_gate.try_cancel();
                        s.state = SessionState::Expired;
                        Self::abort_session_io(s);
                    }
                }
            }
        }
        Self::prune_terminal_sessions(g);
    }

    fn prune_terminal_sessions(g: &mut Inner) {
        let mut terminal: Vec<(Uuid, Instant)> = g
            .sessions
            .iter()
            .filter(|(_, s)| s.state.is_terminal())
            .map(|(id, s)| (*id, s.expires_at))
            .collect();
        if terminal.len() <= MAX_TERMINAL_SESSIONS {
            return;
        }
        terminal.sort_by_key(|(_, t)| *t);
        let remove = terminal.len() - MAX_TERMINAL_SESSIONS;
        for (id, _) in terminal.into_iter().take(remove) {
            g.sessions.remove(&id);
        }
    }

    async fn close_listener_locked(g: &mut Inner) {
        g.listening = false;
        g.window_expires = None;
        g.listener = None;
        if let Some(task) = g.accept_task.take() {
            task.abort();
        }
        if let Some(task) = g.expiry_task.take() {
            task.abort();
        }
        g.listen_addr = None;
        g.mdns.stop().await;
    }

    fn snapshot_locked(g: &Inner) -> EnrollmentSnapshot {
        Self::snapshot_with_candidates(g, g.manual_candidates.clone())
    }

    fn snapshot_with_candidates(
        g: &Inner,
        candidates: Vec<EnrollmentCandidateView>,
    ) -> EnrollmentSnapshot {
        let now = Instant::now();
        let sessions = g
            .sessions
            .values()
            .map(|s| {
                let remaining = s.expires_at.saturating_duration_since(now).as_secs();
                EnrollmentSessionView {
                    session_id: s.session_id,
                    role: match s.role {
                        Role::Initiator => "initiator".into(),
                        Role::Acceptor => "acceptor".into(),
                    },
                    state: s.state.as_str().into(),
                    peer_display_name: s.peer_display_name.clone(),
                    local_fingerprint: s.local_fp.clone(),
                    peer_fingerprint: s.peer_fp.clone(),
                    peer_node_id: s.peer_node_id,
                    dialed_addr: s.dialed_addr.map(|a| a.to_string()),
                    local_confirmed: s.local_confirmed,
                    peer_confirmed: s.peer_confirmed,
                    remaining_secs: remaining,
                    error: s.error.clone(),
                }
            })
            .collect();
        EnrollmentSnapshot {
            listening: g.listening,
            listen_addr: g.listen_addr.map(|a| a.to_string()),
            expires_at: g.window_expires.map(|exp| {
                let dur = exp.saturating_duration_since(Instant::now());
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    + dur;
                Utc.timestamp_opt(secs.as_secs() as i64, 0)
                    .single()
                    .unwrap_or_else(Utc::now)
                    .to_rfc3339()
            }),
            sessions,
            candidates,
            outcome: g.last_outcome.clone(),
        }
    }
}

fn decode_nonce(hex_str: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_str).context("Enrollment mismatch")?;
    if bytes.len() != 32 {
        bail!("Enrollment mismatch");
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;

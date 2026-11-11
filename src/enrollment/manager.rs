//! Enrollment manager: the allowing window and its codes, pairing attempts,
//! durable trust writes and revoke.
//!
//! The PAKE exchange runs inline on each connection under short I/O bounds.
//! Only after both key confirmations verify does a session get a
//! [`session_io::SessionIo`] actor for PersistAck and cancel.

use super::{
    channel::{self, EnrollConn},
    commit_gate::{CommitAbort, CommitGate},
    limits::{
        RateLimits, CODE_TTL, MAX_CONCURRENT_SESSIONS, MAX_PAIRING_ATTEMPTS, MAX_TERMINAL_SESSIONS,
        SESSION_TTL, WINDOW_TTL,
    },
    mdns::EnrollMdns,
    pake::{self, Pake, Side, Transcript},
    protocol::{self, refusal, sanitize_display_name, EnrollMessage, PERSIST_ACK_TIMEOUT_SECS},
    session_io::{IoCommand, SessionEvent, SessionIo},
    tls::EnrollVersion,
};
use crate::{
    auth::{Authenticator, PublicKey, SshAuthenticator},
    config::Config,
    control::{
        EnrollmentCandidateView, EnrollmentOutcomeView, EnrollmentSessionView, EnrollmentSnapshot,
        PairingCode, PairingCodeView, PairingFailure, TrustedDeviceView,
    },
    transport::tls,
};
use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, sync::RwLock, task::JoinHandle};
use uuid::Uuid;

/// Bound waiting for local PersistAck write flush after durable pin.
const IO_ACK_FLUSH_SECS: u64 = 5;
/// How long a "find nearby" browse keeps listening for announcements.
const BROWSE_TTL: Duration = Duration::from_secs(2 * 60);
/// Text an older dialer shows verbatim as its session error.
const LEGACY_REFUSAL: &str =
    "The other computer uses pairing codes. Update ClipSync on this computer, then pair again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Typed the code and dialed.
    Initiator,
    /// Showed the code and accepted.
    Acceptor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Dialing,
    /// PAKE and key confirmation in progress.
    Verifying,
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
            Self::Verifying => "Verifying",
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
    peer_session_id: Option<Uuid>,
    /// From the TLS session, never from a message.
    peer_fp: Option<String>,
    peer_key: Option<PublicKey>,
    peer_node_id: Option<Uuid>,
    peer_display_name: Option<String>,
    dialed_addr: Option<SocketAddr>,
    /// Both key confirmations verified; the only gate for a trust write.
    verified: bool,
    local_persisted: bool,
    local_ack_flushed: bool,
    peer_persisted_ack: bool,
    /// Linearizes cancel vs durable commit; never revoke via best-effort remove.
    commit_gate: Arc<CommitGate>,
    expires_at: Instant,
    failure: Option<PairingFailure>,
    io: Option<SessionIo>,
    event_task: Option<JoinHandle<()>>,
}

impl LiveSession {
    fn new(session_id: Uuid, role: Role, state: SessionState, local_fp: String) -> Self {
        Self {
            session_id,
            role,
            state,
            local_fp,
            peer_session_id: None,
            peer_fp: None,
            peer_key: None,
            peer_node_id: None,
            peer_display_name: None,
            dialed_addr: None,
            verified: false,
            local_persisted: false,
            local_ack_flushed: false,
            peer_persisted_ack: false,
            commit_gate: Arc::new(CommitGate::new()),
            expires_at: Instant::now() + SESSION_TTL,
            failure: None,
            io: None,
            event_task: None,
        }
    }
}

/// The code on screen. Taking it out of the window is the only way to start
/// the allowing side's PAKE.
struct LiveCode {
    code: PairingCode,
    expires_at: Instant,
}

struct Inner {
    config: Arc<Config>,
    auth: Arc<SshAuthenticator>,
    auth_dyn: Arc<dyn Authenticator>,
    listening: bool,
    listen_addr: Option<SocketAddr>,
    listen_port_override: Option<u16>,
    window_ttl_override: Option<Duration>,
    code_ttl_override: Option<Duration>,
    session_ttl_override: Option<Duration>,
    attempts_override: Option<u32>,
    window_expires: Option<Instant>,
    code: Option<LiveCode>,
    /// Codes consumed by connections in this window (the guess budget).
    attempts_used: u32,
    sessions: HashMap<Uuid, LiveSession>,
    /// Manual / injected candidates (merged with mDNS on discover).
    manual_candidates: Vec<EnrollmentCandidateView>,
    rates: RateLimits,
    /// In-flight TLS handshakes (reserved before spawn, released after bind/fail).
    pending_handshakes: usize,
    accept_task: Option<JoinHandle<()>>,
    expiry_task: Option<JoinHandle<()>>,
    listener: Option<Arc<TcpListener>>,
    display_name: String,
    last_outcome: Option<EnrollmentOutcomeView>,
    mdns: EnrollMdns,
    browse_until: Option<Instant>,
    browse_task: Option<JoinHandle<()>>,
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
            if let Some(t) = g.browse_task.take() {
                t.abort();
            }
            g.listening = false;
            g.listener = None;
            g.code = None;
            for s in g.sessions.values_mut() {
                Self::abort_session_io(s);
            }
        }
    }
}

fn outcome(status: &str, peer: Option<String>, detail: &str) -> Option<EnrollmentOutcomeView> {
    Some(EnrollmentOutcomeView {
        status: status.into(),
        peer_fingerprint: peer,
        detail: Some(detail.into()),
    })
}

fn refusal_failure(code: &str) -> PairingFailure {
    match code {
        refusal::NO_CODE => PairingFailure::NoCode,
        refusal::WRONG_CODE => PairingFailure::WrongCode,
        refusal::BUSY => PairingFailure::Busy,
        refusal::RATE_LIMITED => PairingFailure::RateLimited,
        refusal::REFLECTION => PairingFailure::Reflection,
        refusal::CLOSED => PairingFailure::NotListening,
        refusal::UPDATE_REQUIRED => PairingFailure::UpdateRequired,
        _ => PairingFailure::Protocol,
    }
}

/// What the allowing side tells the dialer before closing.
fn refusal_for(failure: PairingFailure) -> (&'static str, &'static str) {
    let code = match failure {
        PairingFailure::NoCode => refusal::NO_CODE,
        PairingFailure::WrongCode => refusal::WRONG_CODE,
        PairingFailure::Busy => refusal::BUSY,
        PairingFailure::RateLimited => refusal::RATE_LIMITED,
        PairingFailure::Reflection => refusal::REFLECTION,
        PairingFailure::NotListening => refusal::CLOSED,
        _ => refusal::PROTOCOL,
    };
    (code, failure.message())
}

struct PeerHello {
    session_id: Uuid,
    display_name: String,
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
                window_ttl_override: None,
                code_ttl_override: None,
                session_ttl_override: None,
                attempts_override: None,
                window_expires: None,
                code: None,
                attempts_used: 0,
                sessions: HashMap::new(),
                manual_candidates: Vec::new(),
                rates: RateLimits::default(),
                pending_handshakes: 0,
                accept_task: None,
                expiry_task: None,
                listener: None,
                display_name,
                last_outcome: None,
                mdns: EnrollMdns::new(our_node_id),
                browse_until: None,
                browse_task: None,
            })),
        })
    }

    /// Tests: bind enrollment listener to ephemeral port on the config IP.
    pub async fn set_listen_port_override(&self, port: u16) {
        self.inner.write().await.listen_port_override = Some(port);
    }

    /// Tests: shorten how long an allowing window stays open.
    pub async fn set_window_ttl_override(&self, ttl: Duration) {
        self.inner.write().await.window_ttl_override = Some(ttl);
    }

    /// Tests: shorten how long each code lives before it is replaced.
    pub async fn set_code_ttl_override(&self, ttl: Duration) {
        self.inner.write().await.code_ttl_override = Some(ttl);
    }

    /// Tests: shorten the per-attempt deadline.
    pub async fn set_session_ttl_override(&self, ttl: Duration) {
        self.inner.write().await.session_ttl_override = Some(ttl);
    }

    /// Tests: change how many attempts (codes) one window accepts.
    pub async fn set_attempts_override(&self, attempts: u32) {
        self.inner.write().await.attempts_override = Some(attempts);
    }

    /// Inject an untrusted discovery hint (tests / manual UI).
    pub async fn inject_candidate(&self, candidate: EnrollmentCandidateView) {
        self.inner.write().await.manual_candidates.push(candidate);
    }

    fn window_ttl(g: &Inner) -> Duration {
        g.window_ttl_override.unwrap_or(WINDOW_TTL)
    }

    fn code_ttl(g: &Inner) -> Duration {
        g.code_ttl_override.unwrap_or(CODE_TTL)
    }

    fn session_ttl(g: &Inner) -> Duration {
        g.session_ttl_override.unwrap_or(SESSION_TTL)
    }

    fn max_attempts(g: &Inner) -> u32 {
        g.attempts_override.unwrap_or(MAX_PAIRING_ATTEMPTS)
    }

    /// Allow one device: listen, advertise, and show a code. Opening again while
    /// open keeps the current window and code.
    pub async fn open(&self) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        if g.listening {
            return Ok(Self::snapshot_locked(&g).await);
        }
        let bind = Self::resolve_listen_addr(&g)?;
        // Fail closed before listening if the OS RNG is unavailable.
        let first = pake::generate_code()?;
        let listener = TcpListener::bind(bind)
            .await
            .context("Enrollment bind failed")?;
        let local = listener.local_addr()?;
        let listener = Arc::new(listener);
        let now = Instant::now();
        let closes = now + Self::window_ttl(&g);
        g.listener = Some(listener.clone());
        g.listen_addr = Some(local);
        g.listening = true;
        g.window_expires = Some(closes);
        g.attempts_used = 0;
        g.code = Some(LiveCode {
            code: first,
            expires_at: (now + Self::code_ttl(&g)).min(closes),
        });
        g.last_outcome = None;

        let local_fp = g.auth.get_public_key().await?.fingerprint();
        let display = g.display_name.clone();
        // Best-effort; loopback fixtures never advertise.
        if let Err(e) = g.mdns.advertise(&display, local, &local_fp).await {
            tracing::debug!("Enrollment mDNS advertise skipped: {e}");
        }

        let mgr = self.inner.clone();
        let auth = g.auth_dyn.clone();
        g.accept_task = Some(tokio::spawn(async move {
            // This loop serves only its own window's listener. A loop that
            // closed its window itself is not aborted, so it must not adopt
            // the listener of a window opened after that.
            let ours = |g: &Inner| {
                g.listening
                    && g.listener
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &listener))
            };
            loop {
                if !ours(&*mgr.read().await) {
                    break;
                }
                let tcp = match listener.accept().await {
                    Ok((tcp, _)) => tcp,
                    Err(_) => break,
                };
                let mgr2 = mgr.clone();
                let auth2 = auth.clone();
                {
                    let mut g = mgr2.write().await;
                    Self::expire_locked(&mut g).await;
                    if !ours(&g) {
                        break;
                    }
                    if !Self::can_start_handshake(&g) || !g.rates.allow_accept(Instant::now()) {
                        continue;
                    }
                    g.pending_handshakes += 1;
                }
                tokio::spawn(async move {
                    // A connection that became a session already released its slot.
                    if !Self::handle_accept(mgr2.clone(), auth2, tcp).await {
                        let mut g = mgr2.write().await;
                        g.pending_handshakes = g.pending_handshakes.saturating_sub(1);
                    }
                });
            }
        }));
        Self::spawn_expiry_watcher(self.inner.clone(), &mut g);
        Ok(Self::snapshot_locked(&g).await)
    }

    /// Look for computers that are allowing a device. Works without opening a
    /// window here; on loopback configurations only injected candidates show.
    pub async fn discover(&self) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        if Self::lan_discovery(&g) {
            match g.mdns.browse().await {
                Ok(()) => {
                    g.browse_until = Some(Instant::now() + BROWSE_TTL);
                    // Stop browsing even if no companion polls again.
                    let mgr = self.inner.clone();
                    if let Some(task) = g.browse_task.replace(tokio::spawn(async move {
                        tokio::time::sleep(BROWSE_TTL).await;
                        Self::expire_locked(&mut *mgr.write().await).await;
                    })) {
                        task.abort();
                    }
                }
                Err(e) => tracing::debug!("Enrollment mDNS browse skipped: {e}"),
            }
        }
        Ok(Self::snapshot_locked(&g).await)
    }

    /// Dial `address` and prove `code`. Returns promptly with the new session;
    /// the exchange continues in the background.
    pub async fn pair(
        &self,
        address: &str,
        code: PairingCode,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    ) -> Result<EnrollmentSnapshot> {
        let addr: SocketAddr = address
            .trim()
            .parse()
            .context("Enrollment address invalid")?;
        let (auth, local_fp, display_name, session_id) = {
            let mut g = self.inner.write().await;
            Self::expire_locked(&mut g).await;
            if !Self::can_start_handshake(&g) {
                bail!("Enrollment busy");
            }
            if !g.rates.allow_accept(Instant::now()) {
                bail!("Enrollment rate limited");
            }
            let local_fp = g.auth.get_public_key().await?.fingerprint();
            let session_id = Uuid::new_v4();
            let mut session = LiveSession::new(
                session_id,
                Role::Initiator,
                SessionState::Dialing,
                local_fp.clone(),
            );
            session.dialed_addr = Some(addr);
            session.expires_at = Instant::now() + Self::session_ttl(&g);
            g.sessions.insert(session_id, session);
            (
                g.auth_dyn.clone(),
                local_fp,
                g.display_name.clone(),
                session_id,
            )
        };

        let mgr = self.inner.clone();
        tokio::spawn(async move {
            let result = Self::run_joining(
                mgr.clone(),
                auth,
                addr,
                session_id,
                code,
                local_fp,
                display_name,
                expected_fingerprint,
                expected_node_id,
            )
            .await;
            if let Err(failure) = result {
                Self::fail_session(&mgr, session_id, failure).await;
            }
        });

        let mut snapshot = self.snapshot().await;
        snapshot.requested_session_id = Some(session_id);
        Ok(snapshot)
    }

    pub async fn inspect(&self, session_id: Option<Uuid>) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        if let Some(id) = session_id {
            if !g.sessions.contains_key(&id) && g.listening {
                bail!("Enrollment session not found");
            }
        }
        Ok(Self::snapshot_locked(&g).await)
    }

    /// Cancel one attempt, or (`None`) every live attempt and stop allowing.
    pub async fn cancel(&self, session_id: Option<Uuid>) -> Result<EnrollmentSnapshot> {
        let mut g = self.inner.write().await;
        Self::expire_locked(&mut g).await;
        let ids: Vec<Uuid> = match session_id {
            Some(id) => vec![id],
            None => g.sessions.keys().copied().collect(),
        };
        for id in ids {
            let Some(s) = g.sessions.get_mut(&id) else {
                continue;
            };
            // Commit won or already authorized: never revoke, never Cancelled.
            if s.state.is_terminal()
                || matches!(s.state, SessionState::WaitingPeerAuth)
                || s.commit_gate.is_committed()
            {
                continue;
            }
            // While Persisting, cancel wins only if the gate is still Open.
            if !s.commit_gate.try_cancel() && matches!(s.state, SessionState::Persisting) {
                continue;
            }
            s.state = SessionState::Cancelled;
            s.failure = Some(PairingFailure::Cancelled);
            if let Some(io) = s.io.take() {
                let _ = io.cmd_tx.try_send(IoCommand::CancelNotify(id));
                io.abort();
            }
            if let Some(t) = s.event_task.take() {
                t.abort();
            }
        }
        if session_id.is_none() {
            let was_listening = g.listening;
            Self::close_window_locked(&mut g).await;
            g.mdns.stop().await;
            g.browse_until = None;
            if let Some(task) = g.browse_task.take() {
                task.abort();
            }
            if was_listening {
                g.last_outcome = outcome("cancelled", None, "Stopped allowing a new device.");
            }
        } else {
            // Cancelling the last attempt in flight may leave the window without codes.
            Self::maintain_window_locked(&mut g).await;
        }
        Ok(Self::snapshot_locked(&g).await)
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
        Self::snapshot_locked(&g).await
    }

    pub async fn is_authorized(&self, key: &PublicKey) -> Result<bool> {
        let auth = self.inner.read().await.auth.clone();
        auth.is_authorized(key).await.map_err(Into::into)
    }

    // --- window and codes ---

    fn lan_discovery(g: &Inner) -> bool {
        g.config
            .socket_addr()
            .is_ok_and(|addr| !addr.ip().is_loopback())
    }

    /// Put a fresh code on screen. It always differs from the one it replaces,
    /// so a person can see that the old one stopped working.
    fn issue_code_locked(g: &mut Inner, previous: Option<&PairingCode>) {
        let now = Instant::now();
        let closes = g.window_expires.unwrap_or(now);
        let fresh = loop {
            match pake::generate_code() {
                Ok(code) if previous == Some(&code) => continue,
                other => break other,
            }
        };
        g.code = match fresh {
            Ok(code) => Some(LiveCode {
                code,
                expires_at: (now + Self::code_ttl(g)).min(closes),
            }),
            Err(e) => {
                tracing::debug!("Enrollment code renewal failed: {e}");
                None
            }
        };
    }

    /// Consume the live code for one connection and put the next one on screen
    /// if the window has attempts left. No two connections ever hold one code.
    fn take_code_locked(g: &mut Inner) -> Result<PairingCode, PairingFailure> {
        if !g.listening {
            return Err(PairingFailure::NotListening);
        }
        let LiveCode { code, .. } = g.code.take().ok_or(PairingFailure::NoCode)?;
        g.attempts_used = g.attempts_used.saturating_add(1);
        if g.attempts_used < Self::max_attempts(g) {
            Self::issue_code_locked(g, Some(&code));
        }
        Ok(code)
    }

    fn allowing_attempt_in_flight(g: &Inner) -> bool {
        g.sessions
            .values()
            .any(|s| s.role == Role::Acceptor && s.state == SessionState::Verifying)
    }

    /// Keep a live code on screen while the window is open, and close the
    /// window once it has run out of time or attempts.
    async fn maintain_window_locked(g: &mut Inner) {
        if !g.listening {
            return;
        }
        let now = Instant::now();
        if g.window_expires.is_some_and(|closes| now >= closes) {
            Self::close_window_locked(g).await;
            g.last_outcome = outcome(
                "expired",
                None,
                "No device paired before this window closed. Choose Allow a new device to try again.",
            );
            return;
        }
        if let Some(expired) = g.code.take_if(|live| now >= live.expires_at) {
            Self::issue_code_locked(g, Some(&expired.code));
        }
        if g.code.is_none() && !Self::allowing_attempt_in_flight(g) {
            Self::close_window_locked(g).await;
            g.last_outcome = outcome(
                "closed",
                None,
                "Every code for this window was used. Choose Allow a new device for new codes.",
            );
        }
    }

    /// Close the allowing window. Attempts still checking a code from it end
    /// too, so nothing pairs after the window has said it closed, and a
    /// successful pairing leaves no sibling attempt running.
    async fn close_window_locked(g: &mut Inner) {
        let ending = if g
            .window_expires
            .is_some_and(|closes| Instant::now() >= closes)
        {
            (SessionState::Expired, PairingFailure::Expired)
        } else {
            (SessionState::Cancelled, PairingFailure::Cancelled)
        };
        for s in g.sessions.values_mut() {
            if s.role == Role::Acceptor
                && s.state == SessionState::Verifying
                && s.commit_gate.try_cancel()
            {
                s.state = ending.0.clone();
                s.failure = Some(ending.1);
                Self::abort_session_io(s);
            }
        }
        g.listening = false;
        g.window_expires = None;
        g.code = None;
        g.listener = None;
        // Both loops also close the window themselves; they exit on `!listening`.
        // Aborting the running task would cancel the mDNS teardown below.
        let current = tokio::task::try_id();
        for task in [g.accept_task.take(), g.expiry_task.take()]
            .into_iter()
            .flatten()
        {
            if Some(task.id()) != current {
                task.abort();
            }
        }
        g.listen_addr = None;
        if g.browse_until.is_none() {
            g.mdns.stop().await;
        } else {
            g.mdns.unadvertise().await;
        }
    }

    // --- allowing side ---

    /// Returns whether the connection became a session (and released its
    /// pending-handshake slot itself).
    async fn handle_accept(
        mgr: Arc<RwLock<Inner>>,
        auth: Arc<dyn Authenticator>,
        tcp: tokio::net::TcpStream,
    ) -> bool {
        let conn = match channel::accept(tcp, auth).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::debug!("Enrollment accept failed: {e}");
                return false;
            }
        };
        if conn.version == EnrollVersion::Legacy {
            {
                let mut g = mgr.write().await;
                if g.listening {
                    g.last_outcome = outcome(
                        "update_required",
                        None,
                        "A computer with an older ClipSync tried to pair. Update ClipSync there, then type the code.",
                    );
                }
            }
            conn.refuse(refusal::UPDATE_REQUIRED, LEGACY_REFUSAL).await;
            return false;
        }
        let peer_key = conn.peer_key.clone();
        let peer_fp = peer_key.fingerprint();
        let peer_node = tls::node_id(&peer_key);

        // Checks that need no code come first; a refused connection costs no attempt.
        let started = {
            let mut g = mgr.write().await;
            Self::expire_locked(&mut g).await;
            match Self::admit_locked(&mut g, &peer_fp).await {
                Err(failure) => Err(failure),
                Ok(local_fp) => match Self::take_code_locked(&mut g) {
                    Err(failure) => Err(failure),
                    Ok(code) => {
                        let session_id = Uuid::new_v4();
                        let mut session = LiveSession::new(
                            session_id,
                            Role::Acceptor,
                            SessionState::Verifying,
                            local_fp.clone(),
                        );
                        session.peer_fp = Some(peer_fp.clone());
                        session.peer_key = Some(peer_key.clone());
                        session.peer_node_id = Some(peer_node);
                        session.dialed_addr = Some(conn.peer_addr);
                        session.expires_at = Instant::now() + Self::session_ttl(&g);
                        g.sessions.insert(session_id, session);
                        g.pending_handshakes = g.pending_handshakes.saturating_sub(1);
                        Ok((session_id, code, local_fp, g.display_name.clone()))
                    }
                },
            }
        };
        let (session_id, code, local_fp, display_name) = match started {
            Ok(started) => started,
            Err(failure) => {
                let (code, message) = refusal_for(failure);
                conn.refuse(code, message).await;
                return false;
            }
        };

        let mut conn = conn;
        let pake = Pake::start(Side::Allowing, code);
        let exchange = Self::allowing_exchange(
            &mut conn,
            pake,
            session_id,
            &display_name,
            &local_fp,
            &peer_fp,
        )
        .await;
        match exchange {
            Ok((peer, confirmation)) => {
                // Commit to this device before our confirmation lets the peer save.
                {
                    let mut g = mgr.write().await;
                    let Some(s) = g.sessions.get_mut(&session_id) else {
                        return true;
                    };
                    if s.state.is_terminal() {
                        return true;
                    }
                    s.peer_session_id = Some(peer.session_id);
                    s.peer_display_name = Some(peer.display_name);
                    s.verified = true;
                    s.state = SessionState::Persisting;
                    // One window, one device: nobody else gets a code after this.
                    Self::close_window_locked(&mut g).await;
                }
                if conn.write_msg(&confirmation).await.is_err() {
                    Self::fail_session(&mgr, session_id, PairingFailure::Disconnected).await;
                    return true;
                }
                Self::start_persisting(mgr, session_id, conn).await;
            }
            Err(failure) => {
                // A wrong code already cost its code; charging it to the key as
                // well would only lock out a person who mistyped.
                if failure == PairingFailure::Protocol {
                    mgr.write()
                        .await
                        .rates
                        .record_failure(&peer_fp, Instant::now());
                }
                if failure == PairingFailure::WrongCode {
                    let (code, message) = refusal_for(failure);
                    conn.refuse(code, message).await;
                }
                Self::fail_session(&mgr, session_id, failure).await;
            }
        }
        true
    }

    /// Capacity, rate and reflection checks before a code is spent.
    async fn admit_locked(g: &mut Inner, peer_fp: &str) -> Result<String, PairingFailure> {
        if !g.listening {
            return Err(PairingFailure::NotListening);
        }
        let active = g.sessions.values().filter(|s| s.state.is_active()).count();
        if active >= MAX_CONCURRENT_SESSIONS {
            return Err(PairingFailure::Busy);
        }
        if !g.rates.allow_failure_budget(peer_fp, Instant::now()) {
            return Err(PairingFailure::RateLimited);
        }
        let local_fp = g
            .auth
            .get_public_key()
            .await
            .map_err(|_| PairingFailure::Protocol)?
            .fingerprint();
        if peer_fp == local_fp {
            g.rates.record_failure(peer_fp, Instant::now());
            return Err(PairingFailure::Reflection);
        }
        Ok(local_fp)
    }

    /// Runs the allowing side up to verifying the peer's confirmation. Returns
    /// our own confirmation unsent: the caller commits to the pairing first.
    async fn allowing_exchange(
        conn: &mut EnrollConn,
        pake: Pake,
        session_id: Uuid,
        display_name: &str,
        local_fp: &str,
        peer_fp: &str,
    ) -> Result<(PeerHello, EnrollMessage), PairingFailure> {
        let protocol = |_| PairingFailure::Protocol;
        let dropped = |_| PairingFailure::Disconnected;
        let EnrollMessage::Hello {
            session_id: their_id,
            nonce_hex,
            display_name: their_name,
            pake_hex,
        } = conn.read_msg().await.map_err(dropped)?
        else {
            return Err(PairingFailure::Protocol);
        };
        let their_nonce = decode_nonce(&nonce_hex).map_err(protocol)?;
        let their_name = sanitize_display_name(&their_name).map_err(protocol)?;
        let their_msg = hex::decode(&pake_hex).map_err(|_| PairingFailure::Protocol)?;
        let our_msg = pake.message().to_vec();
        let key = pake.finish(&their_msg).map_err(protocol)?;
        let nonce = protocol::random_nonce();
        conn.write_msg(&EnrollMessage::HelloAck {
            session_id,
            peer_session_id: their_id,
            nonce_hex: hex::encode(nonce),
            display_name: display_name.into(),
            pake_hex: hex::encode(&our_msg),
        })
        .await
        .map_err(dropped)?;
        let EnrollMessage::KeyConfirm {
            session_id: sid,
            peer_session_id,
            mac_hex,
        } = conn.read_msg().await.map_err(dropped)?
        else {
            return Err(PairingFailure::Protocol);
        };
        if sid != their_id || peer_session_id != session_id {
            return Err(PairingFailure::Protocol);
        }
        let binding = conn.binding;
        let transcript = Transcript {
            allowing_session: session_id,
            joining_session: their_id,
            allowing_nonce: &nonce,
            joining_nonce: &their_nonce,
            allowing_fingerprint: local_fp,
            joining_fingerprint: peer_fp,
            allowing_message: &our_msg,
            joining_message: &their_msg,
            channel_binding: &binding,
        };
        let tag = hex::decode(&mac_hex).unwrap_or_default();
        if !key.verify_peer(&transcript, &tag) {
            return Err(PairingFailure::WrongCode);
        }
        let ours = key.confirmation(&transcript).map_err(protocol)?;
        Ok((
            PeerHello {
                session_id: their_id,
                display_name: their_name,
            },
            EnrollMessage::KeyConfirm {
                session_id,
                peer_session_id: their_id,
                mac_hex: hex::encode(ours),
            },
        ))
    }

    // --- joining side ---

    #[allow(clippy::too_many_arguments)]
    async fn run_joining(
        mgr: Arc<RwLock<Inner>>,
        auth: Arc<dyn Authenticator>,
        addr: SocketAddr,
        session_id: Uuid,
        code: PairingCode,
        local_fp: String,
        display_name: String,
        expected_fingerprint: Option<String>,
        expected_node_id: Option<Uuid>,
    ) -> Result<(), PairingFailure> {
        if mgr
            .read()
            .await
            .sessions
            .get(&session_id)
            .is_none_or(|s| s.state.is_terminal())
        {
            return Err(PairingFailure::Cancelled);
        }
        let mut conn = channel::dial(addr, auth).await?;
        if conn.version == EnrollVersion::Legacy {
            return Err(PairingFailure::UpdateRequired);
        }
        let peer_key = conn.peer_key.clone();
        let peer_fp = peer_key.fingerprint();
        let peer_node = tls::node_id(&peer_key);
        {
            let mut g = mgr.write().await;
            let now = Instant::now();
            if !g.rates.allow_failure_budget(&peer_fp, now) {
                return Err(PairingFailure::RateLimited);
            }
            let bound = Self::bind_checks(
                &g,
                session_id,
                &local_fp,
                &peer_fp,
                peer_node,
                expected_fingerprint.as_deref(),
                expected_node_id,
            );
            if let Err(failure) = bound {
                g.rates.record_failure(&peer_fp, now);
                return Err(failure);
            }
            let Some(s) = g.sessions.get_mut(&session_id) else {
                return Err(PairingFailure::Cancelled);
            };
            if s.state.is_terminal() {
                return Err(PairingFailure::Cancelled);
            }
            s.state = SessionState::Verifying;
            s.peer_fp = Some(peer_fp.clone());
            s.peer_key = Some(peer_key);
            s.peer_node_id = Some(peer_node);
        }
        let exchange = Self::joining_exchange(
            &mut conn,
            Pake::start(Side::Joining, code),
            session_id,
            &display_name,
            &local_fp,
            &peer_fp,
        )
        .await;
        let peer = match exchange {
            Ok(peer) => peer,
            Err(failure) => {
                if failure == PairingFailure::Protocol {
                    mgr.write()
                        .await
                        .rates
                        .record_failure(&peer_fp, Instant::now());
                }
                return Err(failure);
            }
        };
        {
            let mut g = mgr.write().await;
            let Some(s) = g.sessions.get_mut(&session_id) else {
                return Err(PairingFailure::Cancelled);
            };
            if s.state.is_terminal() {
                return Err(PairingFailure::Cancelled);
            }
            s.peer_session_id = Some(peer.session_id);
            s.peer_display_name = Some(peer.display_name);
            s.verified = true;
            s.state = SessionState::Persisting;
        }
        Self::start_persisting(mgr, session_id, conn).await;
        Ok(())
    }

    async fn joining_exchange(
        conn: &mut EnrollConn,
        pake: Pake,
        session_id: Uuid,
        display_name: &str,
        local_fp: &str,
        peer_fp: &str,
    ) -> Result<PeerHello, PairingFailure> {
        let protocol = |_| PairingFailure::Protocol;
        let dropped = |_| PairingFailure::Disconnected;
        let nonce = protocol::random_nonce();
        let our_msg = pake.message().to_vec();
        conn.write_msg(&EnrollMessage::Hello {
            session_id,
            nonce_hex: hex::encode(nonce),
            display_name: display_name.into(),
            pake_hex: hex::encode(&our_msg),
        })
        .await
        .map_err(dropped)?;
        let (their_id, their_nonce, their_name, their_msg) =
            match conn.read_msg().await.map_err(dropped)? {
                EnrollMessage::HelloAck {
                    session_id: their_id,
                    peer_session_id,
                    nonce_hex,
                    display_name,
                    pake_hex,
                } if peer_session_id == session_id => (
                    their_id,
                    decode_nonce(&nonce_hex).map_err(protocol)?,
                    sanitize_display_name(&display_name).map_err(protocol)?,
                    hex::decode(&pake_hex).map_err(|_| PairingFailure::Protocol)?,
                ),
                EnrollMessage::Error { code, .. } => return Err(refusal_failure(&code)),
                _ => return Err(PairingFailure::Protocol),
            };
        let key = pake.finish(&their_msg).map_err(protocol)?;
        let binding = conn.binding;
        let transcript = Transcript {
            allowing_session: their_id,
            joining_session: session_id,
            allowing_nonce: &their_nonce,
            joining_nonce: &nonce,
            allowing_fingerprint: peer_fp,
            joining_fingerprint: local_fp,
            allowing_message: &their_msg,
            joining_message: &our_msg,
            channel_binding: &binding,
        };
        let ours = key.confirmation(&transcript).map_err(protocol)?;
        conn.write_msg(&EnrollMessage::KeyConfirm {
            session_id,
            peer_session_id: their_id,
            mac_hex: hex::encode(ours),
        })
        .await
        .map_err(dropped)?;
        match conn.read_msg().await.map_err(dropped)? {
            EnrollMessage::KeyConfirm {
                session_id: sid,
                peer_session_id,
                mac_hex,
            } if sid == their_id && peer_session_id == session_id => {
                let tag = hex::decode(&mac_hex).unwrap_or_default();
                if !key.verify_peer(&transcript, &tag) {
                    return Err(PairingFailure::WrongCode);
                }
            }
            EnrollMessage::Error { code, .. } => return Err(refusal_failure(&code)),
            _ => return Err(PairingFailure::Protocol),
        }
        Ok(PeerHello {
            session_id: their_id,
            display_name: their_name,
        })
    }

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

    /// Discovery hints are untrusted, but a device picked from the nearby list
    /// must be the one that answers.
    fn bind_checks(
        g: &Inner,
        session_id: Uuid,
        local_fp: &str,
        peer_fp: &str,
        peer_node: Uuid,
        expected_fp: Option<&str>,
        expected_node: Option<Uuid>,
    ) -> Result<(), PairingFailure> {
        if peer_fp == local_fp {
            return Err(PairingFailure::Reflection);
        }
        if expected_fp.is_some_and(|exp| exp != peer_fp)
            || expected_node.is_some_and(|exp| exp != peer_node)
        {
            return Err(PairingFailure::Mismatch);
        }
        let dialed = g.sessions.get(&session_id).and_then(|s| s.dialed_addr);
        if let Some(addr) = dialed {
            for c in g
                .manual_candidates
                .iter()
                .filter(|c| c.address == addr.to_string())
            {
                if c.advertised_fingerprint
                    .as_deref()
                    .is_some_and(|fp| fp != peer_fp)
                    || c.advertised_node_id.is_some_and(|id| id != peer_node)
                {
                    return Err(PairingFailure::Mismatch);
                }
            }
        }
        Ok(())
    }

    // --- shared: durable save and PersistAck ---

    async fn fail_session(mgr: &Arc<RwLock<Inner>>, session_id: Uuid, failure: PairingFailure) {
        let mut g = mgr.write().await;
        let mut acceptor = false;
        if let Some(s) = g.sessions.get_mut(&session_id) {
            acceptor = s.role == Role::Acceptor;
            if !s.state.is_terminal()
                && !matches!(s.state, SessionState::WaitingPeerAuth)
                && !s.commit_gate.is_committed()
            {
                let _ = s.commit_gate.try_cancel();
                s.state = SessionState::Failed;
                s.failure = Some(failure);
                Self::abort_session_io(s);
            }
        }
        if acceptor && failure == PairingFailure::WrongCode && g.listening {
            g.last_outcome = outcome(
                "wrong_code",
                None,
                "Someone typed a code that did not match. That code is used up; a new one is shown.",
            );
        }
        Self::maintain_window_locked(&mut g).await;
    }

    async fn start_persisting(mgr: Arc<RwLock<Inner>>, session_id: Uuid, conn: EnrollConn) {
        if Self::attach_session_io(mgr.clone(), session_id, conn)
            .await
            .is_ok()
        {
            tokio::spawn(async move {
                let _ = Self::persist_mutual_static(mgr, session_id).await;
            });
        }
    }

    async fn attach_session_io(
        mgr: Arc<RwLock<Inner>>,
        session_id: Uuid,
        conn: EnrollConn,
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
                        // Persist owns the outcome while saving. Once our pin and ack
                        // are out, a peer that leaves without acking saved nothing.
                        let mut g = mgr2.write().await;
                        let mut one_sided = None;
                        if let Some(s) = g.sessions.get_mut(&session_id) {
                            if !matches!(s.state, SessionState::Persisting) {
                                if matches!(s.state, SessionState::WaitingPeerAuth)
                                    && s.local_ack_flushed
                                    && !s.peer_persisted_ack
                                {
                                    s.failure = Some(PairingFailure::Incomplete);
                                    one_sided = Some(s.peer_fp.clone());
                                }
                                Self::abort_session_io(s);
                            }
                        }
                        if let Some(peer) = one_sided {
                            g.last_outcome =
                                outcome("incomplete", peer, PairingFailure::Incomplete.message());
                        }
                        break;
                    }
                }
            }
        });
        let mut g = mgr.write().await;
        if let Some(s) = g.sessions.get_mut(&session_id) {
            s.event_task = Some(event_task);
        }
        Ok(())
    }

    async fn on_session_message(mgr: Arc<RwLock<Inner>>, session_id: Uuid, msg: EnrollMessage) {
        match msg {
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
                if let Some(s) = g.sessions.get_mut(&session_id) {
                    let committed = s.commit_gate.is_committed()
                        || matches!(
                            s.state,
                            SessionState::WaitingPeerAuth | SessionState::Succeeded
                        );
                    if !committed && !s.state.is_terminal() && s.commit_gate.try_cancel() {
                        s.state = SessionState::Cancelled;
                        s.failure = Some(PairingFailure::Cancelled);
                        Self::abort_session_io(s);
                    }
                }
            }
            EnrollMessage::Error { code, .. } => {
                Self::fail_session(&mgr, session_id, refusal_failure(&code)).await;
            }
            _ => {}
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
            if peer_session_id != session_id || Some(their_sid) != s.peer_session_id {
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
            g.last_outcome = outcome(
                "succeeded",
                peer_fp,
                "Paired. Both computers saved the pairing.",
            );
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
                s.failure = Some(PairingFailure::Cancelled);
                Self::abort_session_io(s);
                bail!("Enrollment cancelled");
            }
            if Instant::now() >= s.expires_at {
                let _ = s.commit_gate.try_cancel();
                s.state = SessionState::Expired;
                s.failure = Some(PairingFailure::Expired);
                Self::abort_session_io(s);
                bail!("Enrollment expired");
            }
            if !s.verified {
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
                let Some(s) = g.sessions.get_mut(&session_id) else {
                    return;
                };
                if matches!(s.state, SessionState::WaitingPeerAuth) && !s.peer_persisted_ack {
                    let fp = s.peer_fp.clone();
                    s.failure = Some(PairingFailure::Incomplete);
                    Self::abort_session_io(s);
                    g.last_outcome =
                        outcome("incomplete", fp, PairingFailure::Incomplete.message());
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
            PublishedButUncertain,
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
                    tracing::debug!("Enrollment save not confirmed: {e}");
                    s.local_persisted = true;
                    s.state = SessionState::WaitingPeerAuth;
                    s.failure = Some(PairingFailure::Incomplete);
                    Self::abort_session_io(s);
                    Step::PublishedButUncertain
                }
                Err(e) => {
                    let aborted = matches!(e, crate::auth::AuthError::CommitAborted(_))
                        || gate.is_cancelled();
                    if aborted {
                        if !s.state.is_terminal() {
                            let expired = Instant::now() >= s.expires_at;
                            s.state = if expired {
                                SessionState::Expired
                            } else {
                                SessionState::Cancelled
                            };
                            s.failure = Some(if expired {
                                PairingFailure::Expired
                            } else {
                                PairingFailure::Cancelled
                            });
                            Self::abort_session_io(s);
                        }
                        Step::Aborted
                    } else {
                        if !s.state.is_terminal() {
                            s.state = SessionState::Failed;
                            s.failure = Some(PairingFailure::SaveFailed);
                            Self::abort_session_io(s);
                        }
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
        let peer = Some(peer_fp.to_string());
        match step {
            Step::PublishedButUncertain => {
                g.last_outcome = outcome("incomplete", peer, PairingFailure::Incomplete.message());
                Ok("aborted")
            }
            Step::Fail(msg) => {
                g.last_outcome = outcome("failed", peer, PairingFailure::SaveFailed.message());
                Err(anyhow::anyhow!("{msg}"))
            }
            Step::Aborted => {
                g.last_outcome = outcome("cancelled", peer, "Cancelled before anything was saved.");
                Ok("aborted")
            }
            Step::WaitingFlush => {
                g.last_outcome = outcome(
                    "waiting_peer",
                    peer,
                    "Saved here; waiting for the other computer to save.",
                );
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
                s.failure = Some(PairingFailure::Incomplete);
                Self::abort_session_io(s);
                g.last_outcome = outcome(
                    "incomplete",
                    Some(peer_fp.to_string()),
                    PairingFailure::Incomplete.message(),
                );
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
            g.last_outcome = outcome(
                "succeeded",
                Some(peer_fp.to_string()),
                "Paired. Both computers saved the pairing.",
            );
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

    fn spawn_expiry_watcher(mgr: Arc<RwLock<Inner>>, g: &mut Inner) {
        if let Some(t) = g.expiry_task.take() {
            t.abort();
        }
        g.expiry_task = Some(tokio::spawn(async move {
            loop {
                let wake = {
                    let g = mgr.read().await;
                    if !g.listening {
                        break;
                    }
                    let now = Instant::now();
                    [
                        g.window_expires,
                        g.code.as_ref().map(|live| live.expires_at),
                    ]
                    .into_iter()
                    .flatten()
                    .min()
                    .unwrap_or(now + Duration::from_secs(1))
                    .max(now + Duration::from_millis(20))
                };
                tokio::time::sleep_until(wake.into()).await;
                let mut g = mgr.write().await;
                Self::expire_locked(&mut g).await;
                if !g.listening {
                    break;
                }
            }
        }));
    }

    async fn expire_locked(g: &mut Inner) {
        let now = Instant::now();
        for s in g.sessions.values_mut() {
            if now < s.expires_at
                || s.commit_gate.is_committed()
                || s.state.is_terminal()
                || matches!(s.state, SessionState::WaitingPeerAuth)
            {
                continue;
            }
            // While Persisting, expiry wins only if the gate is still Open.
            if s.commit_gate.try_cancel() || !matches!(s.state, SessionState::Persisting) {
                s.state = SessionState::Expired;
                s.failure = Some(PairingFailure::Expired);
                Self::abort_session_io(s);
            }
        }
        Self::maintain_window_locked(g).await;
        if !g.listening && g.browse_until.is_some_and(|until| now >= until) {
            g.browse_until = None;
            g.mdns.stop().await;
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

    /// Addresses a person can type on the other computer. A wildcard bind is
    /// shown as this computer's LAN addresses.
    fn listen_hints(listen: SocketAddr) -> Vec<String> {
        if !listen.ip().is_unspecified() {
            return vec![listen.to_string()];
        }
        if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .filter(|iface| !iface.is_loopback() && iface.ip().is_ipv4() == listen.is_ipv4())
            .map(|iface| SocketAddr::new(iface.ip(), listen.port()).to_string())
            .take(3)
            .collect()
    }

    async fn snapshot_locked(g: &Inner) -> EnrollmentSnapshot {
        let mut candidates = if g.mdns.is_browsing().await {
            g.mdns.snapshot_candidates().await
        } else {
            Vec::new()
        };
        for c in &g.manual_candidates {
            if !candidates.iter().any(|m| m.address == c.address) {
                candidates.push(c.clone());
            }
        }
        let now = Instant::now();
        let sessions = g
            .sessions
            .values()
            .map(|s| EnrollmentSessionView {
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
                remaining_secs: s.expires_at.saturating_duration_since(now).as_secs(),
                error: s.failure.map(|f| f.message().to_string()),
                failure: s.failure.map(|f| f.as_str().to_string()),
            })
            .collect();
        let code = g
            .code
            .as_ref()
            .filter(|_| g.listening)
            .map(|live| PairingCodeView {
                code: live.code.clone(),
                remaining_secs: live
                    .expires_at
                    .saturating_duration_since(now)
                    .as_secs_f64()
                    .ceil() as u64,
                attempts_left: Self::max_attempts(g).saturating_sub(g.attempts_used),
            });
        EnrollmentSnapshot {
            requested_session_id: None,
            listening: g.listening,
            listen_addr: g.listen_addr.map(|a| a.to_string()),
            listen_hints: g.listen_addr.map(Self::listen_hints).unwrap_or_default(),
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
            code,
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

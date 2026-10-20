//! Loopback TLS enrollment harness (D03). Temp keys only — no personal clipboard,
//! no installed service, no production pasteboard.
mod support;
use clipsync::{
    auth::{Authenticator, AuthorizedKeys, KeyPair, KeyType, SshAuthenticator},
    config::Config,
    control::{
        self, Command, EnrollmentCandidateView, EnrollmentTarget, Target, FEATURE_ENROLLMENT,
    },
    enrollment::EnrollmentManager,
    transport::{websocket::WebSocketConfig, Listener, WebSocketTransport},
};
use std::{sync::Arc, time::Duration};
use support::*;

async fn managers_pair() -> (
    Identity,
    Identity,
    Arc<EnrollmentManager>,
    Arc<EnrollmentManager>,
    std::net::SocketAddr,
    std::net::SocketAddr,
) {
    let a = Identity::new().await;
    let b = Identity::new().await;

    async fn mgr(id: &Identity) -> Arc<EnrollmentManager> {
        let mut config = Config::default();
        config.node_id = id.id();
        config.listen_addr = "127.0.0.1:0".into();
        config.auth.ssh_key = id.dir.path().join("identity");
        config.auth.authorized_keys = id.dir.path().join("authorized_keys");
        config.clipboard.history_db = id.dir.path().join("history.db");
        config.advertise_name = format!("enroll-{}", &id.id().to_string()[..8]);
        let manager = Arc::new(EnrollmentManager::new(Arc::new(config), id.auth.clone()).unwrap());
        manager.set_listen_port_override(0).await;
        manager
    }

    let ma = mgr(&a).await;
    let mb = mgr(&b).await;
    let sa = ma.open().await.unwrap();
    let sb = mb.open().await.unwrap();
    let addr_a: std::net::SocketAddr = sa.listen_addr.as_ref().unwrap().parse().unwrap();
    let addr_b: std::net::SocketAddr = sb.listen_addr.as_ref().unwrap().parse().unwrap();
    (a, b, ma, mb, addr_a, addr_b)
}

/// Dial returns promptly (IPC-safe); poll until this session binds the peer.
async fn wait_bound_session(
    mgr: &EnrollmentManager,
    session_id: uuid::Uuid,
    peer_fp: &str,
) -> uuid::Uuid {
    for _ in 0..200 {
        let snap = mgr.inspect(Some(session_id)).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| s.session_id == session_id) {
            if s.peer_fingerprint.as_deref() == Some(peer_fp) {
                return session_id;
            }
            if s.state == "Failed" || s.state == "Cancelled" || s.state == "Expired" {
                panic!(
                    "session {session_id} ended as {} before bind: {:?}",
                    s.state, s.error
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {session_id} did not bind peer {peer_fp}");
}

async fn wait_bound(mgr: &EnrollmentManager, peer_fp: &str) -> uuid::Uuid {
    for _ in 0..200 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap
            .sessions
            .iter()
            .find(|s| s.peer_fingerprint.as_deref() == Some(peer_fp))
        {
            return s.session_id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session did not bind peer {peer_fp}");
}

async fn wait_session_failed(mgr: &EnrollmentManager, session_id: uuid::Uuid) -> String {
    for _ in 0..200 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| s.session_id == session_id) {
            if s.state == "Failed" {
                return s.error.clone().unwrap_or_else(|| "Failed".into());
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {session_id} did not fail");
}

fn dialing_session_id(snap: &clipsync::control::EnrollmentSnapshot) -> uuid::Uuid {
    snap.sessions
        .iter()
        .find(|s| s.state == "Dialing")
        .map(|s| s.session_id)
        .expect("dial snapshot should include Dialing session")
}

async fn trusted_fps(auth: &SshAuthenticator) -> Vec<String> {
    let mut fps: Vec<String> = auth
        .list_trusted_devices()
        .await
        .unwrap()
        .into_iter()
        .map(|(fp, _)| fp)
        .collect();
    fps.sort();
    fps
}

/// Live confirm window only. Cancelled/Failed rows keep the peer fingerprint.
async fn wait_awaiting(mgr: &EnrollmentManager, peer_fp: &str) -> uuid::Uuid {
    for _ in 0..200 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| {
            s.state == "AwaitingLocalConfirm" && s.peer_fingerprint.as_deref() == Some(peer_fp)
        }) {
            return s.session_id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no AwaitingLocalConfirm session for {peer_fp}");
}

async fn wait_authorized(auth: &clipsync::auth::SshAuthenticator, key: &clipsync::auth::PublicKey) {
    for _ in 0..200 {
        if auth.is_authorized(key).await.unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("peer was not authorized in time");
}

#[tokio::test]
async fn neither_confirm_writes_no_trust() {
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let _ = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let _ = wait_bound(&mb, &a.key.public_key().fingerprint()).await;
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn local_only_confirm_does_not_persist() {
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let _ = wait_bound(&mb, &a.key.public_key().fingerprint()).await;
    ma.confirm(sid).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !a.auth.is_authorized(&b.key.public_key()).await.unwrap(),
        "local-only confirm must not write trust"
    );
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn mutual_confirm_writes_both_pins() {
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let sid_b = wait_bound(&mb, &a.key.public_key().fingerprint()).await;

    let ia = ma.inspect(Some(sid_a)).await.unwrap();
    let ib = mb.inspect(Some(sid_b)).await.unwrap();
    let sa = ia.sessions.iter().find(|s| s.session_id == sid_a).unwrap();
    let sb = ib.sessions.iter().find(|s| s.session_id == sid_b).unwrap();
    assert!(sa.peer_fingerprint.as_ref().unwrap().starts_with("SHA256:"));
    assert!(sb.peer_fingerprint.as_ref().unwrap().starts_with("SHA256:"));
    assert_eq!(
        sa.peer_fingerprint.as_ref().unwrap(),
        &b.key.public_key().fingerprint()
    );
    assert_eq!(
        sb.peer_fingerprint.as_ref().unwrap(),
        &a.key.public_key().fingerprint()
    );

    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;

    // Poll until mutual PersistAck or honest waiting/incomplete — never false "succeeded" early.
    let mut saw_honest = false;
    for _ in 0..100 {
        let oa = ma.inspect(None).await.unwrap();
        if let Some(o) = &oa.outcome {
            assert_ne!(o.status, "succeeded_without_ack");
            if matches!(
                o.status.as_str(),
                "waiting_peer" | "succeeded" | "incomplete"
            ) {
                saw_honest = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(saw_honest, "expected honest mutual outcome");
}

#[tokio::test]
async fn cancel_prevents_trust_write() {
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    ma.cancel(Some(sid_a)).await.unwrap();
    let mut sid_b = None;
    for s in mb.inspect(None).await.unwrap().sessions {
        if s.peer_fingerprint.as_deref() == Some(&a.key.public_key().fingerprint()) {
            sid_b = Some(s.session_id);
        }
    }
    if let Some(sid_b) = sid_b {
        let _ = mb.confirm(sid_b).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn reflection_rejected() {
    let a = Identity::new().await;
    let mut config = Config::default();
    config.node_id = a.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = a.dir.path().join("identity");
    config.auth.authorized_keys = a.dir.path().join("authorized_keys");
    config.clipboard.history_db = a.dir.path().join("history.db");
    let ma = Arc::new(EnrollmentManager::new(Arc::new(config), a.auth.clone()).unwrap());
    ma.set_listen_port_override(0).await;
    let snap = ma.open().await.unwrap();
    let addr = snap.listen_addr.unwrap();
    let snap = ma.dial(&addr, None, None).await.unwrap();
    let sid = dialing_session_id(&snap);
    let err = wait_session_failed(&ma, sid).await;
    assert!(
        err.contains("reflection")
            || err.contains("mismatch")
            || err.contains("failed")
            || err.contains("Reflection")
            || err.to_lowercase().contains("reflection"),
        "{err}"
    );
    assert!(
        AuthorizedKeys::load_from_file(&a.dir.path().join("authorized_keys"))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn discovery_fingerprint_mismatch_rejects() {
    let (a, b, ma, _mb, _aa, addr_b) = managers_pair().await;
    let attacker_fp = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    ma.inject_candidate(EnrollmentCandidateView {
        display_name: "spoof".into(),
        address: addr_b.to_string(),
        advertised_fingerprint: Some(attacker_fp.into()),
        advertised_node_id: Some(b.id()),
        source: "mdns".into(),
    })
    .await;
    let snap = ma
        .dial(&addr_b.to_string(), Some(attacker_fp.into()), Some(b.id()))
        .await
        .unwrap();
    let sid = dialing_session_id(&snap);
    let err = wait_session_failed(&ma, sid).await;
    assert!(
        err.contains("mismatch") || err.contains("Mismatch"),
        "{err}"
    );
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn oversize_frame_rejected_no_trust() {
    let (a, b, ma, _mb, _aa, addr_b) = managers_pair().await;
    assert_eq!(clipsync::enrollment::ENROLL_MAX_FRAME_BYTES, 64 * 1024);
    let _ = ma.dial(&addr_b.to_string(), None, None).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    ma.cancel(None).await.unwrap();
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn revoke_preserves_unrelated_pins() {
    let (a, b, ma, _mb, _aa, _ab) = managers_pair().await;
    let keep = Identity::new().await;
    a.auth
        .add_trusted_peer(&keep.key.public_key().to_openssh(), Some("keep".into()))
        .await
        .unwrap();
    a.auth
        .add_trusted_peer(&b.key.public_key().to_openssh(), Some("drop".into()))
        .await
        .unwrap();
    let devices = ma.revoke(&b.key.public_key().fingerprint()).await.unwrap();
    assert!(devices
        .iter()
        .any(|d| d.fingerprint == keep.key.public_key().fingerprint()));
    assert!(!devices
        .iter()
        .any(|d| d.fingerprint == b.key.public_key().fingerprint()));
    assert!(a.auth.is_authorized(&keep.key.public_key()).await.unwrap());
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn enrollment_feature_only_with_wrapper_target() {
    #[derive(Default)]
    struct Stub;
    #[async_trait::async_trait]
    impl Target for Stub {
        async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
            Vec::new()
        }
        async fn sync(&self) -> anyhow::Result<usize> {
            Ok(0)
        }
        async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
            Ok(0)
        }
    }
    let id = Identity::new().await;
    let mut config = Config::default();
    config.auth.authorized_keys = id.dir.path().join("authorized_keys");
    config.clipboard.history_db = id.dir.path().join("history.db");
    config.listen_addr = "127.0.0.1:0".into();
    let manager =
        Arc::new(EnrollmentManager::new(Arc::new(config.clone()), id.auth.clone()).unwrap());
    manager.set_listen_port_override(0).await;

    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let target = Arc::new(EnrollmentTarget::new(Arc::new(Stub), manager.clone()));
    let task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));

    let status = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    assert!(status
        .features
        .as_ref()
        .unwrap()
        .iter()
        .any(|f| f == FEATURE_ENROLLMENT));

    let open = control::request(&path, Command::EnrollmentOpen, max_frame)
        .await
        .unwrap();
    assert!(open.enrollment.as_ref().unwrap().listening);

    drop(task);
}

#[tokio::test]
async fn inspect_closed_returns_snapshot_not_error() {
    let (a, _b, ma, _mb, _aa, _ab) = managers_pair().await;
    ma.cancel(None).await.unwrap();
    let snap = ma.inspect(None).await.expect("closed inspect must succeed");
    assert!(!snap.listening);
    assert!(
        snap.outcome
            .as_ref()
            .map(|o| o.status == "cancelled")
            .unwrap_or(false)
            || snap.sessions.iter().any(|s| s.state == "Cancelled")
            || !a
                .auth
                .is_authorized(&KeyPair::generate(KeyType::Ed25519).unwrap().public_key())
                .await
                .unwrap()
    );
}

#[tokio::test]
async fn confirm_and_read_do_not_deadlock() {
    // Regression: duplex Mutex across reader blocked confirm writes.
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let sid_b = wait_bound(&mb, &a.key.public_key().fingerprint()).await;
    let confirm = tokio::time::timeout(Duration::from_secs(3), async {
        ma.confirm(sid_a).await.unwrap();
        mb.confirm(sid_b).await.unwrap();
    })
    .await;
    assert!(
        confirm.is_ok(),
        "confirm timed out — duplex mutex deadlock?"
    );
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
}

#[tokio::test]
async fn terminal_sessions_do_not_block_capacity() {
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    // Create three failed dials that remain as terminal history.
    for _ in 0..3 {
        let snap = ma
            .dial(&addr_b.to_string(), Some("SHA256:dead".into()), None)
            .await
            .unwrap();
        let sid = dialing_session_id(&snap);
        let _ = wait_session_failed(&ma, sid).await;
    }
    // Active capacity should still allow a good dial (terminal not counted).
    let snap = ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid = dialing_session_id(&snap);
    let sid = wait_bound_session(&ma, sid, &b.key.public_key().fingerprint()).await;
    let _ = wait_bound(&mb, &a.key.public_key().fingerprint()).await;
    assert!(!sid.is_nil());
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn malicious_peer_display_name_cannot_inject_auth_line() {
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    // Acceptor advertises evil name via its config advertise_name.
    // Manager sanitizes on Hello; inject via confirm path comment uses sanitize.
    let evil = Identity::new().await;
    let evil_line = format!(
        "x\nssh-ed25519 {}",
        evil.key
            .public_key()
            .to_openssh()
            .split_whitespace()
            .nth(1)
            .unwrap()
    );
    let err = a
        .auth
        .add_trusted_peer(&b.key.public_key().to_openssh(), Some(evil_line))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("CR/LF") || err.to_string().contains("comment"));
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!a.auth.is_authorized(&evil.key.public_key()).await.unwrap());

    // Also ensure enrollment path sanitizes peer names (no panic / no inject).
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let _ = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let snap = mb.inspect(None).await.unwrap();
    for s in snap.sessions {
        if let Some(name) = &s.peer_display_name {
            assert!(!name.contains('\n'));
            assert!(!name.contains('\r'));
        }
    }
}

#[tokio::test]
async fn cross_process_auth_instances_cohere_on_remove() {
    let dir = tempfile::TempDir::new().unwrap();
    let key_path = dir.path().join("id_ed25519");
    let auth_path = dir.path().join("authorized_keys");
    let cfg = clipsync::auth::AuthConfig {
        private_key_path: key_path.clone(),
        authorized_keys_path: auth_path.clone(),
        generate_if_missing: true,
    };
    let writer = clipsync::auth::SshAuthenticator::new(cfg.clone())
        .await
        .unwrap();
    let reader = clipsync::auth::SshAuthenticator::new(clipsync::auth::AuthConfig {
        private_key_path: key_path,
        authorized_keys_path: auth_path,
        generate_if_missing: false,
    })
    .await
    .unwrap();
    let peer = KeyPair::generate(KeyType::Ed25519).unwrap().public_key();
    writer
        .add_trusted_peer(&peer.to_openssh(), Some("p".into()))
        .await
        .unwrap();
    assert!(reader.is_authorized(&peer).await.unwrap());
    writer.remove_peer(&peer.fingerprint()).await.unwrap();
    assert!(
        !reader.is_authorized(&peer).await.unwrap(),
        "daemon must see CLI revoke without restart"
    );
    assert!(reader.list_trusted_devices().await.unwrap().is_empty());
}

#[tokio::test]
async fn control_enrollment_dial_returns_under_ipc_budget() {
    #[derive(Default)]
    struct Stub;
    #[async_trait::async_trait]
    impl Target for Stub {
        async fn peers(&self) -> Vec<clipsync::adapters::Peer> {
            Vec::new()
        }
        async fn sync(&self) -> anyhow::Result<usize> {
            Ok(0)
        }
        async fn copy_text(&self, _: String) -> anyhow::Result<usize> {
            Ok(0)
        }
    }
    let (a, _b, ma, _mb, _aa, addr_b) = managers_pair().await;
    let mut config = Config::default();
    config.node_id = a.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = a.dir.path().join("identity");
    config.auth.authorized_keys = a.dir.path().join("authorized_keys");
    config.clipboard.history_db = a.dir.path().join("history.db");
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let target = Arc::new(EnrollmentTarget::new(Arc::new(Stub), ma.clone()));
    let task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));

    let started = std::time::Instant::now();
    let resp = control::request_result(
        &path,
        Command::EnrollmentDial {
            address: addr_b.to_string(),
            expected_fingerprint: None,
            expected_node_id: None,
        },
        max_frame,
    )
    .await
    .expect("dial IPC");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "EnrollmentDial must return under server IPC timeout"
    );
    assert!(resp.enrollment.as_ref().unwrap().listening);

    // Cancel must not leave a stuck live session forever.
    control::request(
        &path,
        Command::EnrollmentCancel { session_id: None },
        max_frame,
    )
    .await
    .unwrap();
    let snap = ma.inspect(None).await.unwrap();
    assert!(!snap.listening);
    drop(task);
}

#[tokio::test]
async fn mutual_confirm_both_sides_reach_succeeded() {
    // Root review C: both peers must observe Succeeded (PersistAck flushed), not only pins.
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let sid_b = wait_bound(&mb, &a.key.public_key().fingerprint()).await;
    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;

    let mut a_ok = false;
    let mut b_ok = false;
    for _ in 0..200 {
        let ia = ma.inspect(None).await.unwrap();
        let ib = mb.inspect(None).await.unwrap();
        a_ok = ia
            .sessions
            .iter()
            .any(|s| s.session_id == sid_a && s.state == "Succeeded")
            || ia.outcome.as_ref().is_some_and(|o| o.status == "succeeded");
        b_ok = ib
            .sessions
            .iter()
            .any(|s| s.session_id == sid_b && s.state == "Succeeded")
            || ib.outcome.as_ref().is_some_and(|o| o.status == "succeeded");
        if a_ok && b_ok {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        a_ok,
        "initiator must reach Succeeded after mutual PersistAck"
    );
    assert!(
        b_ok,
        "acceptor must reach Succeeded after mutual PersistAck"
    );
}

#[tokio::test]
async fn cancel_before_commit_writes_nothing_preexisting_survives() {
    // Root review B: cancel-first => no write; preexisting pin must never be removed.
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    a.auth
        .add_trusted_peer(&b.key.public_key().to_openssh(), Some("preexisting".into()))
        .await
        .unwrap();
    assert!(a.auth.is_authorized(&b.key.public_key()).await.unwrap());

    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let _sid_b = wait_bound(&mb, &a.key.public_key().fingerprint()).await;

    // Cancel before either Confirm — no re-enrollment write path.
    ma.cancel(Some(sid_a)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        a.auth.is_authorized(&b.key.public_key()).await.unwrap(),
        "preexisting pin must survive cancel"
    );
    let snap = ma.inspect(Some(sid_a)).await.unwrap();
    let s = snap.sessions.iter().find(|s| s.session_id == sid_a);
    assert!(
        s.map(|s| s.state == "Cancelled").unwrap_or(true),
        "cancel-first must end Cancelled without commit"
    );
}

#[tokio::test]
async fn commit_before_cancel_keeps_authorization_honest() {
    // Root review B: commit-first => pin stays; cancel must not revoke / delete.
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let sid_b = wait_bound(&mb, &a.key.public_key().fingerprint()).await;
    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;

    // Late cancel after durable commit must not strip trust.
    let _ = ma.cancel(Some(sid_a)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        a.auth.is_authorized(&b.key.public_key()).await.unwrap(),
        "committed pin must survive late cancel"
    );
    assert!(b.auth.is_authorized(&a.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn long_idle_within_injected_ttl_still_confirms() {
    // Root review E: idle > IO_TIMEOUT must not kill verify window; use short TTL injection.
    let (a, b, ma, mb, _aa, _addr_b) = managers_pair().await;
    // Wider than typical partial-frame budget yet far under 5 minutes.
    ma.set_ttl_override(Duration::from_secs(3)).await;
    mb.set_ttl_override(Duration::from_secs(3)).await;
    // Re-open so window uses override.
    let _ = ma.cancel(None).await;
    let _ = mb.cancel(None).await;
    ma.open().await.unwrap();
    let sb = mb.open().await.unwrap();
    let addr_b: std::net::SocketAddr = sb.listen_addr.unwrap().parse().unwrap();

    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    let sid_b = wait_bound(&mb, &a.key.public_key().fingerprint()).await;

    // Idle past the historic 15s-as-idle bug scale via injected short window:
    // sleep a meaningful fraction of TTL without sending frames.
    tokio::time::sleep(Duration::from_millis(400)).await;
    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
}

#[tokio::test]
async fn injected_ttl_expiry_closes_without_five_minute_sleep() {
    let (a, b, ma, mb, _aa, _addr_b) = managers_pair().await;
    ma.set_ttl_override(Duration::from_millis(250)).await;
    mb.set_ttl_override(Duration::from_millis(250)).await;
    let _ = ma.cancel(None).await;
    let _ = mb.cancel(None).await;
    ma.open().await.unwrap();
    let sb = mb.open().await.unwrap();
    let addr_b: std::net::SocketAddr = sb.listen_addr.unwrap().parse().unwrap();

    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let snap = ma.inspect(None).await.unwrap();
    let expired = !snap.listening
        || snap
            .sessions
            .iter()
            .any(|s| s.session_id == sid_a && s.state == "Expired")
        || snap.outcome.as_ref().is_some_and(|o| o.status == "expired");
    assert!(expired, "short TTL must expire without 5min wall sleep");
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn loopback_open_does_not_start_mdns_daemon() {
    // Root review D: 127.0.0.1 fixtures must not advertise/browse LAN.
    let (_a, _b, ma, _mb, addr_a, _addr_b) = managers_pair().await;
    assert!(addr_a.ip().is_loopback());
    let snap = ma.discover().await.unwrap();
    assert!(
        snap.candidates
            .iter()
            .all(|c| c.source != "mdns" || c.address.contains("127.")),
        "loopback open must not populate LAN mDNS candidates"
    );
    // Manual inject remains the fixture discovery path.
    ma.inject_candidate(EnrollmentCandidateView {
        display_name: "manual".into(),
        address: addr_a.to_string(),
        advertised_fingerprint: None,
        advertised_node_id: None,
        source: "manual".into(),
    })
    .await;
    let snap = ma.discover().await.unwrap();
    assert!(snap.candidates.iter().any(|c| c.source == "manual"));
}

#[tokio::test]
async fn pause_applied_false_on_nonpaused_copy_and_control_error_preserves_code() {
    use clipsync::{
        adapters::{ClipboardProviderWrapper, HistoryManager},
        sync::SyncEngine,
        transport::{TransportConfig, TransportManager},
    };
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.node_id = identity.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    let clipboard = Clipboard::default();
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig::default(),
        identity.auth.clone(),
        identity.id(),
    ));
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &identity.dir.path().join("history.db"),
            &identity.dir.path().join("history.key"),
        )
        .await
        .unwrap(),
    );
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        transport,
    ));
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let task = Task(tokio::spawn(
        async move { server.run(engine.as_ref()).await },
    ));

    let copy = control::request(
        &path,
        Command::Copy {
            text: "nonpaused".into(),
        },
        max_frame,
    )
    .await
    .unwrap();
    assert_eq!(copy.pause_applied, Some(false));

    control::request(&path, Command::SetPause { paused: true }, max_frame)
        .await
        .unwrap();
    let err = control::request_result(&path, Command::Sync, max_frame)
        .await
        .unwrap_err();
    assert_eq!(
        err.code.as_deref(),
        Some(clipsync::control::error_codes::SYNC_PAUSED)
    );

    let list = control::request(&path, Command::HistoryList { limit: 5 }, max_frame)
        .await
        .unwrap();
    if let Some(entry) = list.history.as_ref().and_then(|h| h.entries.first()) {
        let got = control::request(&path, Command::HistoryGet { id: entry.id }, max_frame)
            .await
            .unwrap();
        assert!(got.instance_id.is_some());
        assert!(got.state_generation.is_some());
    } else {
        let missing = control::request_result(
            &path,
            Command::HistoryGet {
                id: uuid::Uuid::new_v4(),
            },
            max_frame,
        )
        .await
        .unwrap_err();
        assert_eq!(
            missing.code.as_deref(),
            Some(clipsync::control::error_codes::HISTORY_NOT_FOUND)
        );
    }
    drop(task);
}

#[tokio::test]
async fn confirm_replay_after_cancel_writes_nothing() {
    // cancel(Some) consumes the id and leaves the listener open, so confirm
    // is "Enrollment replay" (control code enrollment_replay), not "closed".
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    let keep = Identity::new().await;
    a.auth
        .add_trusted_peer(
            &keep.key.public_key().to_openssh(),
            Some("preexisting".into()),
        )
        .await
        .unwrap();
    let keep_fp = keep.key.public_key().fingerprint();
    let a_fp = a.key.public_key().fingerprint();
    let b_fp = b.key.public_key().fingerprint();

    ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = wait_bound(&ma, &b_fp).await;
    let sid_b = wait_bound(&mb, &a_fp).await;
    ma.cancel(Some(sid_a)).await.unwrap();

    let replay = ma.confirm(sid_a).await.unwrap_err();
    assert!(
        replay.to_string().starts_with("Enrollment replay"),
        "{replay}"
    );
    let snap = ma.inspect(Some(sid_a)).await.unwrap();
    assert_eq!(
        snap.sessions
            .iter()
            .find(|s| s.session_id == sid_a)
            .map(|s| s.state.as_str()),
        Some("Cancelled")
    );
    // One-sided confirm on the peer must not publish either pin.
    let _ = mb.confirm(sid_b).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(trusted_fps(&a.auth).await, vec![keep_fp.clone()]);
    assert!(trusted_fps(&b.auth).await.is_empty());
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());

    // Later fresh session still reaches mutual trust; the cancelled id does not.
    mb.cancel(Some(sid_b)).await.unwrap();
    let snap = ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a2 = dialing_session_id(&snap);
    let sid_a2 = wait_bound_session(&ma, sid_a2, &b_fp).await;
    let sid_b2 = wait_awaiting(&mb, &a_fp).await;
    ma.confirm(sid_a2).await.unwrap();
    mb.confirm(sid_b2).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;

    let again = ma.confirm(sid_a).await.unwrap_err();
    assert!(
        again.to_string().starts_with("Enrollment replay"),
        "{again}"
    );
    let mut expect_a = vec![keep_fp, b_fp];
    expect_a.sort();
    assert_eq!(trusted_fps(&a.auth).await, expect_a);
    assert_eq!(trusted_fps(&b.auth).await, vec![a_fp]);
}

#[tokio::test]
async fn fourth_live_session_rejected_without_trust() {
    assert_eq!(clipsync::enrollment::MAX_CONCURRENT_SESSIONS, 3);
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    let keep = Identity::new().await;
    a.auth
        .add_trusted_peer(
            &keep.key.public_key().to_openssh(),
            Some("preexisting".into()),
        )
        .await
        .unwrap();
    let keep_fp = keep.key.public_key().fingerprint();
    let a_fp = a.key.public_key().fingerprint();
    let b_fp = b.key.public_key().fingerprint();

    for _ in 0..3 {
        let snap = ma.dial(&addr_b.to_string(), None, None).await.unwrap();
        let sid = dialing_session_id(&snap);
        let sid = wait_bound_session(&ma, sid, &b_fp).await;
        let state = ma
            .inspect(Some(sid))
            .await
            .unwrap()
            .sessions
            .into_iter()
            .find(|s| s.session_id == sid)
            .unwrap()
            .state;
        assert_eq!(state, "AwaitingLocalConfirm");
    }
    for _ in 0..200 {
        let n = mb
            .inspect(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .filter(|s| s.state == "AwaitingLocalConfirm")
            .count();
        if n == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        mb.inspect(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .filter(|s| s.state == "AwaitingLocalConfirm")
            .count(),
        3
    );

    let before_a = ma.inspect(None).await.unwrap().sessions.len();
    let before_b = mb.inspect(None).await.unwrap().sessions.len();
    let busy = ma.dial(&addr_b.to_string(), None, None).await.unwrap_err();
    assert!(busy.to_string().starts_with("Enrollment busy"), "{busy}");
    assert_eq!(ma.inspect(None).await.unwrap().sessions.len(), before_a);
    assert_eq!(mb.inspect(None).await.unwrap().sessions.len(), before_b);
    assert_eq!(trusted_fps(&a.auth).await, vec![keep_fp.clone()]);
    assert!(trusted_fps(&b.auth).await.is_empty());
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());

    // Free both caps without closing the listeners, then a new pair can commit.
    for mgr in [&ma, &mb] {
        let ids: Vec<_> = mgr
            .inspect(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .filter(|s| {
                !matches!(
                    s.state.as_str(),
                    "Failed" | "Cancelled" | "Expired" | "Succeeded"
                )
            })
            .map(|s| s.session_id)
            .collect();
        for id in ids {
            mgr.cancel(Some(id)).await.unwrap();
        }
    }
    let snap = ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = dialing_session_id(&snap);
    let sid_a = wait_bound_session(&ma, sid_a, &b_fp).await;
    let sid_b = wait_awaiting(&mb, &a_fp).await;
    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
    let mut expect_a = vec![keep_fp, b_fp];
    expect_a.sort();
    assert_eq!(trusted_fps(&a.auth).await, expect_a);
    assert_eq!(trusted_fps(&b.auth).await, vec![a_fp]);
}

#[tokio::test]
async fn dial_sync_listener_does_not_authorize() {
    // Production sync listener offers clipsync/2. The dialer is pre-pinned there
    // so an unknown-key rejection cannot stand in for the ALPN mismatch.
    assert_eq!(clipsync::enrollment::ENROLL_ALPN, b"clipsync-enroll/1");
    let (a, b, ma, mb, _aa, addr_b) = managers_pair().await;
    let keep = Identity::new().await;
    a.auth
        .add_trusted_peer(
            &keep.key.public_key().to_openssh(),
            Some("preexisting".into()),
        )
        .await
        .unwrap();
    let sync = Identity::new().await;
    sync.trust(&a).await;
    let keep_fp = keep.key.public_key().fingerprint();
    let a_fp = a.key.public_key().fingerprint();
    let b_fp = b.key.public_key().fingerprint();
    let sync_fp = sync.key.public_key().fingerprint();
    assert_eq!(trusted_fps(&sync.auth).await, vec![a_fp.clone()]);
    assert_eq!(trusted_fps(&a.auth).await, vec![keep_fp.clone()]);

    let listener_auth: Arc<dyn Authenticator> = sync.auth.clone();
    let transport = WebSocketTransport::new(
        "127.0.0.1:0".parse().unwrap(),
        listener_auth,
        WebSocketConfig::default(),
        sync.id(),
    );
    let listener = transport.listen().await.expect("clipsync/2 listener");
    let sync_addr = listener.local_addr();
    let _accept_loop = Task(tokio::spawn(async move {
        let mut listener = listener;
        loop {
            let _ = listener.accept().await;
        }
    }));
    tokio::task::yield_now().await;

    let snap = ma.dial(&sync_addr.to_string(), None, None).await.unwrap();
    let sid = dialing_session_id(&snap);
    let err = wait_session_failed(&ma, sid).await;
    assert!(
        err.contains("NoApplicationProtocol")
            || err.contains("Missing enrollment protocol negotiation"),
        "{err}"
    );
    assert_eq!(trusted_fps(&a.auth).await, vec![keep_fp.clone()]);
    assert!(trusted_fps(&b.auth).await.is_empty());
    assert_eq!(trusted_fps(&sync.auth).await, vec![a_fp.clone()]);
    assert!(!a.auth.is_authorized(&sync.key.public_key()).await.unwrap());
    assert!(sync.auth.is_authorized(&a.key.public_key()).await.unwrap());
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());

    let snap = ma.dial(&addr_b.to_string(), None, None).await.unwrap();
    let sid_a = dialing_session_id(&snap);
    let sid_a = wait_bound_session(&ma, sid_a, &b_fp).await;
    let sid_b = wait_awaiting(&mb, &a_fp).await;
    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
    let mut expect_a = vec![keep_fp, b_fp];
    expect_a.sort();
    assert_eq!(trusted_fps(&a.auth).await, expect_a);
    assert!(!expect_a.iter().any(|fp| fp == &sync_fp));
    assert_eq!(trusted_fps(&b.auth).await, vec![a_fp.clone()]);
    assert_eq!(trusted_fps(&sync.auth).await, vec![a_fp]);
    assert!(!a.auth.is_authorized(&sync.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn repeated_peer_mismatch_exhausts_its_budget_without_blocking_another_peer() {
    let (a, b, ma, _mb, _aa, addr_b) = managers_pair().await;
    for _ in 0..5 {
        let response = ma
            .dial(&addr_b.to_string(), Some("SHA256:wrong".into()), None)
            .await
            .unwrap();
        let id = response.requested_session_id.unwrap();
        assert!(wait_session_failed(&ma, id).await.contains("mismatch"));
    }
    let response = ma
        .dial(&addr_b.to_string(), Some("SHA256:wrong".into()), None)
        .await
        .unwrap();
    let error = wait_session_failed(&ma, response.requested_session_id.unwrap()).await;
    assert!(error.contains("rate limited"), "{error}");
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());

    let (c, _d, _mc, _md, addr_c, _addr_d) = managers_pair().await;
    let response = ma.dial(&addr_c.to_string(), None, None).await.unwrap();
    wait_bound_session(
        &ma,
        response.requested_session_id.unwrap(),
        &c.key.public_key().fingerprint(),
    )
    .await;
    assert!(!a.auth.is_authorized(&c.key.public_key()).await.unwrap());
}

#[tokio::test]
async fn reverse_dial_distinguishes_identical_display_names_by_tls_key() {
    async fn named(id: &Identity) -> Arc<EnrollmentManager> {
        let mut config = Config::default();
        config.node_id = id.id();
        config.listen_addr = "127.0.0.1:0".into();
        config.auth.ssh_key = id.dir.path().join("identity");
        config.auth.authorized_keys = id.dir.path().join("authorized_keys");
        config.clipboard.history_db = id.dir.path().join("history.db");
        config.advertise_name = "same-desktop".into();
        let manager = Arc::new(EnrollmentManager::new(Arc::new(config), id.auth.clone()).unwrap());
        manager.set_listen_port_override(0).await;
        manager
    }
    let a = Identity::new().await;
    let b = Identity::new().await;
    let ma = named(&a).await;
    let mb = named(&b).await;
    let addr_a = ma.open().await.unwrap().listen_addr.unwrap();
    mb.open().await.unwrap();
    let dial = mb.dial(&addr_a, None, None).await.unwrap();
    let sid_b = wait_bound_session(
        &mb,
        dial.requested_session_id.unwrap(),
        &a.key.public_key().fingerprint(),
    )
    .await;
    let sid_a = wait_bound(&ma, &b.key.public_key().fingerprint()).await;
    for (manager, id, expected) in [
        (&ma, sid_a, b.key.public_key().fingerprint()),
        (&mb, sid_b, a.key.public_key().fingerprint()),
    ] {
        let snapshot = manager.inspect(Some(id)).await.unwrap();
        let session = snapshot
            .sessions
            .iter()
            .find(|s| s.session_id == id)
            .unwrap();
        assert_eq!(session.peer_display_name.as_deref(), Some("same-desktop"));
        assert_eq!(session.peer_fingerprint.as_deref(), Some(expected.as_str()));
        assert_ne!(
            session.peer_fingerprint.as_deref(),
            Some(session.local_fingerprint.as_str())
        );
    }
    ma.confirm(sid_a).await.unwrap();
    mb.confirm(sid_b).await.unwrap();
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
}

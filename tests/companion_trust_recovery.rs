//! Pairing with a code, then a new `clipsync/2` sync that uses the pins just
//! written. Revoke on that live sync stops clipboard delivery and reconnect. A
//! replacement authenticator and enrollment manager see the revoked pin gone
//! and an unrelated pin kept. Pairings saved before code pairing existed keep
//! connecting.
//!
//! Sync stays on the existing pin-checked TLS WebSocket path and the
//! authorized-devices format is unchanged. Clipboard fixtures are in-memory
//! (`tests/support`); nothing touches the OS pasteboard.

mod support;

use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    auth::{AuthConfig, Authenticator, SshAuthenticator},
    config::Config,
    enrollment::EnrollmentManager,
    sync::SyncEngine,
    transport::{Listener, TransportConfig, TransportManager},
};
use std::{
    net::SocketAddr,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FROM_PEER: &str = "hello from enrolled peer";
const FROM_HOST: &str = "hello from enrolled host";
const AFTER_PEER: &str = "after revoke from peer";
const AFTER_HOST: &str = "after revoke from host";

async fn enrollment_pair() -> (
    Identity,
    Identity,
    Arc<EnrollmentManager>,
    Arc<EnrollmentManager>,
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
    (a, b, ma, mb)
}

/// B allows a device and A types B's code. Returns both session ids.
async fn pair(ma: &EnrollmentManager, mb: &EnrollmentManager) -> (uuid::Uuid, uuid::Uuid) {
    let open = mb.open().await.unwrap();
    let code = open.code.unwrap().code;
    let sid_a = ma
        .pair(&open.listen_addr.unwrap(), code, None, None)
        .await
        .unwrap()
        .requested_session_id
        .unwrap();
    let sid_b = wait_acceptor(mb).await;
    (sid_a, sid_b)
}

async fn wait_acceptor(mgr: &EnrollmentManager) -> uuid::Uuid {
    for _ in 0..400 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(session) = snap.sessions.iter().find(|s| s.role == "acceptor") {
            return session.session_id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no pairing attempt reached the allowing side");
}

async fn wait_authorized(auth: &SshAuthenticator, key: &clipsync::auth::PublicKey) {
    for _ in 0..200 {
        if auth.is_authorized(key).await.unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("peer was not authorized in time");
}

async fn wait_succeeded(mgr: &EnrollmentManager, session_id: uuid::Uuid) {
    for _ in 0..200 {
        let snap = mgr.inspect(None).await.unwrap();
        let session_ok = snap
            .sessions
            .iter()
            .any(|s| s.session_id == session_id && s.state == "Succeeded");
        let outcome_ok = snap
            .outcome
            .as_ref()
            .is_some_and(|o| o.status == "succeeded");
        if session_ok || outcome_ok {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {session_id} did not reach Succeeded");
}

fn pin_file(id: &Identity) -> std::path::PathBuf {
    id.dir.path().join("authorized_keys")
}

async fn pin_text(id: &Identity) -> String {
    match tokio::fs::read_to_string(pin_file(id)).await {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => panic!("read pins: {err}"),
    }
}

fn assert_mode_600(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{}", path.display());
}

async fn sync_side(
    id: &Identity,
    clipboard: &Clipboard,
) -> (Arc<SyncEngine>, Arc<TransportManager>) {
    let mut config = Config::default();
    config.node_id = id.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = id.dir.path().join("identity");
    config.auth.authorized_keys = id.dir.path().join("authorized_keys");
    config.clipboard.history_db = id.dir.path().join("history.db");
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig {
            connect_timeout: Duration::from_secs(2),
            keepalive_interval: Duration::from_millis(100),
            ..Default::default()
        },
        id.auth.clone(),
        id.id(),
    ));
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &id.dir.path().join("history.db"),
            &id.dir.path().join("history.key"),
        )
        .await
        .unwrap(),
    );
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        transport.clone(),
    ));
    (engine, transport)
}

fn spawn_proxy(upstream: SocketAddr) -> (SocketAddr, Task<()>, Arc<tokio::sync::Mutex<Vec<u8>>>) {
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    let proxy = tokio::net::TcpListener::from_std(proxy).unwrap();
    let captured = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let bytes = captured.clone();
    let task = Task(tokio::spawn(async move {
        let (mut incoming, _) = proxy.accept().await.unwrap();
        let mut outgoing = tokio::net::TcpStream::connect(upstream).await.unwrap();
        let (mut ir, mut iw) = incoming.split();
        let (mut or, mut ow) = outgoing.split();
        let forward = async {
            let mut buf = [0u8; 4096];
            loop {
                let n = match ir.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                bytes.lock().await.extend_from_slice(&buf[..n]);
                if ow.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        };
        let reverse = async {
            let _ = tokio::io::copy(&mut or, &mut iw).await;
        };
        tokio::join!(forward, reverse);
    }));
    (proxy_addr, task, captured)
}

#[tokio::test]
async fn enrolled_pins_open_sync_revoke_stops_clipboard_and_survives_reload() {
    let (a, b, ma, mb) = enrollment_pair().await;
    let keep = Identity::new().await;

    // Sync listeners exist before pairing. Empty pin files must fail closed.
    let ca = Clipboard::default();
    let cb = Clipboard::default();
    let (ea, ta) = sync_side(&a, &ca).await;
    let (eb, tb) = sync_side(&b, &cb).await;
    let listener_a = ea.bind_listener().await.unwrap();
    let listener_b = eb.bind_listener().await.unwrap();
    let sync_addr = listener_b.local_addr();
    let run_a = ea.clone();
    let run_b = eb.clone();
    let _serve_a = Task(tokio::spawn(async move { run_a.run(listener_a).await }));
    let _serve_b = Task(tokio::spawn(async move { run_b.run(listener_b).await }));
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Leave both clipboards empty so the 250ms capture loop has nothing new to send.
    let denied = tokio::time::timeout(Duration::from_secs(5), ta.connect_peer(&b.peer(sync_addr)))
        .await
        .expect("pre-pairing sync dial timed out");
    assert!(
        denied.is_err(),
        "clipsync/2 before pairing must fail closed: {denied:?}"
    );
    assert!(!tb.is_connected(a.id()).await);
    assert!(ea.force_sync().await.is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*cb.text.lock().await, "");
    assert_eq!(cb.writes.load(Ordering::SeqCst), 0);
    assert!(!pin_text(&a).await.contains("ssh-ed25519"));
    assert!(!pin_text(&b).await.contains("ssh-ed25519"));

    let (sid_a, sid_b) = pair(&ma, &mb).await;
    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
    wait_succeeded(&ma, sid_a).await;
    wait_succeeded(&mb, sid_b).await;

    let enrolled_a = pin_text(&a).await;
    let enrolled_b = pin_text(&b).await;
    assert!(enrolled_a.contains(&b.key.public_key().to_openssh()));
    assert!(enrolled_a.contains(&b.key.public_key().fingerprint()));
    assert!(enrolled_b.contains(&a.key.public_key().to_openssh()));
    assert!(enrolled_b.contains(&a.key.public_key().fingerprint()));
    assert_mode_600(&pin_file(&a));
    assert_mode_600(&pin_file(&b));

    // Unrelated pin is a persisted neighbor, not a second enrollment.
    b.auth
        .add_trusted_peer(
            &keep.key.public_key().to_openssh(),
            Some("keep-unrelated".into()),
        )
        .await
        .unwrap();
    let with_keep = pin_text(&b).await;
    assert!(with_keep.contains(&keep.key.public_key().to_openssh()));
    assert!(with_keep.contains("keep-unrelated"));
    assert!(with_keep.contains(&a.key.public_key().to_openssh()));

    // New sync dial, after the pin files exist. Bytes are captured in front of B.
    let (proxy_addr, _proxy, captured) = spawn_proxy(sync_addr);
    let connected =
        tokio::time::timeout(Duration::from_secs(5), ta.connect_peer(&b.peer(proxy_addr)))
            .await
            .expect("post-enroll sync dial timed out");
    assert!(
        connected.is_ok(),
        "persisted pins must open clipsync/2: {connected:?}"
    );
    eventually(async || tb.is_connected(a.id()).await && ta.is_connected(b.id()).await).await;

    ca.copy(FROM_PEER).await;
    assert_eq!(ea.force_sync().await.unwrap(), 1);
    eventually(async || *cb.text.lock().await == FROM_PEER).await;
    cb.copy(FROM_HOST).await;
    assert_eq!(eb.force_sync().await.unwrap(), 1);
    eventually(async || *ca.text.lock().await == FROM_HOST).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(ca.writes.load(Ordering::SeqCst), 1);
    assert_eq!(cb.writes.load(Ordering::SeqCst), 1);

    let wire = captured.lock().await.clone();
    assert!(wire.len() > 64, "sync produced no bytes on the wire");
    assert_eq!(
        wire[0], 0x16,
        "first sync byte must be a TLS handshake record"
    );
    assert_eq!(wire[1], 0x03, "sync record version must be TLS");
    assert!(!wire
        .windows(FROM_PEER.len())
        .any(|w| w == FROM_PEER.as_bytes()));
    assert!(!wire
        .windows(FROM_HOST.len())
        .any(|w| w == FROM_HOST.as_bytes()));
    assert!(!wire.windows(13).any(|w| w == b"ClipboardData"));
    assert!(!wire.windows(4).any(|w| w == b"GET "));
    drop(wire);

    let plain = tokio::time::timeout(
        Duration::from_secs(2),
        tokio_tungstenite::connect_async(format!("ws://{sync_addr}/clipsync")),
    )
    .await;
    assert!(
        plain.is_err() || plain.unwrap().is_err(),
        "sync listener must reject plaintext WebSocket"
    );

    // Revoke on the live listener. Same authenticator the sync socket rechecks.
    let devices = mb.revoke(&a.key.public_key().fingerprint()).await.unwrap();
    assert!(devices
        .iter()
        .any(|d| d.fingerprint == keep.key.public_key().fingerprint()));
    assert!(!devices
        .iter()
        .any(|d| d.fingerprint == a.key.public_key().fingerprint()));
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());
    assert!(b.auth.is_authorized(&keep.key.public_key()).await.unwrap());
    // One-sided revoke does not write the other computer's pin file.
    assert!(pin_text(&a)
        .await
        .contains(&b.key.public_key().to_openssh()));

    eventually(async || !ta.is_connected(b.id()).await && !tb.is_connected(a.id()).await).await;

    let writes_a = ca.writes.load(Ordering::SeqCst);
    let writes_b = cb.writes.load(Ordering::SeqCst);
    ca.copy(AFTER_PEER).await;
    assert!(
        ea.force_sync().await.is_err(),
        "revoked session must not queue another clipboard sync"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(*cb.text.lock().await, FROM_HOST);
    assert_eq!(cb.writes.load(Ordering::SeqCst), writes_b);

    cb.copy(AFTER_HOST).await;
    assert!(eb.force_sync().await.is_err());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(*ca.text.lock().await, AFTER_PEER);
    assert_eq!(ca.writes.load(Ordering::SeqCst), writes_a);
    assert_ne!(*ca.text.lock().await, AFTER_HOST);

    let reconnect =
        tokio::time::timeout(Duration::from_secs(5), ta.connect_peer(&b.peer(sync_addr)))
            .await
            .expect("reconnect dial timed out");
    assert!(
        reconnect.is_err(),
        "revoked peer must not reconnect to the listener: {reconnect:?}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!ta.is_connected(b.id()).await);
    assert!(!tb.is_connected(a.id()).await);
    assert_eq!(*cb.text.lock().await, AFTER_HOST);
    assert_eq!(cb.writes.load(Ordering::SeqCst), writes_b);

    let revoked_file = pin_text(&b).await;
    assert!(!revoked_file.contains(&a.key.public_key().to_openssh()));
    assert!(revoked_file.contains(&keep.key.public_key().to_openssh()));
    assert!(revoked_file.contains("keep-unrelated"));
    assert_mode_600(&pin_file(&b));

    drop(ma);
    drop(mb);
    let mut restarted_config = Config::default();
    restarted_config.node_id = b.id();
    restarted_config.listen_addr = "127.0.0.1:0".into();
    restarted_config.auth.ssh_key = b.dir.path().join("identity");
    restarted_config.auth.authorized_keys = pin_file(&b);
    restarted_config.clipboard.history_db = b.dir.path().join("history-restart.db");
    let restarted_auth = Arc::new(
        SshAuthenticator::new(AuthConfig {
            private_key_path: restarted_config.auth.ssh_key.clone(),
            authorized_keys_path: restarted_config.auth.authorized_keys.clone(),
            generate_if_missing: false,
        })
        .await
        .unwrap(),
    );
    assert_eq!(
        restarted_auth.get_public_key().await.unwrap().fingerprint(),
        b.key.public_key().fingerprint()
    );
    assert!(!restarted_auth
        .is_authorized(&a.key.public_key())
        .await
        .unwrap());
    assert!(restarted_auth
        .is_authorized(&keep.key.public_key())
        .await
        .unwrap());
    let restarted =
        EnrollmentManager::new(Arc::new(restarted_config), restarted_auth.clone()).unwrap();
    let reloaded = restarted.trusted_devices().await.unwrap();
    assert!(reloaded
        .iter()
        .any(|d| d.fingerprint == keep.key.public_key().fingerprint()));
    assert!(!reloaded
        .iter()
        .any(|d| d.fingerprint == a.key.public_key().fingerprint()));
    assert_eq!(reloaded.len(), 1);

    ea.shutdown().await;
    eb.shutdown().await;
}

#[tokio::test]
async fn failed_trust_write_preserves_prior_pins_and_can_be_retried() {
    let (a, b, ma, mb) = enrollment_pair().await;
    let keep = Identity::new().await;
    a.auth
        .add_trusted_peer(&keep.key.public_key().to_openssh(), Some("keep".into()))
        .await
        .unwrap();
    let original = pin_text(&a).await;

    // A directory at the fixture lock path causes a portable write failure even
    // when the Linux test process is root. The original pin file is untouched.
    let lock = pin_file(&a).with_extension("lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    let (sid_a, sid_b) = pair(&ma, &mb).await;
    eventually(|| async {
        ma.inspect(Some(sid_a))
            .await
            .unwrap()
            .sessions
            .iter()
            .any(|s| s.session_id == sid_a && s.state == "Failed")
    })
    .await;
    assert_eq!(pin_text(&a).await, original);
    assert!(a.auth.is_authorized(&keep.key.public_key()).await.unwrap());
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    let peer = mb.inspect(Some(sid_b)).await.unwrap();
    assert!(peer.sessions.iter().all(|s| s.state != "Succeeded"));
    assert!(!peer
        .outcome
        .is_some_and(|outcome| outcome.status == "succeeded"));

    std::fs::remove_dir(&lock).unwrap();
    mb.cancel(None).await.unwrap();
    let open = mb.open().await.unwrap();
    let retried = ma
        .pair(
            &open.listen_addr.unwrap(),
            open.code.unwrap().code,
            None,
            None,
        )
        .await
        .unwrap()
        .requested_session_id
        .unwrap();
    wait_succeeded(&ma, retried).await;
    eventually(|| async {
        mb.inspect(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .any(|s| s.role == "acceptor" && s.state == "Succeeded")
    })
    .await;
    assert!(a.auth.is_authorized(&keep.key.public_key()).await.unwrap());
    assert!(a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(b.auth.is_authorized(&a.key.public_key()).await.unwrap());
    assert_mode_600(&pin_file(&a));
}

#[tokio::test]
async fn a_pairing_saved_before_code_pairing_still_connects() {
    let (a, b, _ma, mb) = enrollment_pair().await;
    // Pins exactly as fingerprint-compare enrollment wrote them.
    for (id, peer, name) in [(&a, &b, "desk-clipsync"), (&b, &a, "laptop-clipsync")] {
        let key = peer.key.public_key();
        id.auth
            .add_trusted_peer(
                &key.to_openssh(),
                Some(format!("{name} {}", key.fingerprint())),
            )
            .await
            .unwrap();
    }
    let pins_a = pin_text(&a).await;
    let pins_b = pin_text(&b).await;

    // B is allowing a new device and a stranger guesses wrong meanwhile.
    let open = mb.open().await.unwrap();
    let stranger = Identity::new().await;
    let mut config = Config::default();
    config.node_id = stranger.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = stranger.dir.path().join("identity");
    config.auth.authorized_keys = stranger.dir.path().join("authorized_keys");
    let ms = EnrollmentManager::new(Arc::new(config), stranger.auth.clone()).unwrap();
    let digits: u32 = open.code.unwrap().code.digits().parse().unwrap();
    let guess =
        clipsync::control::PairingCode::parse(&format!("{:06}", (digits + 7) % 1_000_000)).unwrap();
    let sid = ms
        .pair(&open.listen_addr.unwrap(), guess, None, None)
        .await
        .unwrap()
        .requested_session_id
        .unwrap();
    eventually(|| async {
        ms.inspect(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .any(|s| s.session_id == sid && s.state == "Failed")
    })
    .await;

    let ca = Clipboard::default();
    let cb = Clipboard::default();
    let (ea, ta) = sync_side(&a, &ca).await;
    let (eb, tb) = sync_side(&b, &cb).await;
    let listener_b = eb.bind_listener().await.unwrap();
    let sync_addr = listener_b.local_addr();
    let run_b = eb.clone();
    let _serve_b = Task(tokio::spawn(async move { run_b.run(listener_b).await }));
    let connected =
        tokio::time::timeout(Duration::from_secs(5), ta.connect_peer(&b.peer(sync_addr)))
            .await
            .expect("sync dial timed out");
    assert!(
        connected.is_ok(),
        "existing pins must open clipsync/2: {connected:?}"
    );
    eventually(async || tb.is_connected(a.id()).await && ta.is_connected(b.id()).await).await;
    ca.copy(FROM_PEER).await;
    assert_eq!(ea.force_sync().await.unwrap(), 1);
    eventually(async || *cb.text.lock().await == FROM_PEER).await;

    assert_eq!(pin_text(&a).await, pins_a);
    assert_eq!(pin_text(&b).await, pins_b);
    assert!(!b
        .auth
        .is_authorized(&stranger.key.public_key())
        .await
        .unwrap());
    ea.shutdown().await;
    eb.shutdown().await;
}

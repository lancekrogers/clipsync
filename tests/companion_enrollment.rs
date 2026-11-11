//! Loopback pairing-code acceptance tests. Temp keys only — no personal
//! clipboard, no installed service, no production pasteboard.
//!
//! Every refusal asserts that NEITHER side's authorized-devices file changed,
//! byte for byte (including "still absent").
mod support;
use clipsync::{
    auth::{Authenticator, KeyPair, KeyType, SshAuthenticator},
    config::Config,
    control::{
        self, Command, EnrollmentCandidateView, EnrollmentSessionView, EnrollmentSnapshot,
        EnrollmentTarget, PairingCode, Target, FEATURE_ENROLLMENT, FEATURE_ENROLLMENT_CODE,
    },
    enrollment::{EnrollmentManager, ENROLL_ALPN, LEGACY_ENROLL_ALPN},
    transport::{websocket::WebSocketConfig, Listener, WebSocketTransport},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use support::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{client::TlsStream, TlsAcceptor, TlsConnector};

struct Party {
    id: Identity,
    mgr: Arc<EnrollmentManager>,
}

impl Party {
    async fn new(name: &str) -> Self {
        Self::with_identity(Identity::new().await, name).await
    }

    async fn with_identity(id: Identity, name: &str) -> Self {
        let mut config = Config::default();
        config.node_id = id.id();
        config.listen_addr = "127.0.0.1:0".into();
        config.auth.ssh_key = id.dir.path().join("identity");
        config.auth.authorized_keys = id.dir.path().join("authorized_keys");
        config.clipboard.history_db = id.dir.path().join("history.db");
        config.advertise_name = name.into();
        let mgr = Arc::new(EnrollmentManager::new(Arc::new(config), id.auth.clone()).unwrap());
        mgr.set_listen_port_override(0).await;
        Self { id, mgr }
    }

    fn fp(&self) -> String {
        self.id.key.public_key().fingerprint()
    }

    fn pins_path(&self) -> PathBuf {
        self.id.dir.path().join("authorized_keys")
    }

    /// Raw bytes of the authorized-devices file; `None` while it does not exist.
    fn pins(&self) -> Option<Vec<u8>> {
        std::fs::read(self.pins_path()).ok()
    }

    async fn trusted(&self) -> Vec<String> {
        let mut fps: Vec<String> = self
            .id
            .auth
            .list_trusted_devices()
            .await
            .unwrap()
            .into_iter()
            .map(|(fp, _)| fp)
            .collect();
        fps.sort();
        fps
    }

    /// Open a window and return its address and the code on screen.
    async fn allow(&self) -> (String, PairingCode) {
        let snap = self.mgr.open().await.unwrap();
        let code = snap.code.expect("an open window shows a code").code;
        (snap.listen_addr.unwrap(), code)
    }

    async fn code(&self) -> Option<(String, u32)> {
        self.mgr
            .inspect(None)
            .await
            .unwrap()
            .code
            .map(|view| (view.code.digits().to_string(), view.attempts_left))
    }

    async fn pair(&self, addr: &str, code: &str) -> uuid::Uuid {
        self.mgr
            .pair(addr, PairingCode::parse(code).unwrap(), None, None)
            .await
            .unwrap()
            .requested_session_id
            .unwrap()
    }
}

fn wrong(code: &PairingCode) -> String {
    let n: u32 = code.digits().parse().unwrap();
    format!("{:06}", (n + 1) % 1_000_000)
}

fn ended(session: &EnrollmentSessionView) -> bool {
    matches!(
        session.state.as_str(),
        "Succeeded" | "Failed" | "Cancelled" | "Expired"
    )
}

async fn wait_end(mgr: &EnrollmentManager, id: uuid::Uuid) -> EnrollmentSessionView {
    for _ in 0..800 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| s.session_id == id) {
            if ended(s) {
                return s.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {id} did not finish");
}

/// The allowing side's most recent finished attempt.
async fn wait_acceptor_end(mgr: &EnrollmentManager) -> EnrollmentSessionView {
    for _ in 0..800 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap
            .sessions
            .iter()
            .find(|s| s.role == "acceptor" && ended(s))
        {
            return s.clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("allowing side never finished an attempt");
}

async fn wait_code_change(party: &Party, from: &str) -> (String, u32) {
    for _ in 0..400 {
        if let Some((code, left)) = party.code().await {
            if code != from {
                return (code, left);
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("code never changed");
}

async fn wait_failure(
    mgr: &EnrollmentManager,
    id: uuid::Uuid,
    expected: &str,
) -> EnrollmentSessionView {
    for _ in 0..800 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| s.session_id == id) {
            if s.failure.as_deref() == Some(expected) {
                return s.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {id} never reported {expected}");
}

async fn wait_state(mgr: &EnrollmentManager, role: &str, state: &str) -> uuid::Uuid {
    for _ in 0..400 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap
            .sessions
            .iter()
            .find(|s| s.role == role && s.state == state)
        {
            return s.session_id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no {role} session reached {state}");
}

/// An enrollment listener that completes TLS and then never answers.
async fn silent_peer() -> (String, Task<()>) {
    let id = Identity::new().await;
    let (_, server) = clipsync::enrollment::enroll_configs(id.auth.as_ref())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let task = Task(tokio::spawn(async move {
        let _id = id;
        let mut held = Vec::new();
        while let Ok((tcp, _)) = listener.accept().await {
            if let Ok(tls) = TlsAcceptor::from(server.clone()).accept(tcp).await {
                held.push(tls);
            }
        }
    }));
    (addr, task)
}

fn failure(session: &EnrollmentSessionView) -> &str {
    session.failure.as_deref().unwrap_or("<none>")
}

/// A TLS client with any key and ALPN list, standing in for an attacker or an
/// older ClipSync.
async fn raw_dial(id: &Identity, addr: &str, alpn: &[&[u8]]) -> TlsStream<TcpStream> {
    let (client, _) = clipsync::enrollment::enroll_configs_with(id.auth.as_ref(), alpn)
        .await
        .unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    TlsConnector::from(client)
        .connect("clipsync.local".try_into().unwrap(), tcp)
        .await
        .unwrap()
}

async fn write_json<S: AsyncWriteExt + Unpin>(stream: &mut S, value: serde_json::Value) {
    let data = serde_json::to_vec(&value).unwrap();
    stream.write_u32(data.len() as u32).await.unwrap();
    stream.write_all(&data).await.unwrap();
    stream.flush().await.unwrap();
}

async fn read_json<S: AsyncReadExt + Unpin>(stream: &mut S) -> serde_json::Value {
    let len = tokio::time::timeout(Duration::from_secs(5), stream.read_u32())
        .await
        .unwrap()
        .unwrap();
    let mut data = vec![0; len as usize];
    stream.read_exact(&mut data).await.unwrap();
    serde_json::from_slice(&data).unwrap()
}

fn released_port() -> String {
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap().to_string();
    drop(reserved);
    address
}

#[tokio::test]
async fn right_code_pairs_and_each_file_gains_exactly_the_other_key() {
    let allowing = Party::new("Studio Mac").await;
    let joining = Party::new("Travel laptop").await;
    let keep = Identity::new().await;
    allowing
        .id
        .auth
        .add_trusted_peer(&keep.key.public_key().to_openssh(), Some("keep".into()))
        .await
        .unwrap();
    let allowing_before = allowing.trusted().await;
    assert!(joining.pins().is_none());

    let (addr, code) = allowing.allow().await;
    assert_eq!(code.digits().len(), 6);
    let sid = joining.pair(&addr, code.digits()).await;
    // The joining computer never opens a window of its own.
    assert!(!joining.mgr.inspect(None).await.unwrap().listening);
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(session.state, "Succeeded", "{}", failure(&session));
    assert_eq!(
        session.peer_fingerprint.as_deref(),
        Some(allowing.fp().as_str())
    );
    assert_eq!(session.peer_display_name.as_deref(), Some("Studio Mac"));
    let accepted = wait_acceptor_end(&allowing.mgr).await;
    assert_eq!(accepted.state, "Succeeded", "{}", failure(&accepted));
    assert_eq!(
        accepted.peer_fingerprint.as_deref(),
        Some(joining.fp().as_str())
    );
    assert_eq!(accepted.peer_display_name.as_deref(), Some("Travel laptop"));

    let mut expect = allowing_before.clone();
    expect.push(joining.fp());
    expect.sort();
    assert_eq!(allowing.trusted().await, expect);
    assert_eq!(joining.trusted().await, vec![allowing.fp()]);
    let allowing_file = String::from_utf8(allowing.pins().unwrap()).unwrap();
    assert!(allowing_file.contains(&joining.id.key.public_key().to_openssh()));
    assert!(allowing_file.contains(&format!("Travel laptop {}", joining.fp())));
    let joining_file = String::from_utf8(joining.pins().unwrap()).unwrap();
    assert!(joining_file.contains(&format!("Studio Mac {}", allowing.fp())));

    // One window, one device: the window closes and its code is gone.
    let after = allowing.mgr.inspect(None).await.unwrap();
    assert!(!after.listening);
    assert!(after.code.is_none());
    assert_eq!(after.outcome.unwrap().status, "succeeded");
}

#[tokio::test]
async fn wrong_code_writes_nothing_and_shows_a_new_code() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let (_, attempts) = allowing.code().await.unwrap();

    let sid = joining.pair(&addr, &wrong(&code)).await;
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(session.state, "Failed");
    assert_eq!(failure(&session), "wrong_code");
    let accepted = wait_acceptor_end(&allowing.mgr).await;
    assert_eq!(failure(&accepted), "wrong_code");

    let (next, left) = allowing
        .code()
        .await
        .expect("window stays open with a code");
    assert_ne!(next, code.digits());
    assert_eq!(left, attempts - 1);
    assert_eq!(
        allowing
            .mgr
            .inspect(None)
            .await
            .unwrap()
            .outcome
            .unwrap()
            .status,
        "wrong_code"
    );
    assert_eq!((allowing.pins(), joining.pins()), before);

    // The new code still works.
    let sid = joining.pair(&addr, &next).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
}

#[tokio::test]
async fn a_burned_code_cannot_be_used_again() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let attacker = Identity::new().await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let (_, attempts) = allowing.code().await.unwrap();

    // A connection consumes the code before it sends any PAKE input.
    let stalled = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    let (shown, left) = wait_code_change(&allowing, code.digits()).await;
    assert_eq!(left, attempts - 1);
    assert_ne!(shown, code.digits());

    let sid = joining.pair(&addr, code.digits()).await;
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(failure(&session), "wrong_code");
    drop(stalled);
    assert_eq!((allowing.pins(), joining.pins()), before);
    assert!(allowing.trusted().await.is_empty());
}

#[tokio::test]
async fn an_expired_code_is_refused() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    allowing
        .mgr
        .set_code_ttl_override(Duration::from_millis(300))
        .await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let (_, attempts) = allowing.code().await.unwrap();

    // A timer renewal replaces the code without spending an attempt.
    let (_, left) = wait_code_change(&allowing, code.digits()).await;
    assert_eq!(left, attempts);

    let sid = joining.pair(&addr, code.digits()).await;
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(failure(&session), "wrong_code");
    assert_eq!((allowing.pins(), joining.pins()), before);
}

#[tokio::test]
async fn a_relay_with_its_own_keys_cannot_pair_two_honest_computers() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let (to_joining, to_allowing) = (Identity::new().await, Identity::new().await);
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;

    // The relay terminates TLS on both sides with its own keys and forwards
    // every enrollment frame unchanged.
    let (_, relay_server) = clipsync::enrollment::enroll_configs(to_joining.auth.as_ref())
        .await
        .unwrap();
    let (relay_client, _) = clipsync::enrollment::enroll_configs(to_allowing.auth.as_ref())
        .await
        .unwrap();
    let relay = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay.local_addr().unwrap().to_string();
    let upstream = addr.clone();
    let _relay = Task(tokio::spawn(async move {
        let (tcp, _) = relay.accept().await.unwrap();
        let mut down = TlsAcceptor::from(relay_server).accept(tcp).await.unwrap();
        let tcp = TcpStream::connect(upstream).await.unwrap();
        let mut up = TlsConnector::from(relay_client)
            .connect("clipsync.local".try_into().unwrap(), tcp)
            .await
            .unwrap();
        let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
    }));

    let sid = joining.pair(&relay_addr, code.digits()).await;
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(failure(&session), "wrong_code");
    assert_eq!(
        session.peer_fingerprint,
        Some(to_joining.key.public_key().fingerprint())
    );
    // `wrong_code` here means the allowing side read the relayed Hello and
    // key confirmation, and the confirmation did not verify.
    let accepted = wait_acceptor_end(&allowing.mgr).await;
    assert_eq!(failure(&accepted), "wrong_code");
    assert_eq!(
        accepted.peer_fingerprint,
        Some(to_allowing.key.public_key().fingerprint())
    );
    assert_eq!((allowing.pins(), joining.pins()), before);
}

#[tokio::test]
async fn an_older_allowing_computer_is_reported_as_needing_an_update() {
    let old = Identity::new().await;
    let joining = Party::new("joining").await;
    let before = (
        std::fs::read(old.dir.path().join("authorized_keys")).ok(),
        joining.pins(),
    );
    let (_, server) =
        clipsync::enrollment::enroll_configs_with(old.auth.as_ref(), &[LEGACY_ENROLL_ALPN])
            .await
            .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _old = Task(tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = TlsAcceptor::from(server).accept(tcp).await.unwrap();
        let mut sink = [0u8; 256];
        while matches!(tls.read(&mut sink).await, Ok(n) if n > 0) {}
    }));

    let sid = joining.pair(&addr, "123456").await;
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(failure(&session), "update_required");
    assert!(session.error.unwrap().contains("Update ClipSync there"));
    assert_eq!(
        (
            std::fs::read(old.dir.path().join("authorized_keys")).ok(),
            joining.pins()
        ),
        before
    );
}

#[tokio::test]
async fn an_older_joining_computer_is_told_to_update_and_spends_no_code() {
    let allowing = Party::new("allowing").await;
    let old = Identity::new().await;
    let before = (
        allowing.pins(),
        std::fs::read(old.dir.path().join("authorized_keys")).ok(),
    );
    let (addr, code) = allowing.allow().await;
    let attempts = allowing.code().await.unwrap().1;

    let mut tls = raw_dial(&old, &addr, &[LEGACY_ENROLL_ALPN]).await;
    // The fingerprint-compare Hello an older ClipSync sends first.
    write_json(
        &mut tls,
        serde_json::json!({
            "type": "hello",
            "session_id": uuid::Uuid::new_v4(),
            "nonce_hex": "00".repeat(32),
            "local_fingerprint": old.key.public_key().fingerprint(),
            "display_name": "old laptop",
        }),
    )
    .await;
    let reply = read_json(&mut tls).await;
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["code"], "update_required");
    assert!(reply["message"]
        .as_str()
        .unwrap()
        .contains("Update ClipSync on this computer"));

    assert_eq!(
        allowing.code().await,
        Some((code.digits().into(), attempts))
    );
    let snap = allowing.mgr.inspect(None).await.unwrap();
    assert!(snap.sessions.is_empty());
    assert_eq!(snap.outcome.unwrap().status, "update_required");
    assert_eq!(
        (
            allowing.pins(),
            std::fs::read(old.dir.path().join("authorized_keys")).ok()
        ),
        before
    );
}

#[tokio::test]
async fn a_peer_presenting_this_computers_own_key_is_refused() {
    let allowing = Party::new("allowing").await;
    // Same private key, separate authorized-devices file.
    let twin_dir = tempfile::TempDir::new().unwrap();
    allowing
        .id
        .key
        .save_to_file(&twin_dir.path().join("identity"))
        .await
        .unwrap();
    let twin_auth = Arc::new(
        SshAuthenticator::new(clipsync::auth::AuthConfig {
            private_key_path: twin_dir.path().join("identity"),
            authorized_keys_path: twin_dir.path().join("authorized_keys"),
            generate_if_missing: false,
        })
        .await
        .unwrap(),
    );
    let twin = Party::with_identity(
        Identity {
            dir: twin_dir,
            auth: twin_auth,
            key: KeyPair::load_from_file(&allowing.id.dir.path().join("identity"))
                .await
                .unwrap(),
        },
        "twin",
    )
    .await;
    assert_eq!(twin.fp(), allowing.fp());
    let before = (allowing.pins(), twin.pins());
    let (addr, code) = allowing.allow().await;
    let attempts = allowing.code().await.unwrap().1;

    let sid = twin.pair(&addr, code.digits()).await;
    let session = wait_end(&twin.mgr, sid).await;
    assert_eq!(failure(&session), "reflection");

    // The allowing side refuses its own key before spending the code.
    let mut tls = raw_dial(&twin.id, &addr, &[ENROLL_ALPN]).await;
    let reply = read_json(&mut tls).await;
    assert_eq!(reply["code"], "reflection");
    assert_eq!(
        allowing.code().await,
        Some((code.digits().into(), attempts))
    );
    assert_eq!((allowing.pins(), twin.pins()), before);
}

#[tokio::test]
async fn concurrent_attempts_never_share_a_code() {
    let allowing = Party::new("allowing").await;
    let first = Party::new("first").await;
    let second = Party::new("second").await;
    let before = (first.pins(), second.pins());
    let (addr, code) = allowing.allow().await;

    let (a, b) = tokio::join!(
        first.pair(&addr, code.digits()),
        second.pair(&addr, code.digits())
    );
    let (a, b) = tokio::join!(wait_end(&first.mgr, a), wait_end(&second.mgr, b));
    let winners: Vec<_> = [(&first, &a), (&second, &b)]
        .into_iter()
        .filter(|(_, s)| s.state == "Succeeded")
        .collect();
    assert_eq!(winners.len(), 1, "{a:?} / {b:?}");
    let (winner, loser) = if a.state == "Succeeded" {
        (&first, &second)
    } else {
        (&second, &first)
    };
    let lost = if a.state == "Succeeded" { &b } else { &a };
    // The loser either spent the next code or found the window already closed.
    assert!(
        matches!(failure(lost), "wrong_code" | "not_listening" | "busy"),
        "{lost:?}"
    );
    assert_eq!(allowing.trusted().await, vec![winner.fp()]);
    assert_eq!(winner.trusted().await, vec![allowing.fp()]);
    let loser_before = if std::ptr::eq(loser, &first) {
        &before.0
    } else {
        &before.1
    };
    assert_eq!(&loser.pins(), loser_before);
}

#[tokio::test]
async fn many_simultaneous_connections_each_consume_a_distinct_code() {
    let allowing = Party::new("allowing").await;
    allowing.mgr.set_attempts_override(10).await;
    let before = allowing.pins();
    let (addr, code) = allowing.allow().await;
    let attackers = [
        Identity::new().await,
        Identity::new().await,
        Identity::new().await,
    ];
    let mut seen = vec![code.digits().to_string()];
    let mut held = Vec::new();
    for attacker in &attackers {
        held.push(raw_dial(attacker, &addr, &[ENROLL_ALPN]).await);
        let (next, _) = wait_code_change(&allowing, seen.last().unwrap()).await;
        seen.push(next);
    }
    let (_, left) = allowing.code().await.unwrap();
    assert_eq!(left, 10 - 3);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "a code was shown twice: {seen:?}");
    drop(held);
    assert_eq!(allowing.pins(), before);
}

#[tokio::test]
async fn no_code_open_is_reported_distinctly() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    allowing.mgr.set_attempts_override(1).await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let attacker = Identity::new().await;
    let stalled = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    eventually(async || allowing.code().await.is_none()).await;
    assert!(allowing.mgr.inspect(None).await.unwrap().listening);

    let sid = joining.pair(&addr, code.digits()).await;
    let session = wait_end(&joining.mgr, sid).await;
    assert_eq!(failure(&session), "no_code");
    assert!(session.error.unwrap().contains("no pairing code open"));

    // When the last attempt ends, the window closes.
    drop(stalled);
    eventually(async || !allowing.mgr.inspect(None).await.unwrap().listening).await;
    assert_eq!(
        allowing
            .mgr
            .inspect(None)
            .await
            .unwrap()
            .outcome
            .unwrap()
            .status,
        "closed"
    );
    assert_eq!((allowing.pins(), joining.pins()), before);
}

#[tokio::test]
async fn nothing_listening_and_a_non_pairing_port_are_reported_distinctly() {
    let joining = Party::new("joining").await;
    let before = joining.pins();
    let sid = joining.pair(&released_port(), "123456").await;
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "not_listening");

    // A production sync listener offers clipsync/2 only.
    let sync = Identity::new().await;
    sync.trust(&joining.id).await;
    let transport = WebSocketTransport::new(
        "127.0.0.1:0".parse().unwrap(),
        sync.auth.clone(),
        WebSocketConfig::default(),
        sync.id(),
    );
    let listener = transport.listen().await.unwrap();
    let sync_addr = listener.local_addr();
    let _accept = Task(tokio::spawn(async move {
        let mut listener = listener;
        loop {
            let _ = listener.accept().await;
        }
    }));
    let sid = joining.pair(&sync_addr.to_string(), "123456").await;
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "not_pairing");
    assert_eq!(joining.pins(), before);
    assert!(!joining
        .id
        .auth
        .is_authorized(&sync.key.public_key())
        .await
        .unwrap());
}

#[tokio::test]
async fn a_picked_nearby_device_must_be_the_one_that_answers() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let attempts = allowing.code().await.unwrap().1;
    let spoofed = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    joining
        .mgr
        .inject_candidate(EnrollmentCandidateView {
            display_name: "spoof".into(),
            address: addr.clone(),
            advertised_fingerprint: Some(spoofed.into()),
            advertised_node_id: Some(allowing.id.id()),
            source: "mdns".into(),
        })
        .await;
    let sid = joining
        .mgr
        .pair(
            &addr,
            PairingCode::parse(code.digits()).unwrap(),
            Some(spoofed.into()),
            Some(allowing.id.id()),
        )
        .await
        .unwrap()
        .requested_session_id
        .unwrap();
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "mismatch");
    // The joining side stops before sending any PAKE message. The connection
    // still spent a code; nothing was guessed.
    eventually(async || allowing.code().await.map(|c| c.1) == Some(attempts - 1)).await;
    assert_eq!((allowing.pins(), joining.pins()), before);
}

#[tokio::test]
async fn stopping_the_window_discards_its_code() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let snap = allowing.mgr.cancel(None).await.unwrap();
    assert!(!snap.listening && snap.code.is_none());
    assert_eq!(snap.outcome.unwrap().status, "cancelled");
    let sid = joining.pair(&addr, code.digits()).await;
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "not_listening");
    assert_eq!((allowing.pins(), joining.pins()), before);
}

#[tokio::test]
async fn an_idle_window_closes_on_its_own() {
    let allowing = Party::new("allowing").await;
    allowing
        .mgr
        .set_window_ttl_override(Duration::from_millis(250))
        .await;
    let before = allowing.pins();
    let (_addr, _code) = allowing.allow().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let snap = allowing.mgr.inspect(None).await.unwrap();
    assert!(!snap.listening && snap.code.is_none());
    assert_eq!(snap.outcome.unwrap().status, "expired");
    assert_eq!(allowing.pins(), before);
}

#[tokio::test]
async fn a_window_accepts_a_bounded_number_of_attempts() {
    assert_eq!(clipsync::enrollment::MAX_PAIRING_ATTEMPTS, 5);
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let before = (allowing.pins(), joining.pins());
    let (addr, mut code) = allowing.allow().await;
    for attempt in 1..=5u32 {
        let sid = joining.pair(&addr, &wrong(&code)).await;
        assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "wrong_code");
        match allowing.code().await {
            Some((next, left)) => {
                assert!(attempt < 5);
                assert_eq!(left, 5 - attempt);
                code = PairingCode::parse(&next).unwrap();
            }
            None => assert_eq!(attempt, 5),
        }
    }
    eventually(async || !allowing.mgr.inspect(None).await.unwrap().listening).await;
    let sid = joining.pair(&addr, code.digits()).await;
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "not_listening");
    assert_eq!((allowing.pins(), joining.pins()), before);
}

#[tokio::test]
async fn saved_on_one_side_only_is_reported_as_incomplete() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let allowing_before = allowing.pins();
    // A directory at the allowing side's lock path makes only its save fail.
    let lock = allowing.pins_path().with_extension("lock");
    let _ = std::fs::remove_file(&lock);
    std::fs::create_dir(&lock).unwrap();
    let (addr, code) = allowing.allow().await;

    let sid = joining.pair(&addr, code.digits()).await;
    let session = wait_failure(&joining.mgr, sid, "incomplete").await;
    assert_eq!(session.state, "WaitingPeerAuth");
    assert!(session.error.unwrap().contains("did not confirm it saved"));
    assert_eq!(
        joining
            .mgr
            .inspect(None)
            .await
            .unwrap()
            .outcome
            .unwrap()
            .status,
        "incomplete"
    );
    assert_eq!(joining.trusted().await, vec![allowing.fp()]);
    assert_eq!(
        failure(&wait_acceptor_end(&allowing.mgr).await),
        "save_failed"
    );
    std::fs::remove_dir(&lock).unwrap();
    assert_eq!(allowing.pins(), allowing_before);
}

#[tokio::test]
async fn malformed_and_oversize_frames_after_the_burn_write_nothing() {
    let allowing = Party::new("allowing").await;
    let before = allowing.pins();
    let (addr, _) = allowing.allow().await;
    let attempts = allowing.code().await.unwrap().1;
    let attacker = Identity::new().await;

    let mut oversize = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    oversize.write_u32(u32::MAX).await.unwrap();
    oversize.flush().await.unwrap();
    let mut garbage = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    garbage.write_u32(5).await.unwrap();
    garbage.write_all(b"{{{{{").await.unwrap();
    garbage.flush().await.unwrap();

    eventually(async || {
        allowing
            .mgr
            .inspect(None)
            .await
            .unwrap()
            .sessions
            .iter()
            .filter(|s| s.role == "acceptor" && s.state == "Failed")
            .count()
            == 2
    })
    .await;
    let (code, left) = allowing.code().await.expect("window stays open");
    assert_eq!(left, attempts - 2);
    assert_eq!(allowing.pins(), before);

    // The same window still pairs a real device.
    let joining = Party::new("joining").await;
    let sid = joining.pair(&addr, &code).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
}

#[tokio::test]
async fn a_fourth_simultaneous_attempt_is_refused_without_spending_a_code() {
    assert_eq!(clipsync::enrollment::MAX_CONCURRENT_SESSIONS, 3);
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let before = (allowing.pins(), joining.pins());
    let (addr, code) = allowing.allow().await;
    let attackers = [
        Identity::new().await,
        Identity::new().await,
        Identity::new().await,
    ];
    let mut shown = code.digits().to_string();
    let mut held = Vec::new();
    for attacker in &attackers {
        held.push(raw_dial(attacker, &addr, &[ENROLL_ALPN]).await);
        shown = wait_code_change(&allowing, &shown).await.0;
    }
    let (_, left) = allowing.code().await.unwrap();

    let sid = joining.pair(&addr, &shown).await;
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "busy");
    assert_eq!(allowing.code().await, Some((shown, left)));
    assert_eq!((allowing.pins(), joining.pins()), before);
    drop(held);
}

#[tokio::test]
async fn cancelling_or_expiring_an_attempt_mid_exchange_writes_nothing() {
    // Allowing side: one stalled attempt is cancelled, the next one expires.
    let allowing = Party::new("allowing").await;
    allowing
        .mgr
        .set_session_ttl_override(Duration::from_millis(300))
        .await;
    let before = allowing.pins();
    let (addr, _) = allowing.allow().await;
    let attacker = Identity::new().await;
    let first = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    let id = wait_state(&allowing.mgr, "acceptor", "Verifying").await;
    allowing.mgr.cancel(Some(id)).await.unwrap();
    let cancelled = wait_end(&allowing.mgr, id).await;
    assert_eq!(cancelled.state, "Cancelled");
    let second = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    let id = wait_state(&allowing.mgr, "acceptor", "Verifying").await;
    assert_eq!(wait_end(&allowing.mgr, id).await.state, "Expired");
    assert!(allowing.mgr.inspect(None).await.unwrap().listening);
    drop((first, second));
    assert_eq!(allowing.pins(), before);

    // Joining side: the computer that answered never replies.
    let (silent, _peer) = silent_peer().await;
    let joining = Party::new("joining").await;
    let sid = joining.pair(&silent, "123456").await;
    wait_state(&joining.mgr, "initiator", "Verifying").await;
    joining.mgr.cancel(Some(sid)).await.unwrap();
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Cancelled");
    joining
        .mgr
        .set_session_ttl_override(Duration::from_millis(300))
        .await;
    let sid = joining.pair(&silent, "123456").await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Expired");
    assert!(joining.pins().is_none());
}

#[tokio::test]
async fn honest_typos_do_not_lock_a_computer_out() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    // Five typos use up a window; the codes were the cost, not the key.
    let (addr, mut code) = allowing.allow().await;
    for _ in 0..5 {
        let sid = joining.pair(&addr, &wrong(&code)).await;
        assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "wrong_code");
        if let Some((next, _)) = allowing.code().await {
            code = PairingCode::parse(&next).unwrap();
        }
    }
    eventually(async || !allowing.mgr.inspect(None).await.unwrap().listening).await;
    let (addr, code) = allowing.allow().await;
    let sid = joining.pair(&addr, code.digits()).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
}

#[tokio::test]
async fn a_successful_pairing_ends_attempts_still_checking_other_codes() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let (addr, code) = allowing.allow().await;
    let attacker = Identity::new().await;
    let stalled = raw_dial(&attacker, &addr, &[ENROLL_ALPN]).await;
    let sibling = wait_state(&allowing.mgr, "acceptor", "Verifying").await;
    let (shown, _) = wait_code_change(&allowing, code.digits()).await;

    let sid = joining.pair(&addr, &shown).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
    // One window, one device: the other attempt cannot pair after this.
    assert_eq!(wait_end(&allowing.mgr, sibling).await.state, "Cancelled");
    assert_eq!(allowing.trusted().await, vec![joining.fp()]);
    drop(stalled);
}

#[tokio::test]
async fn revoke_preserves_unrelated_pins() {
    let a = Party::new("a").await;
    let b = Identity::new().await;
    let keep = Identity::new().await;
    a.id.auth
        .add_trusted_peer(&keep.key.public_key().to_openssh(), Some("keep".into()))
        .await
        .unwrap();
    a.id.auth
        .add_trusted_peer(&b.key.public_key().to_openssh(), Some("drop".into()))
        .await
        .unwrap();
    let devices = a
        .mgr
        .revoke(&b.key.public_key().fingerprint())
        .await
        .unwrap();
    assert!(devices
        .iter()
        .any(|d| d.fingerprint == keep.key.public_key().fingerprint()));
    assert!(!devices
        .iter()
        .any(|d| d.fingerprint == b.key.public_key().fingerprint()));
    assert!(a
        .id
        .auth
        .is_authorized(&keep.key.public_key())
        .await
        .unwrap());
    assert!(!a.id.auth.is_authorized(&b.key.public_key()).await.unwrap());
}

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

fn control_config(party: &Party) -> Config {
    let mut config = Config::default();
    config.node_id = party.id.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = party.id.dir.path().join("identity");
    config.auth.authorized_keys = party.pins_path();
    config.clipboard.history_db = party.id.dir.path().join("history.db");
    config
}

#[tokio::test]
async fn the_service_advertises_code_pairing_and_not_the_retired_flow() {
    let party = Party::new("a").await;
    let config = control_config(&party);
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let target = Arc::new(EnrollmentTarget::new(Arc::new(Stub), party.mgr.clone()));
    let _task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));

    let status = control::request(&path, Command::Status, max_frame)
        .await
        .unwrap();
    let features = status.features.unwrap();
    assert!(features.iter().any(|f| f == FEATURE_ENROLLMENT_CODE));
    assert!(!features.iter().any(|f| f == FEATURE_ENROLLMENT));

    let open = control::request(&path, Command::EnrollmentOpen, max_frame)
        .await
        .unwrap();
    let snap = open.enrollment.unwrap();
    assert!(snap.listening);
    assert_eq!(snap.code.unwrap().code.digits().len(), 6);

    // The retired confirm command no longer parses.
    let retired = serde_json::json!({
        "version": 1,
        "command": {"EnrollmentConfirm": {"session_id": uuid::Uuid::new_v4()}}
    });
    assert!(serde_json::from_value::<control::Request>(retired).is_err());
}

#[tokio::test]
async fn inspect_closed_returns_snapshot_not_error() {
    let party = Party::new("a").await;
    party.allow().await;
    party.mgr.cancel(None).await.unwrap();
    let snap = party
        .mgr
        .inspect(None)
        .await
        .expect("closed inspect must succeed");
    assert!(!snap.listening);
    assert!(snap.code.is_none());
}

#[tokio::test]
async fn control_pair_returns_under_ipc_budget() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let (addr, code) = allowing.allow().await;
    let config = control_config(&joining);
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let target = Arc::new(EnrollmentTarget::new(Arc::new(Stub), joining.mgr.clone()));
    let _task = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));

    let started = std::time::Instant::now();
    let resp = control::request_result(
        &path,
        Command::EnrollmentPair {
            address: addr,
            code,
            expected_fingerprint: None,
            expected_node_id: None,
        },
        max_frame,
    )
    .await
    .expect("pair IPC");
    assert!(started.elapsed() < Duration::from_secs(3));
    let sid = resp.enrollment.unwrap().requested_session_id.unwrap();
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");

    let bad = control::request_result(
        &path,
        Command::EnrollmentPair {
            address: "not an address".into(),
            code: PairingCode::parse("123456").unwrap(),
            expected_fingerprint: None,
            expected_node_id: None,
        },
        max_frame,
    )
    .await
    .unwrap_err();
    assert_eq!(
        bad.code.as_deref(),
        Some(control::error_codes::ENROLLMENT_ADDRESS_INVALID)
    );
}

#[tokio::test]
async fn a_finished_pairing_survives_a_late_cancel() {
    let allowing = Party::new("allowing").await;
    let joining = Party::new("joining").await;
    let (addr, code) = allowing.allow().await;
    let sid = joining.pair(&addr, code.digits()).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
    let _ = joining.mgr.cancel(Some(sid)).await;
    let _ = joining.mgr.cancel(None).await;
    let _ = allowing.mgr.cancel(None).await;
    assert_eq!(joining.trusted().await, vec![allowing.fp()]);
    assert_eq!(allowing.trusted().await, vec![joining.fp()]);
}

#[tokio::test]
async fn display_names_cannot_inject_authorization_lines() {
    let a = Party::new("a").await;
    let b = Identity::new().await;
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
    let err =
        a.id.auth
            .add_trusted_peer(&b.key.public_key().to_openssh(), Some(evil_line))
            .await
            .unwrap_err();
    assert!(err.to_string().contains("CR/LF") || err.to_string().contains("comment"));
    assert!(!a
        .id
        .auth
        .is_authorized(&evil.key.public_key())
        .await
        .unwrap());

    // A peer whose name carries control characters pairs with a cleaned label.
    let joining = Party::new("Laptop\u{7}\u{1b}[31m").await;
    let (addr, code) = a.allow().await;
    let sid = joining.pair(&addr, code.digits()).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
    let file = String::from_utf8(a.pins().unwrap()).unwrap();
    assert!(!file.contains('\u{7}') && !file.contains('\u{1b}'));
    assert_eq!(
        file.lines()
            .filter(|l| l.starts_with("ssh-ed25519"))
            .count(),
        1
    );
}

#[tokio::test]
async fn loopback_discovery_never_starts_mdns() {
    let party = Party::new("a").await;
    let snap = party.mgr.discover().await.unwrap();
    assert!(snap.candidates.iter().all(|c| c.source != "mdns"));
    party
        .mgr
        .inject_candidate(EnrollmentCandidateView {
            display_name: "manual".into(),
            address: "127.0.0.1:9".into(),
            advertised_fingerprint: None,
            advertised_node_id: None,
            source: "manual".into(),
        })
        .await;
    // Candidates survive ordinary polls, with or without a window here.
    assert!(party.mgr.discover().await.unwrap().candidates.len() == 1);
    assert!(party.mgr.inspect(None).await.unwrap().candidates.len() == 1);
}

#[tokio::test]
async fn repeated_mismatches_exhaust_that_peers_budget_without_blocking_another() {
    let target = Party::new("target").await;
    let joining = Party::new("joining").await;
    let (addr, _) = target.allow().await;
    target.mgr.set_attempts_override(50).await;
    for _ in 0..5 {
        let sid = joining
            .mgr
            .pair(
                &addr,
                PairingCode::parse("000000").unwrap(),
                Some("SHA256:wrong".into()),
                None,
            )
            .await
            .unwrap()
            .requested_session_id
            .unwrap();
        assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "mismatch");
    }
    let sid = joining
        .mgr
        .pair(&addr, PairingCode::parse("000000").unwrap(), None, None)
        .await
        .unwrap()
        .requested_session_id
        .unwrap();
    assert_eq!(failure(&wait_end(&joining.mgr, sid).await), "rate_limited");

    let other = Party::new("other").await;
    let (other_addr, code) = other.allow().await;
    let sid = joining.pair(&other_addr, code.digits()).await;
    assert_eq!(wait_end(&joining.mgr, sid).await.state, "Succeeded");
    assert!(target.trusted().await.is_empty());
}

#[tokio::test]
async fn identical_display_names_are_told_apart_by_tls_key() {
    let a = Party::new("same-desktop").await;
    let b = Party::new("same-desktop").await;
    let (addr, code) = a.allow().await;
    let sid = b.pair(&addr, code.digits()).await;
    let joined = wait_end(&b.mgr, sid).await;
    assert_eq!(joined.state, "Succeeded");
    assert_eq!(joined.peer_fingerprint.as_deref(), Some(a.fp().as_str()));
    let accepted = wait_acceptor_end(&a.mgr).await;
    assert_eq!(accepted.peer_fingerprint.as_deref(), Some(b.fp().as_str()));
    assert_ne!(a.fp(), b.fp());
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
    let writer = SshAuthenticator::new(cfg.clone()).await.unwrap();
    let reader = SshAuthenticator::new(clipsync::auth::AuthConfig {
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

fn snapshot_json(snap: &EnrollmentSnapshot) -> String {
    serde_json::to_string(snap).unwrap()
}

#[test]
fn a_snapshot_carries_the_code_to_the_local_ui_but_debug_hides_it() {
    let code = PairingCode::parse("482913").unwrap();
    let snap = EnrollmentSnapshot {
        requested_session_id: None,
        listening: true,
        listen_addr: Some("127.0.0.1:8485".into()),
        listen_hints: vec![],
        expires_at: None,
        sessions: vec![],
        candidates: vec![],
        outcome: None,
        code: Some(clipsync::control::PairingCodeView {
            code: code.clone(),
            remaining_secs: 90,
            attempts_left: 5,
        }),
    };
    assert!(snapshot_json(&snap).contains("482913"));
    assert!(!format!("{snap:?}").contains("482913"));
    let command = Command::EnrollmentPair {
        address: "127.0.0.1:8485".into(),
        code,
        expected_fingerprint: None,
        expected_node_id: None,
    };
    assert!(!format!("{command:?}").contains("482913"));
}

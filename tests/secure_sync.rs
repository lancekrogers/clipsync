mod support;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    auth::{AuthError, AuthToken, Authenticator, PeerId, PublicKey},
    clipboard::ClipboardProvider,
    config::Config,
    sync::SyncEngine,
    transport::{
        websocket::WebSocketConfig, Listener, Message, MessagePayload, MessageType,
        TransportConfig, TransportManager, WebSocketTransport,
    },
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use support::*;

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig {
        connect_timeout: Duration::from_secs(2),
        keepalive_interval: Duration::from_millis(100),
        ..Default::default()
    }
}
async fn fixture<C: ClipboardProvider + Clone + 'static>(
    id: &Identity,
    clipboard: &C,
) -> (Arc<SyncEngine>, Arc<TransportManager>) {
    let mut config = Config::default();
    config.node_id = id.id();
    config.listen_addr = "127.0.0.1:0".into();
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
    (
        Arc::new(SyncEngine::without_discovery(
            Arc::new(config),
            Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
            history,
            transport.clone(),
        )),
        transport,
    )
}
#[tokio::test]
async fn bidirectional_sync_no_echo_revocation_and_reconnect() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = Clipboard::default();
    let cb = Clipboard::default();
    let (ea, ta) = fixture(&a, &ca).await;
    let (eb, tb) = fixture(&b, &cb).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr = lb.local_addr();
    let ra = ea.clone();
    let rb = eb.clone();
    let _a = Task(tokio::spawn(async move { ra.run(la).await }));
    let _b = Task(tokio::spawn(async move { rb.run(lb).await }));
    tokio::time::sleep(Duration::from_millis(30)).await;
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    ca.copy("hello from alpha").await;
    assert_eq!(ea.force_sync().await.unwrap(), 1);
    eventually(async || *cb.text.lock().await == "hello from alpha").await;
    cb.copy("hello from beta").await;
    assert_eq!(eb.force_sync().await.unwrap(), 1);
    eventually(async || *ca.text.lock().await == "hello from beta").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(ca.writes.load(Ordering::SeqCst), 1);
    assert_eq!(cb.writes.load(Ordering::SeqCst), 1);
    tb.shutdown().await;
    eventually(async || !ta.is_connected(b.id()).await).await;
    ca.copy("copied while disconnected").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    eventually(async || *cb.text.lock().await == "copied while disconnected").await;
    ca.copy("after reconnect").await;
    ea.force_sync().await.unwrap();
    eventually(async || *cb.text.lock().await == "after reconnect").await;
    b.auth
        .remove_peer(&a.key.public_key().fingerprint())
        .await
        .unwrap();
    eventually(async || !tb.is_connected(a.id()).await).await;
    assert!(ta.connect_peer(&b.peer(addr)).await.is_err() || !tb.is_connected(a.id()).await);
    ea.shutdown().await;
    eb.shutdown().await;
}

#[tokio::test]
async fn copy_local_updates_clipboard_and_syncs_without_echo() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = Clipboard::default();
    let cb = Clipboard::default();
    let (ea, ta) = fixture(&a, &ca).await;
    let (eb, tb) = fixture(&b, &cb).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr = lb.local_addr();
    let ra = ea.clone();
    let rb = eb.clone();
    let _a = Task(tokio::spawn(async move { ra.run(la).await }));
    let _b = Task(tokio::spawn(async move { rb.run(lb).await }));
    tokio::time::sleep(Duration::from_millis(30)).await;
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    assert_eq!(
        ea.copy_local("explicit daemon copy".into()).await.unwrap(),
        1
    );
    eventually(async || *cb.text.lock().await == "explicit daemon copy").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(ca.writes.load(Ordering::SeqCst), 1);
    assert_eq!(cb.writes.load(Ordering::SeqCst), 1);
    ea.shutdown().await;
    eb.shutdown().await;
}

#[tokio::test]
async fn copy_local_skips_network_for_sensitive_content() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    let ca = Clipboard::default();
    let cb = Clipboard::default();
    let (ea, ta) = fixture(&a, &ca).await;
    let (eb, tb) = fixture(&b, &cb).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr = lb.local_addr();
    let ra = ea.clone();
    let rb = eb.clone();
    let _a = Task(tokio::spawn(async move { ra.run(la).await }));
    let _b = Task(tokio::spawn(async move { rb.run(lb).await }));
    tokio::time::sleep(Duration::from_millis(30)).await;
    ta.connect_peer(&b.peer(addr)).await.unwrap();
    eventually(async || tb.is_connected(a.id()).await).await;
    let secret = "ghp_1234567890abcdef1234567890abcdef1234";
    assert_eq!(ea.copy_local(secret.to_string()).await.unwrap(), 0);
    assert_eq!(*ca.text.lock().await, secret);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_ne!(*cb.text.lock().await, secret);
    ea.shutdown().await;
    eb.shutdown().await;
}

#[tokio::test]
async fn copy_local_holds_state_lock_while_clipboard_write_is_in_flight() {
    let a = Identity::new().await;
    let gated = GatedClipboard::new();
    let (ea, _) = fixture(&a, &gated).await;
    let engine = ea.clone();
    let copy = tokio::spawn(async move { engine.copy_local("held during write".into()).await });
    eventually(async || gated.set_started()).await;
    let capture_engine = ea.clone();
    let capture = tokio::spawn(async move { capture_engine.force_sync().await });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!capture.is_finished());
    gated.release_set();
    assert_eq!(copy.await.unwrap().unwrap(), 0);
    let _ = capture.await;
    assert_eq!(gated.writes(), 1);
    ea.shutdown().await;
}

#[tokio::test]
async fn copy_local_oversize_does_not_touch_clipboard() {
    let a = Identity::new().await;
    let ca = Clipboard::default();
    let (ea, _) = fixture(&a, &ca).await;
    let oversize = "x".repeat(5_242_881);
    assert!(ea.copy_local(oversize).await.is_err());
    assert_eq!(ca.writes.load(Ordering::SeqCst), 0);
    ea.shutdown().await;
}

struct Impersonator {
    signer: Arc<dyn Authenticator>,
    claimed: PublicKey,
}
#[async_trait::async_trait]
impl Authenticator for Impersonator {
    async fn identity_pkcs8(&self) -> Result<Vec<u8>, AuthError> {
        self.signer.identity_pkcs8().await
    }
    async fn trusted_keys(&self) -> Result<Vec<PublicKey>, AuthError> {
        self.signer.trusted_keys().await
    }
    async fn get_public_key(&self) -> Result<PublicKey, AuthError> {
        Ok(self.claimed.clone())
    }
    async fn is_authorized(&self, key: &PublicKey) -> Result<bool, AuthError> {
        self.signer.is_authorized(key).await
    }
    async fn authenticate_peer(&self, _: &PublicKey) -> Result<AuthToken, AuthError> {
        unreachable!()
    }
    async fn verify_token(&self, _: &AuthToken) -> Result<PeerId, AuthError> {
        unreachable!()
    }
}
#[tokio::test]
async fn reject_unknown_keys_public_key_only_impersonation_and_plaintext() {
    let server = Identity::new().await;
    let trusted = Identity::new().await;
    let attacker = Identity::new().await;
    server.trust(&trusted).await;
    attacker.trust(&server).await;
    let transport = WebSocketTransport::new(
        "127.0.0.1:0".parse().unwrap(),
        server.auth.clone(),
        websocket_config(),
        server.id(),
    );
    let mut listener = transport.listen().await.unwrap();
    let addr = listener.local_addr();
    let peer = server.peer(addr);
    let (client, accepted) = tokio::join!(
        WebSocketTransport::connect_to_peer(
            &peer,
            attacker.auth.clone(),
            websocket_config(),
            attacker.id()
        ),
        listener.accept()
    );
    assert!(client.is_err());
    assert!(accepted.is_err());
    let imposter = Arc::new(Impersonator {
        signer: attacker.auth.clone(),
        claimed: trusted.key.public_key(),
    });
    let peer = server.peer(addr);
    let (client, accepted) = tokio::join!(
        WebSocketTransport::connect_to_peer(&peer, imposter, websocket_config(), trusted.id()),
        listener.accept()
    );
    assert!(client.is_err());
    assert!(accepted.is_err());
    let (plain, accepted) = tokio::join!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/clipsync")),
        listener.accept()
    );
    assert!(plain.is_err());
    assert!(accepted.is_err());
}
fn remote(origin: uuid::Uuid, text: &str, sequence: u64, id: uuid::Uuid) -> Message {
    let mut message = Message::new(
        MessageType::ClipboardData,
        MessagePayload::Clipboard(clipsync::transport::ClipboardData {
            format: clipsync::transport::protocol::ClipboardFormat::Text,
            data: text.as_bytes().to_vec(),
            compression: None,
            checksum: clipsync::history::encryption::Encryptor::compute_checksum(text.as_bytes()),
            metadata: Default::default(),
        }),
    );
    message.source_peer_id = Some(origin);
    message.sequence = sequence;
    message.correlation_id = Some(id);
    message
}
#[tokio::test]
async fn stale_duplicate_failed_write_and_future_events_do_not_replace_clipboard() {
    let identity = Identity::new().await;
    let clipboard = Clipboard::default();
    let (engine, _) = fixture(&identity, &clipboard).await;
    let clock = chrono::Utc::now().timestamp_millis() as u64 + 1000;
    let origin = uuid::Uuid::new_v4();
    let current = remote(origin, "newer text", clock, uuid::Uuid::new_v4());
    engine.apply_remote(current.clone()).await.unwrap();
    engine
        .apply_remote(remote(
            origin,
            "stale text",
            clock - 1,
            uuid::Uuid::new_v4(),
        ))
        .await
        .unwrap();
    engine.apply_remote(current).await.unwrap();
    assert_eq!(*clipboard.text.lock().await, "newer text");
    assert_eq!(clipboard.writes.load(Ordering::SeqCst), 1);
    clipboard.fail.store(true, Ordering::SeqCst);
    let retry = remote(origin, "retry text", clock + 1, uuid::Uuid::new_v4());
    assert!(engine.apply_remote(retry.clone()).await.is_err());
    clipboard.fail.store(false, Ordering::SeqCst);
    engine.apply_remote(retry).await.unwrap();
    assert_eq!(*clipboard.text.lock().await, "retry text");
    assert!(engine
        .apply_remote(remote(
            origin,
            "future text",
            u64::MAX,
            uuid::Uuid::new_v4()
        ))
        .await
        .is_err());
    assert_eq!(*clipboard.text.lock().await, "retry text");
}

#[tokio::test]
async fn clipboard_payload_is_encrypted_on_the_wire_and_wrong_server_is_rejected() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::Mutex,
    };
    let server = Identity::new().await;
    let client = Identity::new().await;
    server.trust(&client).await;
    client.trust(&server).await;
    let transport = WebSocketTransport::new(
        "127.0.0.1:0".parse().unwrap(),
        server.auth.clone(),
        websocket_config(),
        server.id(),
    );
    let mut listener = transport.listen().await.unwrap();
    let server_addr = listener.local_addr();
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let bytes = captured.clone();
    let _proxy = Task(tokio::spawn(async move {
        let (mut incoming, _) = proxy.accept().await.unwrap();
        let mut outgoing = TcpStream::connect(server_addr).await.unwrap();
        let (mut ir, mut iw) = incoming.split();
        let (mut or, mut ow) = outgoing.split();
        let forward = async {
            let mut buf = [0; 4096];
            loop {
                let n = ir.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                bytes.lock().await.extend_from_slice(&buf[..n]);
                ow.write_all(&buf[..n]).await?;
            }
            Ok::<_, std::io::Error>(())
        };
        let reverse = tokio::io::copy(&mut or, &mut iw);
        let _ = tokio::join!(forward, reverse);
    }));
    let peer = server.peer(proxy_addr);
    let (connection, accepted) = tokio::join!(
        WebSocketTransport::connect_to_peer(
            &peer,
            client.auth.clone(),
            websocket_config(),
            client.id()
        ),
        listener.accept()
    );
    let mut connection = connection.unwrap();
    let mut accepted = accepted.unwrap();
    use clipsync::transport::Connection;
    let plaintext = "clipboard fixture that must never appear on the wire";
    connection
        .send(remote(client.id(), plaintext, 1, uuid::Uuid::new_v4()))
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), accepted.receive())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(received.payload, MessagePayload::Clipboard(ref p) if p.data == plaintext.as_bytes())
    );
    let wire = captured.lock().await;
    assert!(!wire
        .windows(plaintext.len())
        .any(|w| w == plaintext.as_bytes()));
    assert!(!wire.windows(13).any(|w| w == b"ClipboardData"));
    drop(wire);
    drop(connection);
    drop(accepted);
    // A different authorized server still cannot substitute for the discovered identity.
    let mut wrong_peer = server.peer(server_addr);
    wrong_peer.id = client.id();
    let (connection, accepted) = tokio::join!(
        WebSocketTransport::connect_to_peer(
            &wrong_peer,
            client.auth.clone(),
            websocket_config(),
            client.id()
        ),
        listener.accept()
    );
    assert!(connection.is_err());
    assert!(accepted.is_err());
}

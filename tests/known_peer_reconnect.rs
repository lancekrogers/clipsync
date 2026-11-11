mod support;

use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    config::Config,
    sync::{discovery_connect::connect_peers, known_peers::KnownPeers, SyncEngine},
    transport::{Listener, TransportConfig, TransportManager},
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use support::*;

fn transport(id: &Identity) -> Arc<TransportManager> {
    Arc::new(TransportManager::with_auth(
        TransportConfig {
            connect_timeout: Duration::from_secs(2),
            keepalive_interval: Duration::from_millis(100),
            ..Default::default()
        },
        id.auth.clone(),
        id.id(),
    ))
}

async fn engine(
    id: &Identity,
    clipboard: &Clipboard,
    transport: Arc<TransportManager>,
    known: Option<Arc<KnownPeers>>,
) -> Arc<SyncEngine> {
    let mut config = Config::default();
    config.node_id = id.id();
    config.listen_addr = "127.0.0.1:0".into();
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &id.dir.path().join("history.db"),
            &id.dir.path().join("history.key"),
        )
        .await
        .unwrap(),
    );
    let mut engine = SyncEngine::without_discovery(
        Arc::new(config),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        transport,
    );
    if let Some(known) = known {
        engine = engine.with_known_peers(known);
    }
    Arc::new(engine)
}

async fn serve(engine: &Arc<SyncEngine>) -> (SocketAddr, Task<anyhow::Result<()>>) {
    let listener = engine.bind_listener().await.unwrap();
    let addr = listener.local_addr();
    let run = engine.clone();
    (
        addr,
        Task(tokio::spawn(async move { run.run(listener).await })),
    )
}

/// Accepts TCP and hangs up, counting how often anything dials it.
async fn counting_listener() -> (SocketAddr, Arc<AtomicUsize>, Task<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let counted = accepts.clone();
    let task = Task(tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    }));
    (addr, accepts, task)
}

#[tokio::test]
async fn known_peer_is_redialed_after_restart_without_discovery() {
    let a = Identity::new().await;
    let b = Identity::new().await;
    a.trust(&b).await;
    b.trust(&a).await;
    // The lower id dials first, so no fallback grace delays the redial.
    let (dialer, server) = Identity::ordered_pair(&a, &b);
    let store_path = dialer.dir.path().join("known_peers.json");

    let server_clipboard = Clipboard::default();
    let server_transport = transport(server);
    let server_engine = engine(server, &server_clipboard, server_transport.clone(), None).await;
    let (server_addr, _serving) = serve(&server_engine).await;

    // First run: an ordinary dial authenticates and records the address.
    {
        let store = KnownPeers::open(&store_path);
        let first = transport(dialer);
        let mut retry = HashMap::new();
        connect_peers(
            &first,
            dialer.id(),
            &[server.peer(server_addr)],
            &mut retry,
            Instant::now(),
            Some(&store),
        )
        .await;
        assert!(first.is_connected(server.id()).await);
        first.shutdown().await;
    }
    let persisted = KnownPeers::open(&store_path)
        .get(server.id())
        .await
        .expect("address persisted after an authenticated dial");
    assert_eq!(persisted.addresses, vec![server_addr]);
    eventually(async || !server_transport.is_connected(dialer.id()).await).await;

    // Restart: new transport and engine, no discovery, only the persisted store.
    let dialer_clipboard = Clipboard::default();
    let restarted = transport(dialer);
    let dialer_engine = engine(
        dialer,
        &dialer_clipboard,
        restarted.clone(),
        Some(Arc::new(KnownPeers::open(&store_path))),
    )
    .await;
    let (_, _dialing) = serve(&dialer_engine).await;
    eventually(async || restarted.is_connected(server.id()).await).await;

    dialer_clipboard.copy("after restart").await;
    assert_eq!(dialer_engine.force_sync().await.unwrap(), 1);
    eventually(async || *server_clipboard.text.lock().await == "after restart").await;

    dialer_engine.shutdown().await;
    server_engine.shutdown().await;
}

#[tokio::test]
async fn revoked_peer_is_not_redialed_and_is_forgotten() {
    let mut ids = vec![
        Identity::new().await,
        Identity::new().await,
        Identity::new().await,
    ];
    // Lowest id dials both others without waiting for the fallback grace.
    ids.sort_by_key(|id| id.id());
    let (dialer, rest) = ids.split_first().unwrap();
    let (trusted, revoked) = (&rest[0], &rest[1]);
    dialer.trust(trusted).await;
    dialer.trust(revoked).await;
    assert!(dialer
        .auth
        .remove_peer(&revoked.key.public_key().fingerprint())
        .await
        .unwrap());

    let (trusted_addr, trusted_dials, _trusted_listener) = counting_listener().await;
    let (revoked_addr, revoked_dials, _revoked_listener) = counting_listener().await;
    let store_path = dialer.dir.path().join("known_peers.json");
    {
        let store = KnownPeers::open(&store_path);
        store
            .record(trusted.id(), "trusted", trusted_addr)
            .await
            .unwrap();
        store
            .record(revoked.id(), "revoked", revoked_addr)
            .await
            .unwrap();
    }

    let dialer_engine = engine(
        dialer,
        &Clipboard::default(),
        transport(dialer),
        Some(Arc::new(KnownPeers::open(&store_path))),
    )
    .await;
    let (_, _running) = serve(&dialer_engine).await;

    // Two dials of the trusted address span more than one full pass over the store.
    tokio::time::timeout(Duration::from_secs(15), async {
        while trusted_dials.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("trusted known peer was not redialed");
    assert_eq!(revoked_dials.load(Ordering::SeqCst), 0);

    let reopened = KnownPeers::open(&store_path);
    assert!(reopened.contains(trusted.id()).await);
    assert!(!reopened.contains(revoked.id()).await);
    dialer_engine.shutdown().await;
}

mod support;

use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    config::Config,
    discovery::PeerInfo,
    sync::{
        connection_policy::FALLBACK_INITIATOR_GRACE, discovery_connect::discovery_connect_peers,
        SyncEngine,
    },
    transport::{Connection, Listener, TransportConfig, TransportManager, WebSocketTransport},
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use support::*;

async fn fixture(id: &Identity, clipboard: &Clipboard) -> (Arc<SyncEngine>, Arc<TransportManager>) {
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

fn peer_with_addresses(id: uuid::Uuid, addrs: Vec<SocketAddr>) -> PeerInfo {
    let port = addrs.first().map(|a| a.port()).unwrap_or(0);
    PeerInfo {
        id,
        name: "peer".into(),
        addresses: addrs,
        port,
        version: "2.0.0".into(),
        platform: "test".into(),
        metadata: Default::default(),
        last_seen: 0,
    }
}

async fn sync_bidirectional(ea: &SyncEngine, eb: &SyncEngine, ca: &Clipboard, cb: &Clipboard) {
    ca.copy("alpha -> beta").await;
    assert_eq!(ea.force_sync().await.unwrap(), 1);
    eventually(async || *cb.text.lock().await == "alpha -> beta").await;
    cb.copy("beta -> alpha").await;
    assert_eq!(eb.force_sync().await.unwrap(), 1);
    eventually(async || *ca.text.lock().await == "beta -> alpha").await;
}

#[tokio::test]
async fn discovery_connect_prefers_lower_id_toward_only_reachable_listener() {
    let left = Identity::new().await;
    let right = Identity::new().await;
    left.trust(&right).await;
    right.trust(&left).await;
    let (low, high) = Identity::ordered_pair(&left, &right);

    let (engine_high, _transport_high) = fixture(high, &Clipboard::default()).await;
    let (_, transport_low) = fixture(low, &Clipboard::default()).await;
    let listener = engine_high.bind_listener().await.unwrap();
    let addr = listener.local_addr();
    let run = engine_high.clone();
    let _task = Task(tokio::spawn(async move { run.run(listener).await }));
    tokio::time::sleep(Duration::from_millis(40)).await;

    let peer = high.peer(addr);
    let mut retry = HashMap::new();
    discovery_connect_peers(
        &transport_low,
        low.id(),
        &[peer],
        &mut retry,
        std::time::Instant::now(),
    )
    .await;
    eventually(async || transport_low.is_connected(high.id()).await).await;
    engine_high.shutdown().await;
}

#[tokio::test]
async fn higher_id_fallback_reaches_only_lower_listener() {
    let left = Identity::new().await;
    let right = Identity::new().await;
    left.trust(&right).await;
    right.trust(&left).await;
    let (low, high) = Identity::ordered_pair(&left, &right);

    let (engine_low, transport_low) = fixture(low, &Clipboard::default()).await;
    let (_, transport_high) = fixture(high, &Clipboard::default()).await;
    let listener = engine_low.bind_listener().await.unwrap();
    let addr = listener.local_addr();
    let run = engine_low.clone();
    let _task = Task(tokio::spawn(async move { run.run(listener).await }));
    tokio::time::sleep(Duration::from_millis(40)).await;

    let now = std::time::Instant::now();
    let mut retry = HashMap::new();
    let peers = [low.peer(addr)];
    discovery_connect_peers(&transport_high, high.id(), &peers, &mut retry, now).await;
    assert!(!transport_high.is_connected(low.id()).await);
    discovery_connect_peers(
        &transport_high,
        high.id(),
        &peers,
        &mut retry,
        now + FALLBACK_INITIATOR_GRACE,
    )
    .await;
    assert!(transport_high.is_connected(low.id()).await);
    eventually(async || transport_low.is_connected(high.id()).await).await;
    engine_low.shutdown().await;
}

#[tokio::test]
async fn simultaneous_dial_settles_and_syncs_both_ways() {
    let left = Identity::new().await;
    let right = Identity::new().await;
    left.trust(&right).await;
    right.trust(&left).await;
    let ca = Clipboard::default();
    let cb = Clipboard::default();
    let (ea, ta) = fixture(&left, &ca).await;
    let (eb, tb) = fixture(&right, &cb).await;
    let la = ea.bind_listener().await.unwrap();
    let lb = eb.bind_listener().await.unwrap();
    let addr_a = la.local_addr();
    let addr_b = lb.local_addr();
    let ra = ea.clone();
    let rb = eb.clone();
    let _a = Task(tokio::spawn(async move { ra.run(la).await }));
    let _b = Task(tokio::spawn(async move { rb.run(lb).await }));
    tokio::time::sleep(Duration::from_millis(30)).await;

    let peer_b = right.peer(addr_b);
    let peer_a = left.peer(addr_a);
    let (r1, r2) = tokio::join!(ta.connect_peer(&peer_b), tb.connect_peer(&peer_a));
    assert!(r1.is_ok() && r2.is_ok());
    eventually(async || ta.is_connected(right.id()).await && tb.is_connected(left.id()).await)
        .await;
    sync_bidirectional(&ea, &eb, &ca, &cb).await;

    tb.shutdown().await;
    eventually(async || !ta.is_connected(right.id()).await).await;
    ta.connect_peer(&right.peer(addr_b)).await.unwrap();
    eventually(async || tb.is_connected(left.id()).await).await;
    sync_bidirectional(&ea, &eb, &ca, &cb).await;

    ea.shutdown().await;
    eb.shutdown().await;
}

#[tokio::test]
async fn stalled_tls_first_candidate_falls_back_to_working_listener() {
    use tokio::net::TcpListener;

    let server = Identity::new().await;
    let client = Identity::new().await;
    server.trust(&client).await;
    client.trust(&server).await;

    let stall_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stall_addr = stall_listener.local_addr().unwrap();
    let _stall = Task(tokio::spawn(async move {
        let (stream, _) = stall_listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(stream);
    }));

    let transport = WebSocketTransport::new(
        "127.0.0.1:0".parse().unwrap(),
        server.auth.clone(),
        clipsync::transport::websocket::WebSocketConfig {
            connect_timeout: Duration::from_secs(2),
            ..Default::default()
        },
        server.id(),
    );
    let mut listener = transport.listen().await.unwrap();
    let good = listener.local_addr();
    let _serve = Task(tokio::spawn(async move {
        let _ = listener.accept().await;
    }));

    let peer = peer_with_addresses(server.id(), vec![stall_addr, good]);
    let connection = WebSocketTransport::connect_to_peer(
        &peer,
        client.auth.clone(),
        clipsync::transport::websocket::WebSocketConfig {
            connect_timeout: Duration::from_secs(2),
            ..Default::default()
        },
        client.id(),
    )
    .await
    .expect("should succeed via second candidate");
    assert_eq!(connection.peer_info().id, server.id());
}

#[tokio::test]
async fn aborted_dial_releases_in_flight_lease() {
    use tokio::net::TcpListener;

    let server = Identity::new().await;
    let client = Identity::new().await;
    server.trust(&client).await;
    client.trust(&server).await;

    let stall_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stall_addr = stall_listener.local_addr().unwrap();
    let _stall = Task(tokio::spawn(async move {
        let (stream, _) = stall_listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(stream);
    }));

    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig {
            connect_timeout: Duration::from_secs(3),
            ..Default::default()
        },
        client.auth.clone(),
        client.id(),
    ));
    let peer = peer_with_addresses(server.id(), vec![stall_addr]);
    let slow = transport.clone();
    let slow_peer = peer.clone();
    let handle = tokio::spawn(async move { slow.connect_peer(&slow_peer).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    handle.abort();
    let _ = handle.await;

    let good_transport = WebSocketTransport::new(
        "127.0.0.1:0".parse().unwrap(),
        server.auth.clone(),
        clipsync::transport::websocket::WebSocketConfig {
            connect_timeout: Duration::from_secs(2),
            ..Default::default()
        },
        server.id(),
    );
    let mut listener = good_transport.listen().await.unwrap();
    let good = listener.local_addr();
    let _serve = Task(tokio::spawn(async move {
        let _ = listener.accept().await;
    }));
    let peer = peer_with_addresses(server.id(), vec![good]);
    transport.connect_peer(&peer).await.unwrap();
}

#[tokio::test]
async fn connect_failure_mentions_endpoint_not_disk_space() {
    let client = Identity::new().await;
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig {
            connect_timeout: Duration::from_millis(600),
            ..Default::default()
        },
        client.auth.clone(),
        client.id(),
    ));
    let peer = peer_with_addresses(
        uuid::Uuid::new_v4(),
        vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1)],
    );
    let err = transport.connect_peer(&peer).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("127.0.0.1") || message.contains("connect"));
    assert!(!message.to_lowercase().contains("disk space"));
}

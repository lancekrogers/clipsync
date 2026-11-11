//! Real mDNS discovery + code pairing regression (disposable Linux only).
//!
//! Scope limit: two enrollment managers / processes in the **same** disposable
//! container with real sockets and multicast — **not** a physical two-host LAN proof.
//! Ordinary `cargo test` skips this file (`#[ignore]`); root Linux runs execute with
//! `--ignored` under `CLIPSYNC_DISPOSABLE_DESKTOP=1`.
//!
//! Temp identities only. No `inject_candidate`. No host mDNS. No personal config.
mod support;

use clipsync::{
    auth::{Authenticator, AuthorizedKeys},
    config::Config,
    control::EnrollmentCandidateView,
    enrollment::EnrollmentManager,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use support::*;

const DISCOVERY_BUDGET: Duration = Duration::from_secs(30);

fn require_disposable_linux() {
    assert_eq!(
        std::env::var("CLIPSYNC_DISPOSABLE_DESKTOP").as_deref(),
        Ok("1"),
        "companion_discovery requires CLIPSYNC_DISPOSABLE_DESKTOP=1 (disposable container)"
    );
    assert!(
        cfg!(target_os = "linux"),
        "companion_discovery requires Linux at runtime (got {})",
        std::env::consts::OS
    );
}

/// Reserve an ephemeral TCP port on 0.0.0.0, then release it for EnrollmentManager bind.
///
/// Note: `EnrollmentManager::set_listen_port_override(0)` with an unspecified listen IP
/// remaps to 127.0.0.1 (see `resolve_listen_addr` in `src/enrollment/manager.rs`). A
/// non-zero override keeps the configured 0.0.0.0 bind for real-interface advertise.
async fn reserve_ephemeral_all_ifaces_port() -> u16 {
    let probe =
        tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))
            .await
            .expect("probe bind 0.0.0.0:0");
    let port = probe.local_addr().expect("probe local_addr").port();
    drop(probe);
    port
}

async fn manager_on_all_ifaces(id: &Identity, label: &str) -> (Arc<EnrollmentManager>, u16) {
    let port = reserve_ephemeral_all_ifaces_port().await;
    let mut config = Config::default();
    config.node_id = id.id();
    config.listen_addr = format!("0.0.0.0:{port}");
    config.auth.ssh_key = id.dir.path().join("identity");
    config.auth.authorized_keys = id.dir.path().join("authorized_keys");
    config.clipboard.history_db = id.dir.path().join("history.db");
    config.advertise_name = format!("{label}-{}", &id.id().to_string()[..8]);

    let manager = Arc::new(EnrollmentManager::new(Arc::new(config), id.auth.clone()).unwrap());
    // Non-zero override preserves unspecified IP; override 0 would force localhost.
    manager.set_listen_port_override(port).await;
    (manager, port)
}

async fn wait_mdns_peer(
    mgr: &EnrollmentManager,
    peer_id: uuid::Uuid,
    peer_fp: &str,
    label: &str,
) -> EnrollmentCandidateView {
    let deadline = Instant::now() + DISCOVERY_BUDGET;
    loop {
        let snap = mgr.discover().await.expect("discover");
        if let Some(c) = snap.candidates.iter().find(|c| {
            c.source == "mdns"
                && c.advertised_node_id == Some(peer_id)
                && c.advertised_fingerprint.as_deref() == Some(peer_fp)
        }) {
            return c.clone();
        }
        if Instant::now() >= deadline {
            panic!(
                "{label}: no mDNS candidate for peer {peer_id} (fp {peer_fp}) within {:?} — \
                 multicast/_clipsync-enroll._tcp discovery failed or unavailable. \
                 candidates={:?}",
                DISCOVERY_BUDGET, snap.candidates
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_succeeded(mgr: &EnrollmentManager, session_id: uuid::Uuid) {
    for _ in 0..200 {
        let snap = mgr.inspect(Some(session_id)).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| s.session_id == session_id) {
            if s.state == "Succeeded" {
                return;
            }
            if matches!(s.state.as_str(), "Failed" | "Cancelled" | "Expired") {
                panic!(
                    "session {session_id} ended as {} before Succeeded: {:?}",
                    s.state, s.error
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("session {session_id} did not reach Succeeded");
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

/// B allows a device and advertises `_clipsync-enroll._tcp` on 0.0.0.0. A,
/// with no window of its own, finds B over real mDNS (no inject), types B's
/// code with the advertised fingerprint hint → both Succeeded + durable pins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "disposable Linux only: CLIPSYNC_DISPOSABLE_DESKTOP=1 + real mDNS multicast"]
async fn mdns_discovery_and_code_pairing_regression() {
    require_disposable_linux();

    let a = Identity::new().await;
    let b = Identity::new().await;
    let fp_a = a.key.public_key().fingerprint();
    let fp_b = b.key.public_key().fingerprint();
    assert_ne!(a.id(), b.id(), "identities must be distinct");
    assert_ne!(fp_a, fp_b, "fingerprints must be distinct");

    let (ma, _) = manager_on_all_ifaces(&a, "manager-a").await;
    let (mb, port_b) = manager_on_all_ifaces(&b, "manager-b").await;
    let open = mb.open().await.expect("enrollment open");
    let addr_b: SocketAddr = open.listen_addr.as_ref().unwrap().parse().unwrap();
    assert!(
        addr_b.ip().is_unspecified(),
        "expected 0.0.0.0, got {addr_b}"
    );
    assert_eq!(addr_b.port(), port_b);
    let code = open.code.expect("an open window shows a code").code;
    assert!(
        !open.listen_hints.is_empty(),
        "a wildcard bind must offer LAN addresses to type"
    );

    // Real mDNS, browse only: A never opens a window.
    let cand_for_b = wait_mdns_peer(&ma, b.id(), &fp_b, "manager-a→b").await;
    assert_eq!(cand_for_b.source, "mdns");
    assert!(!ma.inspect(None).await.unwrap().listening);
    assert_eq!(
        cand_for_b.advertised_fingerprint.as_deref(),
        Some(fp_b.as_str()),
        "mDNS ssh_fp hint must match B TLS fingerprint"
    );
    let dial_addr: SocketAddr = cand_for_b.address.parse().expect("candidate address parse");
    assert_eq!(
        dial_addr.port(),
        addr_b.port(),
        "discovered candidate port must match B listen port"
    );

    for id in [&a, &b] {
        assert!(
            AuthorizedKeys::load_from_file(&id.dir.path().join("authorized_keys"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    let sid_a = ma
        .pair(
            &cand_for_b.address,
            code,
            cand_for_b.advertised_fingerprint.clone(),
            cand_for_b.advertised_node_id,
        )
        .await
        .expect("pair")
        .requested_session_id
        .unwrap();

    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
    wait_succeeded(&ma, sid_a).await;
    let sid_b = mb
        .inspect(None)
        .await
        .unwrap()
        .sessions
        .iter()
        .find(|s| s.role == "acceptor")
        .expect("allowing side session")
        .session_id;
    wait_succeeded(&mb, sid_b).await;

    let keys_a = AuthorizedKeys::load_from_file(&a.dir.path().join("authorized_keys"))
        .await
        .unwrap();
    let keys_b = AuthorizedKeys::load_from_file(&b.dir.path().join("authorized_keys"))
        .await
        .unwrap();
    assert!(
        keys_a.is_authorized(&b.key.public_key()),
        "A authorized_keys missing B pin"
    );
    assert!(
        keys_b.is_authorized(&a.key.public_key()),
        "B authorized_keys missing A pin"
    );

    let _ = ma.cancel(None).await;
    let _ = mb.cancel(None).await;
}

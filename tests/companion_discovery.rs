//! Real mDNS enrollment discovery + dial mutual-trust regression (disposable Linux only).
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

async fn open_manager_on_all_ifaces(
    id: &Identity,
    label: &str,
) -> (Arc<EnrollmentManager>, SocketAddr) {
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
    let snap = manager.open().await.expect("enrollment open");
    assert!(snap.listening, "{label}: expected listening after open");
    let addr: SocketAddr = snap
        .listen_addr
        .as_ref()
        .unwrap_or_else(|| panic!("{label}: missing listen_addr"))
        .parse()
        .unwrap_or_else(|e| panic!("{label}: parse listen_addr: {e}"));
    assert!(
        addr.ip().is_unspecified(),
        "{label}: expected bind 0.0.0.0 (unspecified), got {addr} — \
         if override was 0, see resolve_listen_addr localhost remap in \
         src/enrollment/manager.rs"
    );
    assert_eq!(addr.port(), port, "{label}: ephemeral port mismatch");
    (manager, addr)
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

fn dialing_session_id(snap: &clipsync::control::EnrollmentSnapshot) -> uuid::Uuid {
    snap.sessions
        .iter()
        .find(|s| s.state == "Dialing")
        .map(|s| s.session_id)
        .expect("dial snapshot should include Dialing session")
}

/// Two managers advertise `_clipsync-enroll._tcp` on 0.0.0.0 ephemeral ports,
/// discover each other via real mDNS (no inject), dial with advertised TLS
/// fingerprint hint, mutual confirm → both Succeeded + durable pins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "disposable Linux only: CLIPSYNC_DISPOSABLE_DESKTOP=1 + real mDNS multicast"]
async fn mdns_discovery_and_mutual_trust_regression() {
    require_disposable_linux();

    let a = Identity::new().await;
    let b = Identity::new().await;
    let fp_a = a.key.public_key().fingerprint();
    let fp_b = b.key.public_key().fingerprint();
    assert_ne!(a.id(), b.id(), "identities must be distinct");
    assert_ne!(fp_a, fp_b, "fingerprints must be distinct");

    let (ma, addr_a) = open_manager_on_all_ifaces(&a, "manager-a").await;
    let (mb, addr_b) = open_manager_on_all_ifaces(&b, "manager-b").await;
    assert_ne!(addr_a.port(), addr_b.port(), "ephemeral ports must differ");

    // Real mDNS: each must see the other as source=mdns with correct TLS fp hint.
    let cand_for_b = wait_mdns_peer(&ma, b.id(), &fp_b, "manager-a→b").await;
    let cand_for_a = wait_mdns_peer(&mb, a.id(), &fp_a, "manager-b→a").await;

    assert_eq!(cand_for_b.source, "mdns");
    assert_eq!(cand_for_a.source, "mdns");
    assert_eq!(
        cand_for_b.advertised_fingerprint.as_deref(),
        Some(fp_b.as_str()),
        "mDNS ssh_fp hint must match B TLS fingerprint"
    );
    assert_eq!(
        cand_for_a.advertised_fingerprint.as_deref(),
        Some(fp_a.as_str()),
        "mDNS ssh_fp hint must match A TLS fingerprint"
    );

    // Dial address must be reachable; prefer discovered address, require correct port.
    let dial_addr: SocketAddr = cand_for_b.address.parse().expect("candidate address parse");
    assert_eq!(
        dial_addr.port(),
        addr_b.port(),
        "discovered candidate port must match B listen port"
    );

    // No trust before mutual confirmation.
    assert!(
        !a.auth.is_authorized(&b.key.public_key()).await.unwrap(),
        "A must not trust B before confirm"
    );
    assert!(
        !b.auth.is_authorized(&a.key.public_key()).await.unwrap(),
        "B must not trust A before confirm"
    );
    assert!(
        AuthorizedKeys::load_from_file(&a.dir.path().join("authorized_keys"))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        AuthorizedKeys::load_from_file(&b.dir.path().join("authorized_keys"))
            .await
            .unwrap()
            .is_empty()
    );

    // Dial with advertised fingerprint hint (mismatch would fail bind).
    let dial_snap = ma
        .dial(
            &cand_for_b.address,
            cand_for_b.advertised_fingerprint.clone(),
            cand_for_b.advertised_node_id,
        )
        .await
        .expect("dial");
    let _dialing = dialing_session_id(&dial_snap);

    let sid_a = wait_bound(&ma, &fp_b).await;
    let sid_b = wait_bound(&mb, &fp_a).await;

    // Still no durable trust until both confirm.
    assert!(!a.auth.is_authorized(&b.key.public_key()).await.unwrap());
    assert!(!b.auth.is_authorized(&a.key.public_key()).await.unwrap());

    ma.confirm(sid_a).await.expect("confirm A");
    mb.confirm(sid_b).await.expect("confirm B");

    wait_authorized(&a.auth, &b.key.public_key()).await;
    wait_authorized(&b.auth, &a.key.public_key()).await;
    wait_succeeded(&ma, sid_a).await;
    wait_succeeded(&mb, sid_b).await;

    // Durable pins on disk.
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

    // Clean manager cancellation (stop listeners / mDNS / sessions).
    let _ = ma.cancel(None).await;
    let _ = mb.cancel(None).await;
}

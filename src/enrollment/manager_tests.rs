//! Force the peer-ack-before-local-flush ordering without network scheduling luck.
use super::*;
use crate::auth::{AuthConfig, KeyPair, KeyType};

async fn waiting_session() -> (tempfile::TempDir, EnrollmentManager, Uuid, Uuid, String) {
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate(KeyType::Ed25519).unwrap();
    let local_fp = key.public_key().fingerprint();
    let identity = dir.path().join("identity");
    key.save_to_file(&identity).await.unwrap();
    let auth = Arc::new(
        SshAuthenticator::new(AuthConfig {
            private_key_path: identity,
            authorized_keys_path: dir.path().join("authorized"),
            generate_if_missing: false,
        })
        .await
        .unwrap(),
    );
    let manager = EnrollmentManager::new(Arc::new(Config::default()), auth).unwrap();
    let session_id = Uuid::new_v4();
    let peer_session_id = Uuid::new_v4();
    let gate = Arc::new(CommitGate::new());
    gate.finish_commit();
    let mut session = LiveSession::new(
        session_id,
        Role::Initiator,
        SessionState::WaitingPeerAuth,
        local_fp.clone(),
    );
    session.peer_session_id = Some(peer_session_id);
    session.peer_fp = Some("peer".into());
    session.verified = true;
    session.local_persisted = true;
    session.commit_gate = gate;
    session.expires_at = Instant::now() + Duration::from_secs(30);
    session.event_task = Some(tokio::spawn(std::future::pending()));
    manager
        .inner
        .write()
        .await
        .sessions
        .insert(session_id, session);
    (dir, manager, session_id, peer_session_id, local_fp)
}

#[tokio::test]
async fn peer_ack_cannot_abort_local_writer_before_its_flush() {
    let (_dir, manager, sid, peer_sid, local_fp) = waiting_session().await;
    EnrollmentManager::handle_persist_ack(&manager.inner, sid, peer_sid, sid, &local_fp).await;
    {
        let guard = manager.inner.read().await;
        let session = &guard.sessions[&sid];
        assert!(matches!(session.state, SessionState::WaitingPeerAuth));
        assert!(session.peer_persisted_ack);
        assert!(
            session.event_task.is_some(),
            "must retain I/O until local ack flush"
        );
    }
    EnrollmentManager::complete_after_ack_flush(&manager.inner, sid, "peer", true).await;
    let guard = manager.inner.read().await;
    assert!(matches!(
        guard.sessions[&sid].state,
        SessionState::Succeeded
    ));
    assert!(guard.sessions[&sid].event_task.is_none());
}

#[tokio::test]
async fn failed_local_ack_is_incomplete_even_when_peer_ack_arrived() {
    let (_dir, manager, sid, peer_sid, local_fp) = waiting_session().await;
    EnrollmentManager::handle_persist_ack(&manager.inner, sid, peer_sid, sid, &local_fp).await;
    EnrollmentManager::complete_after_ack_flush(&manager.inner, sid, "peer", false).await;
    let guard = manager.inner.read().await;
    assert!(!matches!(
        guard.sessions[&sid].state,
        SessionState::Succeeded
    ));
    assert!(guard.sessions[&sid].local_persisted);
    assert!(!guard.sessions[&sid].local_ack_flushed);
    assert_eq!(guard.last_outcome.as_ref().unwrap().status, "incomplete");
}

/// Each take consumes the code on screen and shows a fresh one; the window's
/// budget bounds how many codes are ever handed out.
#[tokio::test]
async fn every_burn_hands_out_a_distinct_code_until_the_budget_is_spent() {
    let (_dir, manager, _, _, _) = waiting_session().await;
    let mut g = manager.inner.write().await;
    g.listening = true;
    g.window_expires = Some(Instant::now() + Duration::from_secs(60));
    g.attempts_used = 0;
    EnrollmentManager::issue_code_locked(&mut g, None);
    let mut taken = Vec::new();
    for _ in 0..MAX_PAIRING_ATTEMPTS {
        let shown = g.code.as_ref().map(|live| live.code.digits().to_string());
        let code = EnrollmentManager::take_code_locked(&mut g).unwrap();
        assert_eq!(Some(code.digits().to_string()), shown);
        taken.push(code.digits().to_string());
        if let Some(next) = g.code.as_ref() {
            assert_ne!(next.code.digits(), code.digits());
        }
    }
    assert_eq!(taken.len() as u32, MAX_PAIRING_ATTEMPTS);
    assert!(g.code.is_none());
    assert!(matches!(
        EnrollmentManager::take_code_locked(&mut g),
        Err(PairingFailure::NoCode)
    ));
    g.listening = false;
    assert!(matches!(
        EnrollmentManager::take_code_locked(&mut g),
        Err(PairingFailure::NotListening)
    ));
}

async fn temp_auth(dir: &std::path::Path) -> Arc<SshAuthenticator> {
    let key = KeyPair::generate(KeyType::Ed25519).unwrap();
    let identity = dir.join("identity");
    key.save_to_file(&identity).await.unwrap();
    Arc::new(
        SshAuthenticator::new(AuthConfig {
            private_key_path: identity,
            authorized_keys_path: dir.join("authorized"),
            generate_if_missing: false,
        })
        .await
        .unwrap(),
    )
}

fn loopback_config() -> Arc<Config> {
    Arc::new(Config {
        listen_addr: "127.0.0.1:0".into(),
        ..Config::default()
    })
}

/// A window that closes on its timer ends attempts still checking its codes:
/// a late but correct confirmation gets no reply, nothing is saved, and a
/// window opened afterwards stays open.
#[tokio::test]
async fn an_expired_window_ends_its_attempts_in_flight() {
    let (a_dir, j_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a_auth, j_auth) = (temp_auth(a_dir.path()).await, temp_auth(j_dir.path()).await);
    let allowing = EnrollmentManager::new(loopback_config(), a_auth.clone()).unwrap();
    allowing.set_listen_port_override(0).await;
    allowing
        .set_window_ttl_override(Duration::from_millis(600))
        .await;
    let open = allowing.open().await.unwrap();
    let addr: SocketAddr = open.listen_addr.unwrap().parse().unwrap();
    let code = open.code.unwrap().code;

    // Drive the joining side by hand so it can stall between its messages.
    let mut conn = channel::dial(addr, j_auth.clone()).await.ok().unwrap();
    let pake = Pake::start(Side::Joining, code);
    let our_msg = pake.message().to_vec();
    let nonce = protocol::random_nonce();
    let sid = Uuid::new_v4();
    conn.write_msg(&EnrollMessage::Hello {
        session_id: sid,
        nonce_hex: hex::encode(nonce),
        display_name: "late joiner".into(),
        pake_hex: hex::encode(&our_msg),
    })
    .await
    .unwrap();
    let EnrollMessage::HelloAck {
        session_id: their_id,
        nonce_hex,
        pake_hex,
        ..
    } = conn.read_msg().await.unwrap()
    else {
        panic!("no HelloAck")
    };
    let their_msg = hex::decode(pake_hex).unwrap();
    let their_nonce = decode_nonce(&nonce_hex).unwrap();
    let key = pake.finish(&their_msg).unwrap();

    tokio::time::sleep(Duration::from_millis(900)).await;
    let snap = allowing.inspect(None).await.unwrap();
    assert!(!snap.listening);
    assert_eq!(snap.outcome.unwrap().status, "expired");
    assert!(snap
        .sessions
        .iter()
        .any(|s| s.role == "acceptor" && s.state == "Expired"));
    allowing
        .set_window_ttl_override(Duration::from_secs(600))
        .await;
    assert!(allowing.open().await.unwrap().code.is_some());

    let local_fp = j_auth.get_public_key().await.unwrap().fingerprint();
    let peer_fp = conn.peer_key.fingerprint();
    let binding = conn.binding;
    let transcript = Transcript {
        allowing_session: their_id,
        joining_session: sid,
        allowing_nonce: &their_nonce,
        joining_nonce: &nonce,
        allowing_fingerprint: &peer_fp,
        joining_fingerprint: &local_fp,
        allowing_message: &their_msg,
        joining_message: &our_msg,
        channel_binding: &binding,
    };
    conn.write_msg(&EnrollMessage::KeyConfirm {
        session_id: sid,
        peer_session_id: their_id,
        mac_hex: hex::encode(key.confirmation(&transcript).unwrap()),
    })
    .await
    .unwrap();
    assert!(conn.read_msg().await.is_err(), "an ended attempt replied");
    let joiner = j_auth.get_public_key().await.unwrap();
    assert!(!a_auth.is_authorized(&joiner).await.unwrap());
    let snap = allowing.inspect(None).await.unwrap();
    assert!(
        snap.listening && snap.code.is_some(),
        "the new window closed"
    );
}

/// An accept loop whose window was closed under it exits instead of taking
/// over the listener of a window opened later, so closing that window frees
/// the port.
#[tokio::test]
async fn an_accept_loop_never_adopts_a_later_window() {
    let dir = tempfile::tempdir().unwrap();
    let manager = EnrollmentManager::new(loopback_config(), temp_auth(dir.path()).await).unwrap();
    manager.set_listen_port_override(0).await;
    let first: SocketAddr = manager
        .open()
        .await
        .unwrap()
        .listen_addr
        .unwrap()
        .parse()
        .unwrap();
    let later = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
    let later_addr = later.local_addr().unwrap();
    {
        // As when the loop closed its own window (it is not aborted) and a
        // new window opened before it looked again.
        let mut g = manager.inner.write().await;
        let _detached = g.accept_task.take();
        g.listener = Some(later);
    }
    // Wake the loop if it is still waiting on the first listener.
    let _ = tokio::net::TcpStream::connect(first).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    manager.cancel(None).await.unwrap();
    let mut freed = false;
    for _ in 0..100 {
        if std::net::TcpListener::bind(later_addr).is_ok() {
            freed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        freed,
        "a stale accept loop still holds the later window's port"
    );
}

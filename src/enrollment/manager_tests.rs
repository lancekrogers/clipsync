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

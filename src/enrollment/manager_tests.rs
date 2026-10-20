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
    manager.inner.write().await.sessions.insert(
        session_id,
        LiveSession {
            session_id,
            role: Role::Initiator,
            state: SessionState::WaitingPeerAuth,
            local_fp: local_fp.clone(),
            local_nonce: [1; 32],
            peer_session_id: Some(peer_session_id),
            peer_fp: Some("peer".into()),
            peer_key: None,
            peer_nonce: Some([2; 32]),
            peer_node_id: None,
            peer_display_name: None,
            dialed_addr: None,
            local_confirmed: true,
            peer_confirmed: true,
            local_persisted: true,
            local_ack_flushed: false,
            peer_persisted_ack: false,
            commit_gate: gate,
            expires_at: Instant::now() + Duration::from_secs(30),
            error: None,
            io: None,
            event_task: Some(tokio::spawn(std::future::pending())),
        },
    );
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

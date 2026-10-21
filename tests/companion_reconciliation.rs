//! Controller reconciliation regressions against a private daemon fixture.
//! Temp identity/history only — no personal pasteboard, no live service.
mod support;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    companion::{
        client::{Controller, Intent},
        model::{Connection, Route},
    },
    config::Config,
    control::{self, EnrollmentTarget},
    enrollment::EnrollmentManager,
    sync::SyncEngine,
    transport::{TransportConfig, TransportManager},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use support::{Clipboard, Identity, Task};

async fn wait(client: &mut Controller, predicate: impl Fn(&Controller) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        client.tick();
        if predicate(client) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting for controller state: {:?}",
            client.view
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct Fixture {
    config: Config,
    clipboard: Clipboard,
    _engine: Arc<SyncEngine>,
    enrollment: Arc<EnrollmentManager>,
    task: Task<anyhow::Result<()>>,
    _identity: Identity,
}
async fn fixture() -> Fixture {
    let identity = Identity::new().await;
    let mut config = Config::default();
    config.node_id = identity.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.auth.ssh_key = identity.dir.path().join("identity");
    config.auth.authorized_keys = identity.dir.path().join("authorized_keys");
    config.clipboard.history_db = identity.dir.path().join("history.db");
    config.clipboard.history_key = Some(identity.dir.path().join("history.key"));
    let clipboard = Clipboard::default();
    let history = Arc::new(
        HistoryManager::new_with_key_path(
            &config.clipboard.history_db,
            config.clipboard.history_key.as_ref().unwrap(),
        )
        .await
        .unwrap(),
    );
    let engine = Arc::new(SyncEngine::without_discovery(
        Arc::new(config.clone()),
        Arc::new(ClipboardProviderWrapper::new(Box::new(clipboard.clone()))),
        history,
        Arc::new(TransportManager::with_auth(
            TransportConfig::default(),
            identity.auth.clone(),
            identity.id(),
        )),
    ));
    engine
        .copy_local("first retained copy".into())
        .await
        .unwrap();
    engine
        .copy_local("second retained copy".into())
        .await
        .unwrap();
    let enrollment =
        Arc::new(EnrollmentManager::new(Arc::new(config.clone()), identity.auth.clone()).unwrap());
    enrollment.set_listen_port_override(0).await;
    let target = EnrollmentTarget::new(engine.clone(), enrollment.clone());
    let server = control::Server::bind(control::socket_path(&config), &config).unwrap();
    let task = Task(tokio::spawn(async move { server.run(&target).await }));
    Fixture {
        config,
        clipboard,
        _engine: engine,
        enrollment,
        task,
        _identity: identity,
    }
}

#[tokio::test]
async fn devices_poll_while_enrollment_closed_stays_quiet() {
    let Fixture {
        config,
        clipboard: _clipboard,
        _engine,
        enrollment,
        task: _task,
        _identity,
    } = fixture().await;
    assert!(!enrollment.inspect(None).await.unwrap().listening);
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |c| c.view.can_act()).await;
    assert!(client.view.enrollment_supported);
    client.dispatch(Intent::Navigate(Route::Devices));
    wait(&mut client, |c| c.view.enrollment.is_some()).await;
    assert!(!client.view.enrollment.as_ref().unwrap().listening);
    assert!(client.view.error.is_none(), "{:?}", client.view.error);
    assert!(client.view.can_act());
}

#[tokio::test]
async fn copy_stays_disabled_until_current_detail_arrives() {
    let Fixture {
        config,
        clipboard,
        _engine,
        enrollment: _enrollment,
        task: _task,
        _identity,
    } = fixture().await;
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |c| c.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |c| c.view.history.len() >= 2).await;
    let id = client.view.history[0].id;
    client.dispatch(Intent::Select(id));
    assert!(client.view.detail.is_none());
    assert!(client.view.detail_loading);
    // Before detail: CopySelected must no-op (pasteboard unchanged).
    clipboard.copy("sentinel-before-detail").await;
    client.dispatch(Intent::CopySelected);
    assert!(!client.view.action_pending);
    assert_eq!(*clipboard.text.lock().await, "sentinel-before-detail");
    wait(&mut client, |c| {
        c.view.detail.is_some() && !c.view.detail_loading
    })
    .await;
    assert_eq!(client.view.selected, Some(id));
    client.dispatch(Intent::CopySelected);
    wait(&mut client, |c| {
        !c.view.action_pending
            && c.view
                .notice
                .as_deref()
                .is_some_and(|text| text.contains("Copied locally"))
    })
    .await;
    assert_eq!(
        *clipboard.text.lock().await,
        client.view.detail.as_deref().unwrap()
    );
}

#[tokio::test]
async fn rapid_reselection_keeps_detail_bound_to_current_row() {
    let Fixture {
        config,
        clipboard: _clipboard,
        _engine,
        enrollment: _enrollment,
        task: _task,
        _identity,
    } = fixture().await;
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |c| c.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |c| c.view.history.len() >= 2).await;
    let first = client.view.history[0].id;
    let second = client.view.history[1].id;
    client.dispatch(Intent::Select(first));
    client.dispatch(Intent::Select(second));
    wait(&mut client, |c| {
        c.view.selected == Some(second) && c.view.detail.is_some() && !c.view.detail_loading
    })
    .await;
    assert_eq!(client.view.selected, Some(second));
    assert!(
        client
            .view
            .detail
            .as_deref()
            .is_some_and(|text| text.contains("retained copy")),
        "detail should be full text for the selected row: {:?}",
        client.view.detail
    );
    // Copy still gated on current detail (not the abandoned first selection).
    assert!(client.view.can_act());
    assert!(client.view.detail.is_some());
}

#[tokio::test]
async fn stale_status_disables_actions_including_copy() {
    let Fixture {
        config,
        clipboard: _clipboard,
        _engine,
        enrollment: _enrollment,
        task,
        _identity,
    } = fixture().await;
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |c| c.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::History));
    wait(&mut client, |c| !c.view.history.is_empty()).await;
    let id = client.view.history[0].id;
    client.dispatch(Intent::Select(id));
    wait(&mut client, |c| c.view.detail.is_some()).await;
    drop(task);
    wait(&mut client, |c| {
        c.view.connection == Connection::Unavailable || c.view.stale
    })
    .await;
    assert!(!client.view.can_act());
    client.dispatch(Intent::CopySelected);
    assert!(!client.view.action_pending);
}

#[tokio::test]
async fn enrollment_open_and_cancel_reconcile_through_control() {
    let Fixture {
        config,
        clipboard: _clipboard,
        _engine,
        enrollment,
        task: _task,
        _identity,
    } = fixture().await;
    let mut client = Controller::connect(config).unwrap();
    wait(&mut client, |c| c.view.can_act()).await;
    enrollment.set_listen_port_override(0).await;
    let opened = enrollment.open().await.unwrap();
    assert!(opened.listening);
    // Drive inspect through the controller after open so the happy path is visible.
    client.dispatch(Intent::Navigate(Route::Devices));
    client.dispatch(Intent::OpenEnrollment);
    wait(&mut client, |c| {
        c.view
            .enrollment
            .as_ref()
            .is_some_and(|snap| snap.listening)
            && !c.view.action_pending
    })
    .await;
    let snap = client.view.enrollment.as_ref().unwrap();
    assert!(
        snap.listen_addr
            .as_ref()
            .unwrap()
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .port()
            > 0
    );
    client.dispatch(Intent::CancelEnrollment(None));
    wait(&mut client, |c| {
        !c.view.action_pending && c.view.enrollment.as_ref().is_some_and(|s| !s.listening)
    })
    .await;
    assert!(client.view.error.is_none());
}

#[tokio::test]
async fn newer_resume_clears_a_stale_pause_success_notice() {
    let Fixture {
        config,
        clipboard: _clipboard,
        _engine,
        enrollment: _enrollment,
        task: _task,
        _identity,
    } = fixture().await;
    let mut desktop = Controller::connect(config.clone()).unwrap();
    let mut terminal = Controller::connect(config).unwrap();
    wait(&mut desktop, |client| client.view.can_act()).await;
    wait(&mut terminal, |client| client.view.can_act()).await;
    desktop.dispatch(Intent::SetPaused(true));
    wait(&mut desktop, |client| {
        client.view.connection == Connection::Paused
            && !client.view.action_pending
            && client
                .view
                .notice
                .as_deref()
                .is_some_and(|text| text.contains("Paused until"))
    })
    .await;
    wait(&mut terminal, |client| {
        client.view.connection == Connection::Paused
    })
    .await;
    terminal.dispatch(Intent::SetPaused(false));
    wait(&mut terminal, |client| {
        client.view.connection != Connection::Paused && !client.view.action_pending
    })
    .await;
    wait(&mut desktop, |client| {
        client.view.connection != Connection::Paused
            && client
                .view
                .notice
                .as_deref()
                .is_none_or(|text| !text.contains("Paused"))
    })
    .await;
    assert_ne!(desktop.view.connection, Connection::Paused);
}

#[tokio::test]
async fn a_new_pairing_attempt_selects_its_own_result_after_failure() {
    let fixture = fixture().await;
    let mut client = Controller::connect(fixture.config.clone()).unwrap();
    wait(&mut client, |c| c.view.can_act()).await;
    client.dispatch(Intent::Navigate(Route::Devices));
    client.dispatch(Intent::OpenEnrollment);
    wait(&mut client, |c| {
        !c.view.action_pending && c.view.enrollment.as_ref().is_some_and(|s| s.listening)
    })
    .await;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap().to_string();
    drop(reserved);
    let mut previous = None;
    for attempt in 1..=2 {
        client.dispatch(Intent::DialEnrollment {
            address: address.clone(),
            expected_fingerprint: None,
            expected_node_id: None,
        });
        wait(&mut client, |c| {
            !c.view.action_pending
                && c.view.enrollment.as_ref().is_some_and(|s| {
                    s.sessions.len() == attempt && s.sessions.iter().all(|s| s.state == "Failed")
                })
        })
        .await;
        let selected = client
            .view
            .selected_session
            .expect("the attempt must be selected");
        assert_ne!(
            Some(selected),
            previous,
            "retry still shows the previous failed attempt"
        );
        previous = Some(selected);
    }
    assert!(fixture
        .enrollment
        .trusted_devices()
        .await
        .unwrap()
        .is_empty());
}

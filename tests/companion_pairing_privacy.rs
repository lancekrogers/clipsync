//! A pairing code is shown only to the local UI. It never reaches tracing,
//! the diagnostics export, error text, or the joining computer's snapshot.
mod support;
use clipsync::{
    adapters::{ClipboardProviderWrapper, HistoryManager},
    config::Config,
    control::{self, Command, EnrollmentTarget},
    enrollment::EnrollmentManager,
    sync::SyncEngine,
    transport::{TransportConfig, TransportManager},
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use support::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn config_for(id: &Identity, name: &str) -> Config {
    let mut config = Config::default();
    config.node_id = id.id();
    config.listen_addr = "127.0.0.1:0".into();
    config.advertise_name = name.into();
    config.auth.ssh_key = id.dir.path().join("identity");
    config.auth.authorized_keys = id.dir.path().join("authorized_keys");
    config.clipboard.history_db = id.dir.path().join("history.db");
    config.clipboard.history_key = Some(id.dir.path().join("history.key"));
    config
}

/// Whole-token matches only: a six-digit code can occur by chance inside a
/// hex nonce or UUID, but not as its own token in this output.
fn leaks(text: &str, code: &str) -> bool {
    let grouped = format!("{} {}", &code[..3], &code[3..]);
    text.contains(&grouped)
        || text
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| token == code)
}

async fn wait_finished(mgr: &EnrollmentManager, id: uuid::Uuid) -> String {
    for _ in 0..800 {
        let snap = mgr.inspect(None).await.unwrap();
        if let Some(s) = snap.sessions.iter().find(|s| s.session_id == id) {
            if matches!(s.state.as_str(), "Succeeded" | "Failed") {
                return s.state.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("attempt did not finish");
}

#[tokio::test(flavor = "current_thread")]
async fn codes_stay_out_of_logs_diagnostics_and_the_joining_snapshot() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let allowing_id = Identity::new().await;
    let allowing = Arc::new(
        EnrollmentManager::new(
            Arc::new(config_for(&allowing_id, "allowing")),
            allowing_id.auth.clone(),
        )
        .unwrap(),
    );
    allowing.set_listen_port_override(0).await;

    // The joining side is a full daemon target so its control server and
    // diagnostics export run for real.
    let joining_id = Identity::new().await;
    let config = config_for(&joining_id, "joining");
    let transport = Arc::new(TransportManager::with_auth(
        TransportConfig::default(),
        joining_id.auth.clone(),
        joining_id.id(),
    ));
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
        Arc::new(ClipboardProviderWrapper::new(
            Box::new(Clipboard::default()),
        )),
        history,
        transport,
    ));
    let joining = Arc::new(
        EnrollmentManager::new(Arc::new(config.clone()), joining_id.auth.clone()).unwrap(),
    );
    let target = Arc::new(EnrollmentTarget::new(engine, joining.clone()));
    let path = control::socket_path(&config);
    let max_frame = control::max_frame_bytes(&config);
    let server = control::Server::bind(path.clone(), &config).unwrap();
    let _server = Task(tokio::spawn(
        async move { server.run(target.as_ref()).await },
    ));

    let open = allowing.open().await.unwrap();
    let open_debug = format!("{open:?}");
    let addr = open.listen_addr.unwrap();
    let mut codes = vec![open.code.unwrap().code];

    // A wrong attempt first, then the right code.
    let wrong = format!(
        "{:06}",
        (codes[0].digits().parse::<u32>().unwrap() + 1) % 1_000_000
    );
    let sent = control::request(
        &path,
        Command::EnrollmentPair {
            address: addr.clone(),
            code: control::PairingCode::parse(&wrong).unwrap(),
            expected_fingerprint: None,
            expected_node_id: None,
        },
        max_frame,
    )
    .await
    .unwrap();
    let id = sent.enrollment.unwrap().requested_session_id.unwrap();
    assert_eq!(wait_finished(&joining, id).await, "Failed");
    codes.push(allowing.inspect(None).await.unwrap().code.unwrap().code);

    let sent = control::request(
        &path,
        Command::EnrollmentPair {
            address: addr.clone(),
            code: codes[1].clone(),
            expected_fingerprint: None,
            expected_node_id: None,
        },
        max_frame,
    )
    .await
    .unwrap();
    let id = sent.enrollment.unwrap().requested_session_id.unwrap();
    assert_eq!(wait_finished(&joining, id).await, "Succeeded");

    // A failing control command lands in the diagnostics error ring.
    let _ = control::request_result(
        &path,
        Command::EnrollmentPair {
            address: "nowhere".into(),
            code: codes[1].clone(),
            expected_fingerprint: None,
            expected_node_id: None,
        },
        max_frame,
    )
    .await;
    let diagnostics = control::request(&path, Command::DiagnosticsExport, max_frame)
        .await
        .unwrap()
        .diagnostics
        .unwrap();
    let diagnostics = serde_json::to_string(&diagnostics).unwrap();
    let joining_view = control::request(
        &path,
        Command::EnrollmentInspect { session_id: None },
        max_frame,
    )
    .await
    .unwrap();
    let joining_view = serde_json::to_string(&joining_view).unwrap();
    let allowing_debug = format!("{:?}", allowing.inspect(None).await.unwrap());
    let errors: Vec<String> = joining
        .inspect(None)
        .await
        .unwrap()
        .sessions
        .into_iter()
        .chain(allowing.inspect(None).await.unwrap().sessions)
        .filter_map(|s| s.error)
        .collect();
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("Enrollment"), "tracing was not captured");

    for code in &codes {
        let digits = code.digits();
        assert!(!leaks(&logs, digits), "code in tracing output");
        assert!(!leaks(&diagnostics, digits), "code in diagnostics");
        assert!(!leaks(&joining_view, digits), "code in joining snapshot");
        assert!(!leaks(&allowing_debug, digits), "code in Debug output");
        assert!(!leaks(&open_debug, digits), "code in Debug output");
        assert!(errors.iter().all(|e| !leaks(e, digits)), "code in errors");
    }
    assert!(leaks(
        &serde_json::to_string(&codes[0]).unwrap(),
        codes[0].digits()
    ));
}

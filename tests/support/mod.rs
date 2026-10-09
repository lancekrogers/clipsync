#![allow(dead_code)]
use clipsync::{
    auth::{AuthConfig, KeyPair, KeyType, SshAuthenticator},
    clipboard::{ClipboardContent, ClipboardError, ClipboardProvider, ClipboardWatcher},
    transport::{tls, PeerInfo},
};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tempfile::TempDir;
use tokio::sync::Mutex;
pub struct Identity {
    pub dir: TempDir,
    pub auth: Arc<SshAuthenticator>,
    pub key: KeyPair,
}
impl Identity {
    pub async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let key = KeyPair::generate(KeyType::Ed25519).unwrap();
        key.save_to_file(&dir.path().join("identity"))
            .await
            .unwrap();
        let auth = Arc::new(
            SshAuthenticator::new(AuthConfig {
                private_key_path: dir.path().join("identity"),
                authorized_keys_path: dir.path().join("authorized_keys"),
                generate_if_missing: false,
            })
            .await
            .unwrap(),
        );
        Self { dir, auth, key }
    }
    pub async fn trust(&self, other: &Self) {
        self.auth
            .add_trusted_peer(&other.key.public_key().to_openssh(), None)
            .await
            .unwrap();
    }
    pub fn id(&self) -> uuid::Uuid {
        tls::node_id(&self.key.public_key())
    }
    pub fn ordered_pair<'a>(a: &'a Self, b: &'a Self) -> (&'a Self, &'a Self) {
        if a.id() < b.id() {
            (a, b)
        } else {
            (b, a)
        }
    }
    pub fn peer(&self, addr: SocketAddr) -> PeerInfo {
        PeerInfo {
            id: self.id(),
            name: "test".into(),
            addresses: vec![addr],
            port: addr.port(),
            version: "2.0.0".into(),
            platform: "test".into(),
            metadata: Default::default(),
            last_seen: 0,
        }
    }
}
#[derive(Clone, Default)]
pub struct Clipboard {
    pub text: Arc<Mutex<String>>,
    pub writes: Arc<AtomicUsize>,
    pub fail: Arc<AtomicBool>,
}
impl Clipboard {
    pub async fn copy(&self, text: &str) {
        *self.text.lock().await = text.into();
    }
}
#[async_trait::async_trait]
impl ClipboardProvider for Clipboard {
    async fn get_content(&self) -> Result<ClipboardContent, ClipboardError> {
        Ok(ClipboardContent::text(self.text.lock().await.clone()))
    }
    async fn set_content(&self, data: &ClipboardContent) -> Result<(), ClipboardError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ClipboardError::Platform("fixture write failure".into()));
        }
        *self.text.lock().await = String::from_utf8(data.data.clone()).unwrap();
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn clear(&self) -> Result<(), ClipboardError> {
        self.copy("").await;
        Ok(())
    }
    fn name(&self) -> &str {
        "isolated memory clipboard"
    }
    async fn watch(&self) -> Result<ClipboardWatcher, ClipboardError> {
        Err(ClipboardError::WatchError("not used by poller".into()))
    }
}
pub struct Task<T>(pub tokio::task::JoinHandle<T>);
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub async fn eventually(mut predicate: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !predicate().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition did not become true");
}

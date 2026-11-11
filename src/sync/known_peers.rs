//! Last working addresses of authenticated peers, so a restart can reconnect without discovery.
//!
//! Entries are dial hints only. Every dial still goes through TLS pinning against the
//! authorized keys, and entries for peers that are no longer authorized are dropped.

use crate::discovery::PeerInfo;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;
use uuid::Uuid;

const FILE_VERSION: u32 = 1;
/// The transport splits one connect timeout across candidates, so keep the list short.
pub const MAX_ADDRESSES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownPeer {
    pub name: String,
    /// Most recently verified first.
    pub addresses: Vec<SocketAddr>,
}

#[derive(Serialize, Deserialize)]
struct KnownPeersFile {
    version: u32,
    peers: BTreeMap<Uuid, KnownPeer>,
}

pub struct KnownPeers {
    path: PathBuf,
    peers: Mutex<BTreeMap<Uuid, KnownPeer>>,
}

impl KnownPeers {
    /// Load the store at `path`. A missing or unreadable file starts empty.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let peers = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<KnownPeersFile>(&bytes) {
                Ok(file) if file.version == FILE_VERSION => file.peers,
                Ok(file) => {
                    tracing::warn!(
                        "Ignoring known peers at {} with unsupported version {}",
                        path.display(),
                        file.version
                    );
                    BTreeMap::new()
                }
                Err(e) => {
                    tracing::warn!("Ignoring unreadable known peers at {}: {e}", path.display());
                    BTreeMap::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                tracing::warn!("Could not read known peers at {}: {e}", path.display());
                BTreeMap::new()
            }
        };
        if !peers.is_empty() {
            tracing::info!(
                "Loaded {} known peer(s) for reconnect from {}",
                peers.len(),
                path.display()
            );
        }
        Self {
            path,
            peers: Mutex::new(peers),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn get(&self, id: Uuid) -> Option<KnownPeer> {
        self.peers.lock().await.get(&id).cloned()
    }

    pub async fn contains(&self, id: Uuid) -> bool {
        self.peers.lock().await.contains_key(&id)
    }

    pub async fn is_empty(&self) -> bool {
        self.peers.lock().await.is_empty()
    }

    /// Remember an address that just completed an authenticated outbound handshake.
    pub async fn record(&self, id: Uuid, name: &str, addr: SocketAddr) -> Result<()> {
        let mut peers = self.peers.lock().await;
        let entry = peers.entry(id).or_insert_with(|| KnownPeer {
            name: name.to_string(),
            addresses: Vec::new(),
        });
        if entry.addresses.first() == Some(&addr) && entry.name == name {
            return Ok(());
        }
        entry.name = name.to_string();
        entry.addresses.retain(|a| *a != addr);
        entry.addresses.insert(0, addr);
        entry.addresses.truncate(MAX_ADDRESSES);
        self.persist(&peers)
    }

    /// Drop peers outside `authorized`, then add stored addresses to `peers`
    /// (after any discovered ones) so each peer id is dialed once.
    pub async fn merge_into(&self, peers: &mut Vec<PeerInfo>, authorized: &HashSet<Uuid>) {
        let mut known = self.peers.lock().await;
        let revoked: Vec<Uuid> = known
            .keys()
            .filter(|id| !authorized.contains(id))
            .copied()
            .collect();
        if !revoked.is_empty() {
            for id in &revoked {
                known.remove(id);
                tracing::info!("Forgetting known peer {id}: no longer authorized");
            }
            if let Err(e) = self.persist(&known) {
                tracing::warn!("Could not update known peers: {e:#}");
            }
        }
        for (id, entry) in known.iter() {
            if let Some(peer) = peers.iter_mut().find(|p| p.id == *id) {
                for addr in &entry.addresses {
                    if !peer.addresses.contains(addr) {
                        peer.addresses.push(*addr);
                    }
                }
            } else if let Some(first) = entry.addresses.first() {
                peers.push(PeerInfo {
                    id: *id,
                    name: entry.name.clone(),
                    addresses: entry.addresses.clone(),
                    port: first.port(),
                    version: "unknown".into(),
                    platform: "unknown".into(),
                    metadata: Default::default(),
                    last_seen: 0,
                });
            }
        }
    }

    fn persist(&self, peers: &BTreeMap<Uuid, KnownPeer>) -> Result<()> {
        let json = serde_json::to_vec_pretty(&KnownPeersFile {
            version: FILE_VERSION,
            peers: peers.clone(),
        })?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(format!(".tmp-{}", std::process::id()));
        let tmp = PathBuf::from(tmp);
        let written = replace_private(&tmp, &self.path, &json);
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written.with_context(|| format!("write {}", self.path.display()))
    }
}

fn replace_private(tmp: &Path, dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn addr(last: u8) -> SocketAddr {
        SocketAddr::from(([192, 168, 1, last], 8484))
    }

    #[tokio::test]
    async fn records_survive_reopen_newest_first_and_capped() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("known_peers.json");
        let id = Uuid::new_v4();
        let store = KnownPeers::open(&path);
        for last in [1, 2, 3, 4, 2] {
            store.record(id, "mac", addr(last)).await.unwrap();
        }

        let reopened = KnownPeers::open(&path);
        let peer = reopened.get(id).await.unwrap();
        assert_eq!(peer.name, "mac");
        assert_eq!(peer.addresses, vec![addr(2), addr(4), addr(3)]);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn corrupt_file_starts_empty_and_is_replaced_on_next_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("known_peers.json");
        std::fs::write(&path, b"{not json").unwrap();
        let store = KnownPeers::open(&path);
        assert!(store.is_empty().await);

        let id = Uuid::new_v4();
        store.record(id, "arch", addr(9)).await.unwrap();
        assert!(KnownPeers::open(&path).contains(id).await);
    }

    #[tokio::test]
    async fn merge_appends_to_discovered_peer_and_drops_unauthorized() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("known_peers.json");
        let discovered_id = Uuid::new_v4();
        let offline_id = Uuid::new_v4();
        let revoked_id = Uuid::new_v4();
        let store = KnownPeers::open(&path);
        store.record(discovered_id, "mac", addr(1)).await.unwrap();
        store.record(offline_id, "arch", addr(2)).await.unwrap();
        store.record(revoked_id, "old", addr(3)).await.unwrap();

        let mut peers = vec![PeerInfo {
            id: discovered_id,
            name: "mac".into(),
            addresses: vec![addr(7)],
            port: 8484,
            version: "1".into(),
            platform: "macos".into(),
            metadata: Default::default(),
            last_seen: 0,
        }];
        let authorized = HashSet::from([discovered_id, offline_id]);
        store.merge_into(&mut peers, &authorized).await;

        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].addresses, vec![addr(7), addr(1)]);
        assert_eq!(peers[1].id, offline_id);
        assert_eq!(peers[1].addresses, vec![addr(2)]);
        assert!(!KnownPeers::open(&path).contains(revoked_id).await);
    }
}

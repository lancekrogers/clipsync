//! Enrollment-only mDNS advertisement and browse (`_clipsync-enroll._tcp`).
//!
//! TXT fields are address/candidate hints only — never authorization.

use crate::control::EnrollmentCandidateView;
use anyhow::{anyhow, Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo as MdnsServiceInfo};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

pub const ENROLL_SERVICE_TYPE: &str = "_clipsync-enroll._tcp.local.";
/// Bounds what announcements on a busy or hostile network can add to every snapshot.
const MAX_NEARBY_DEVICES: usize = 32;
const MAX_ADDRESSES_PER_DEVICE: usize = 4;

pub struct EnrollMdns {
    daemon: Arc<Mutex<Option<ServiceDaemon>>>,
    service_fullname: Arc<Mutex<Option<String>>>,
    browse_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    candidates: Arc<RwLock<Vec<EnrollmentCandidateView>>>,
    our_id: Uuid,
}

impl EnrollMdns {
    pub fn new(our_id: Uuid) -> Self {
        Self {
            daemon: Arc::new(Mutex::new(None)),
            service_fullname: Arc::new(Mutex::new(None)),
            browse_handle: Arc::new(Mutex::new(None)),
            candidates: Arc::new(RwLock::new(Vec::new())),
            our_id,
        }
    }

    async fn ensure_daemon(&self) -> Result<()> {
        let mut daemon = self.daemon.lock().await;
        if daemon.is_none() {
            *daemon = Some(ServiceDaemon::new().context("Enrollment mDNS daemon")?);
        }
        Ok(())
    }

    /// Collect nearby allowing computers. Callers gate this on a LAN-facing
    /// configuration; loopback fixtures must never touch host mDNS.
    pub async fn browse(&self) -> Result<()> {
        let mut handle = self.browse_handle.lock().await;
        if handle.as_ref().is_some_and(|h| !h.is_finished()) {
            return Ok(());
        }
        self.ensure_daemon().await?;
        let receiver = {
            let daemon = self.daemon.lock().await;
            daemon
                .as_ref()
                .ok_or_else(|| anyhow!("Enrollment mDNS daemon"))?
                .browse(ENROLL_SERVICE_TYPE)?
        };
        let candidates = self.candidates.clone();
        let our_id = self.our_id;
        *handle = Some(tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                if let Err(e) = handle_event(event, &candidates, our_id).await {
                    tracing::debug!("Enrollment mDNS event: {e}");
                }
            }
        }));
        Ok(())
    }

    pub async fn is_browsing(&self) -> bool {
        self.browse_handle
            .lock()
            .await
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Announce this computer while it allows a device. The TXT record carries
    /// address hints only, never the pairing code.
    pub async fn advertise(
        &self,
        display_name: &str,
        listen: SocketAddr,
        fingerprint: &str,
    ) -> Result<()> {
        // Loopback / fixture binds must never open a host mDNS daemon (LAN leak).
        if listen.ip().is_loopback() {
            tracing::debug!("Enrollment mDNS skipped for loopback listen {}", listen);
            return Ok(());
        }
        let addresses = local_addresses_for(listen)?;
        let hostname = format!("{}.local.", sanitize_hostname(display_name));
        let instance = format!("ClipSync-Enroll-{}", self.our_id);
        let info = MdnsServiceInfo::new(
            ENROLL_SERVICE_TYPE,
            &instance,
            &hostname,
            addresses.as_slice(),
            listen.port(),
            Some(txt_properties(self.our_id, fingerprint, display_name)),
        )?;
        let fullname = info.get_fullname().to_string();
        self.ensure_daemon().await?;
        if let Some(daemon) = self.daemon.lock().await.as_ref() {
            daemon.register(info)?;
        }
        *self.service_fullname.lock().await = Some(fullname);
        Ok(())
    }

    /// Withdraw the announcement but keep browsing.
    pub async fn unadvertise(&self) {
        if let Some(fullname) = self.service_fullname.lock().await.take() {
            if let Some(daemon) = self.daemon.lock().await.as_ref() {
                let _ = daemon.unregister(&fullname);
            }
        }
    }

    pub async fn snapshot_candidates(&self) -> Vec<EnrollmentCandidateView> {
        self.candidates.read().await.clone()
    }

    pub async fn stop(&self) {
        if let Some(handle) = self.browse_handle.lock().await.take() {
            handle.abort();
        }
        if let Some(fullname) = self.service_fullname.lock().await.take() {
            if let Some(daemon) = self.daemon.lock().await.as_ref() {
                let _ = daemon.unregister(&fullname);
            }
        }
        if let Some(daemon) = self.daemon.lock().await.take() {
            let _ = daemon.shutdown();
        }
        self.candidates.write().await.clear();
    }
}

impl Drop for EnrollMdns {
    fn drop(&mut self) {
        // Best-effort sync cleanup so tests/daemons do not leak mDNS daemons.
        if let Ok(mut handle) = self.browse_handle.try_lock() {
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
        if let Ok(mut name) = self.service_fullname.try_lock() {
            if let Some(fullname) = name.take() {
                if let Ok(daemon) = self.daemon.try_lock() {
                    if let Some(d) = daemon.as_ref() {
                        let _ = d.unregister(&fullname);
                    }
                }
            }
        }
        if let Ok(mut daemon) = self.daemon.try_lock() {
            if let Some(d) = daemon.take() {
                let _ = d.shutdown();
            }
        }
    }
}

/// `enroll=1` marks a pairing listener for every ClipSync version; older
/// joiners that dial it are told to update.
fn txt_properties(id: Uuid, fingerprint: &str, display_name: &str) -> HashMap<String, String> {
    HashMap::from([
        ("enroll".to_string(), "1".to_string()),
        ("id".to_string(), id.to_string()),
        ("ssh_fp".to_string(), fingerprint.to_string()),
        ("name".to_string(), display_name.to_string()),
    ])
}

async fn handle_event(
    event: ServiceEvent,
    candidates: &Arc<RwLock<Vec<EnrollmentCandidateView>>>,
    our_id: Uuid,
) -> Result<()> {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let props: HashMap<String, String> = info
                .get_properties()
                .iter()
                .map(|p| {
                    let v = p
                        .val()
                        .map(|b| String::from_utf8_lossy(b).to_string())
                        .unwrap_or_default();
                    (p.key().to_string(), v)
                })
                .collect();
            if props.get("enroll").map(String::as_str) != Some("1") {
                return Ok(());
            }
            let node_id = props
                .get("id")
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| anyhow!("missing id"))?;
            if node_id == our_id {
                return Ok(());
            }
            let fp = props.get("ssh_fp").cloned();
            let name = props
                .get("name")
                .cloned()
                .unwrap_or_else(|| info.get_hostname().to_string());
            let port = info.get_port();
            let mut views = Vec::new();
            for addr in info.get_addresses().iter().take(MAX_ADDRESSES_PER_DEVICE) {
                views.push(EnrollmentCandidateView {
                    display_name: name.clone(),
                    address: SocketAddr::new(*addr, port).to_string(),
                    advertised_fingerprint: fp.clone(),
                    advertised_node_id: Some(node_id),
                    source: "mdns".into(),
                });
            }
            if views.is_empty() {
                return Ok(());
            }
            let mut guard = candidates.write().await;
            guard.retain(|c| c.advertised_node_id != Some(node_id));
            let mut devices: Vec<_> = guard.iter().map(|c| c.advertised_node_id).collect();
            devices.dedup();
            if devices.len() >= MAX_NEARBY_DEVICES {
                return Ok(());
            }
            guard.extend(views);
            Ok(())
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            if let Some(id) = extract_enroll_id(&fullname) {
                candidates
                    .write()
                    .await
                    .retain(|c| c.advertised_node_id != Some(id));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn extract_enroll_id(fullname: &str) -> Option<Uuid> {
    let name = fullname.split('.').next()?;
    let id = name.strip_prefix("ClipSync-Enroll-")?;
    Uuid::parse_str(id).ok()
}

fn sanitize_hostname(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    if cleaned.is_empty() {
        "clipsync".into()
    } else {
        cleaned
    }
}

fn local_addresses_for(listen: SocketAddr) -> Result<Vec<IpAddr>> {
    if listen.ip().is_loopback() {
        bail_loopback()?;
    }
    if !listen.ip().is_unspecified() {
        return Ok(vec![listen.ip()]);
    }
    let mut out = Vec::new();
    for iface in if_addrs::get_if_addrs()? {
        if iface.is_loopback() {
            continue;
        }
        match (listen.ip(), iface.ip()) {
            (IpAddr::V4(_), IpAddr::V4(_)) => out.push(iface.ip()),
            (IpAddr::V6(_), IpAddr::V6(_)) => out.push(iface.ip()),
            _ => {}
        }
    }
    if out.is_empty() {
        anyhow::bail!("Enrollment mDNS: no non-loopback addresses for advertise");
    }
    Ok(out)
}

fn bail_loopback() -> Result<()> {
    anyhow::bail!("Enrollment mDNS refuses loopback advertisement")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_record_carries_only_address_hints() {
        let id = Uuid::new_v4();
        let props = txt_properties(id, "SHA256:fp", "Desk");
        let mut keys: Vec<_> = props.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["enroll", "id", "name", "ssh_fp"]);
        assert_eq!(props["id"], id.to_string());
    }

    #[test]
    fn extract_enroll_id_parses() {
        let id = Uuid::new_v4();
        let name = format!("ClipSync-Enroll-{id}._clipsync-enroll._tcp.local.");
        assert_eq!(extract_enroll_id(&name), Some(id));
    }
}

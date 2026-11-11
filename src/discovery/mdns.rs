//! mDNS/DNS-SD service discovery implementation

use crate::discovery::{
    addresses::advertise_for_listener, types::DiscoveryMethod, Discovery, DiscoveryEvent, PeerInfo,
    PeerManager, ServiceInfo,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use mdns_sd::{
    DaemonEvent, ServiceDaemon, ServiceEvent, ServiceInfo as MdnsServiceInfo, TxtProperties,
};
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

const SERVICE_TYPE: &str = "_clipsync._tcp.local.";
/// Coalesces a burst of rebuild requests into one rebuild.
const REBUILD_DEBOUNCE: Duration = Duration::from_secs(3);
/// Upper bound on how often the advertisement is rebuilt.
const REBUILD_MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Why the mDNS daemon and advertisement are being rebuilt.
///
/// mdns-sd skips an interface whose test multicast send fails when the daemon starts
/// (as it does while macOS is still deciding Local Network access). It binds the
/// interface again once sends work, but only re-announces services with automatic
/// addresses. Ours lists explicit addresses, so it stays unanswered on that interface
/// until it is registered again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    /// Network sends failed earlier in this process and now succeed.
    NetworkRecovered,
    /// mdns-sd bound an interface address we advertise after registration.
    InterfaceAdded(IpAddr),
}

impl std::fmt::Display for RebuildReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkRecovered => write!(f, "network sends recovered after failing"),
            Self::InterfaceAdded(ip) => {
                write!(f, "interface address {ip} bound after registration")
            }
        }
    }
}

/// Requests an in-process rebuild of the mDNS daemon. Requests are coalesced.
#[derive(Clone, Default)]
pub struct RebuildHandle {
    inner: Arc<RebuildRequests>,
}

#[derive(Default)]
struct RebuildRequests {
    pending: std::sync::Mutex<Option<RebuildReason>>,
    notify: Notify,
}

impl RebuildHandle {
    pub fn request(&self, reason: RebuildReason) {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(reason);
        self.inner.notify.notify_one();
    }

    fn take(&self) -> Option<RebuildReason> {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// Run `rebuild` once per burst of requests, never more often than `min_interval`.
async fn run_rebuilds<F, Fut>(
    requests: RebuildHandle,
    debounce: Duration,
    min_interval: Duration,
    mut rebuild: F,
) where
    F: FnMut(RebuildReason) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut last: Option<tokio::time::Instant> = None;
    loop {
        requests.inner.notify.notified().await;
        tokio::time::sleep(debounce).await;
        if let Some(last) = last {
            tokio::time::sleep_until(last + min_interval).await;
        }
        let Some(reason) = requests.take() else {
            continue;
        };
        rebuild(reason).await;
        last = Some(tokio::time::Instant::now());
    }
}

/// mDNS-based service discovery
pub struct MdnsDiscovery {
    shared: Arc<Shared>,
    rebuild: RebuildHandle,
    rebuild_task: Option<JoinHandle<()>>,
}

struct Shared {
    peer_manager: PeerManager,
    our_id: Arc<RwLock<Option<Uuid>>>,
    state: Mutex<DaemonState>,
}

#[derive(Default)]
struct DaemonState {
    daemon: Option<ServiceDaemon>,
    browse: Option<JoinHandle<()>>,
    monitor: Option<JoinHandle<()>>,
    /// Kept so a rebuild can register again with fresh interface addresses.
    service: Option<ServiceInfo>,
    registered: Option<String>,
}

impl Shared {
    fn launch(&self, state: &mut DaemonState) -> Result<()> {
        let daemon = ServiceDaemon::new()?;
        let receiver = daemon.browse(SERVICE_TYPE);
        // Keep the daemon even if browsing failed so teardown still shuts it down.
        state.daemon = Some(daemon);
        let receiver = receiver?;
        let peer_manager = self.peer_manager.clone();
        let our_id = self.our_id.clone();
        state.browse = Some(tokio::spawn(async move {
            MdnsDiscovery::browse_loop(receiver, peer_manager, our_id).await;
        }));
        Ok(())
    }

    fn register(
        state: &mut DaemonState,
        service_info: &ServiceInfo,
        rebuild: &RebuildHandle,
    ) -> Result<Vec<IpAddr>> {
        let daemon = state
            .daemon
            .as_ref()
            .ok_or_else(|| anyhow!("mDNS daemon not started"))?;
        // Watch before computing addresses: with no usable address yet, the
        // interface coming up is what must trigger the next attempt.
        let watcher = tokio::spawn(watch_interfaces(
            daemon.monitor()?,
            listen_bind(service_info),
            rebuild.clone(),
        ));
        if let Some(previous) = state.monitor.replace(watcher) {
            previous.abort();
        }
        let mdns_info = MdnsDiscovery::create_mdns_service_info(service_info)?;
        let fullname = mdns_info.get_fullname().to_string();
        let mut addresses: Vec<IpAddr> = mdns_info.get_addresses().iter().copied().collect();
        addresses.sort();
        daemon.register(mdns_info)?;
        state.registered = Some(fullname);
        Ok(addresses)
    }

    fn teardown(state: &mut DaemonState) {
        for task in [state.browse.take(), state.monitor.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
        if let Some(daemon) = state.daemon.take() {
            if let Some(name) = state.registered.take() {
                if let Err(e) = daemon.unregister(&name) {
                    tracing::debug!("mDNS unregister {name}: {e}");
                }
            }
            if let Err(e) = daemon.shutdown() {
                tracing::debug!("mDNS shutdown: {e}");
            }
        }
    }

    async fn rebuild(&self, reason: RebuildReason, requests: &RebuildHandle) {
        let mut state = self.state.lock().await;
        let Some(service) = state.service.clone() else {
            return;
        };
        tracing::warn!("Rebuilding mDNS advertisement: {reason}");
        Self::teardown(&mut state);
        let result = self
            .launch(&mut state)
            .and_then(|()| Self::register(&mut state, &service, requests));
        match result {
            Ok(addresses) => tracing::info!(
                "mDNS advertisement re-registered for {} on {addresses:?}",
                service.id
            ),
            Err(e) if state.monitor.is_some() => {
                tracing::warn!("mDNS advertisement rebuild failed; waiting for an interface: {e:#}")
            }
            Err(e) => {
                tracing::warn!("mDNS advertisement rebuild failed; retrying: {e:#}");
                requests.request(reason);
            }
        }
    }
}

fn listen_bind(service_info: &ServiceInfo) -> SocketAddr {
    service_info
        .listen_bind
        .unwrap_or_else(|| SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), service_info.port))
}

/// Request a rebuild when mdns-sd binds an address we would advertise.
async fn watch_interfaces(
    events: mdns_sd::Receiver<DaemonEvent>,
    bind: SocketAddr,
    requests: RebuildHandle,
) {
    while let Ok(event) = events.recv_async().await {
        let ip = match event {
            DaemonEvent::IpAdd(ip) => ip,
            DaemonEvent::Error(e) => {
                tracing::debug!("mDNS daemon error: {e}");
                continue;
            }
            _ => continue,
        };
        if advertise_for_listener(bind, &[ip]).contains(&ip) {
            tracing::info!("mDNS bound interface address {ip} after registration");
            requests.request(RebuildReason::InterfaceAdded(ip));
        }
    }
}

impl MdnsDiscovery {
    /// Create a new mDNS discovery instance
    pub fn new(peer_manager: PeerManager) -> Result<Self> {
        Ok(Self {
            shared: Arc::new(Shared {
                peer_manager,
                our_id: Arc::new(RwLock::new(None)),
                state: Mutex::new(DaemonState::default()),
            }),
            rebuild: RebuildHandle::default(),
            rebuild_task: None,
        })
    }

    /// Handle for other components to request an advertisement rebuild.
    pub fn rebuild_handle(&self) -> RebuildHandle {
        self.rebuild.clone()
    }

    /// Browse loop to handle discovered services
    async fn browse_loop(
        receiver: mdns_sd::Receiver<ServiceEvent>,
        peer_manager: PeerManager,
        our_id: Arc<RwLock<Option<Uuid>>>,
    ) {
        while let Ok(event) = receiver.recv_async().await {
            if let Err(e) = Self::handle_service_event(event, &peer_manager, &our_id).await {
                tracing::error!("Error handling mDNS event: {e}");
            }
        }
    }

    /// Handle a service discovery event
    async fn handle_service_event(
        event: ServiceEvent,
        peer_manager: &PeerManager,
        our_id: &Arc<RwLock<Option<Uuid>>>,
    ) -> Result<()> {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                // Parse service info
                let peer_info = Self::parse_service_info(&info)?;

                // Skip our own service
                let our_id = our_id.read().await;
                if let Some(id) = our_id.as_ref() {
                    if peer_info.id == *id {
                        return Ok(());
                    }
                }

                // Add peer
                peer_manager
                    .add_peer(peer_info, DiscoveryMethod::Mdns)
                    .await?;
            }
            ServiceEvent::ServiceRemoved(_, full_name) => {
                // Extract peer ID from service name if possible
                if let Some(peer_id) = Self::extract_peer_id(&full_name) {
                    peer_manager.remove_peer(peer_id).await?;
                }
            }
            _ => {
                // Other events we don't need to handle
            }
        }

        Ok(())
    }

    /// Parse mDNS service info into PeerInfo
    fn parse_service_info(info: &MdnsServiceInfo) -> Result<PeerInfo> {
        // Extract TXT record data
        let txt_data = Self::parse_txt_records(info.get_properties());

        // Get addresses
        let addresses: Vec<SocketAddr> = info
            .get_addresses()
            .iter()
            .map(|addr| SocketAddr::new(*addr, info.get_port()))
            .collect();

        if addresses.is_empty() {
            return Err(anyhow!("No addresses found for service"));
        }

        // Extract service name (remove .local. suffix)
        let name = info
            .get_hostname()
            .trim_end_matches(".local.")
            .trim_end_matches('.')
            .to_string();

        Ok(PeerInfo::from_mdns(
            name,
            addresses,
            info.get_port(),
            &txt_data,
        ))
    }

    /// Parse TXT records into key-value pairs
    fn parse_txt_records(properties: &TxtProperties) -> Vec<(String, String)> {
        properties
            .iter()
            .map(|prop| {
                let key = prop.key().to_string();
                let value = if let Some(val) = prop.val() {
                    String::from_utf8_lossy(val).to_string()
                } else {
                    String::new()
                };
                (key, value)
            })
            .collect()
    }

    /// Extract peer ID from service name
    fn extract_peer_id(full_name: &str) -> Option<Uuid> {
        // Service name format: "ClipSync-{uuid}.{service_type}"
        let parts: Vec<&str> = full_name.split('.').collect();
        if let Some(name_part) = parts.first() {
            if let Some(uuid_str) = name_part.strip_prefix("ClipSync-") {
                return Uuid::parse_str(uuid_str).ok();
            }
        }
        None
    }

    /// Create mDNS service info from our ServiceInfo
    fn create_mdns_service_info(service_info: &ServiceInfo) -> Result<MdnsServiceInfo> {
        let service_name = format!("ClipSync-{}", service_info.id);
        let hostname = format!("{}.local.", service_info.name);

        // Convert txt_data to properties
        let mut properties = HashMap::new();
        properties.insert("id".to_string(), service_info.id.to_string());
        for (key, value) in &service_info.txt_data {
            properties.insert(key.clone(), value.clone());
        }

        let bind = listen_bind(service_info);
        let interface_ips = Self::get_local_addresses()?;
        let addresses = advertise_for_listener(bind, &interface_ips);
        if addresses.is_empty() {
            return Err(anyhow!("No listener-compatible IP addresses found"));
        }

        Ok(MdnsServiceInfo::new(
            SERVICE_TYPE,
            &service_name,
            &hostname,
            addresses.as_slice(),
            service_info.port,
            Some(properties),
        )?)
    }

    /// Get local IP addresses (excluding loopback)
    fn get_local_addresses() -> Result<Vec<IpAddr>> {
        let mut addresses = Vec::new();

        for iface in if_addrs::get_if_addrs()? {
            if !iface.is_loopback() {
                addresses.push(iface.ip());
            }
        }

        Ok(addresses)
    }
}

#[async_trait]
impl Discovery for MdnsDiscovery {
    async fn start(&mut self) -> Result<()> {
        let mut state = self.shared.state.lock().await;
        if state.daemon.is_none() {
            self.shared.launch(&mut state)?;
        }
        drop(state);
        if self.rebuild_task.is_none() {
            let shared = Arc::downgrade(&self.shared);
            let requests = self.rebuild.clone();
            self.rebuild_task = Some(tokio::spawn(run_rebuilds(
                self.rebuild.clone(),
                REBUILD_DEBOUNCE,
                REBUILD_MIN_INTERVAL,
                move |reason| {
                    let shared = shared.upgrade();
                    let requests = requests.clone();
                    async move {
                        if let Some(shared) = shared {
                            shared.rebuild(reason, &requests).await;
                        }
                    }
                },
            )));
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(task) = self.rebuild_task.take() {
            task.abort();
        }
        let mut state = self.shared.state.lock().await;
        Shared::teardown(&mut state);
        state.service = None;
        Ok(())
    }

    async fn discover_peers(&mut self) -> Result<Vec<PeerInfo>> {
        // Return peers discovered via mDNS
        self.shared
            .peer_manager
            .get_peers_by_method(DiscoveryMethod::Mdns)
            .await
    }

    async fn announce(&mut self, service_info: ServiceInfo) -> Result<()> {
        *self.shared.our_id.write().await = Some(service_info.id);
        let mut state = self.shared.state.lock().await;
        let addresses = Shared::register(&mut state, &service_info, &self.rebuild)?;
        tracing::info!(
            "mDNS advertisement registered for {} on {addresses:?}",
            service_info.id
        );
        state.service = Some(service_info);
        Ok(())
    }

    fn subscribe_changes(&mut self) -> mpsc::Receiver<DiscoveryEvent> {
        self.shared.peer_manager.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_peer_id() {
        let full_name = "ClipSync-550e8400-e29b-41d4-a716-446655440000._clipsync._tcp.local.";
        let peer_id = MdnsDiscovery::extract_peer_id(full_name);
        assert!(peer_id.is_some());
        assert_eq!(
            peer_id.unwrap().to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn test_extract_peer_id_valid() {
        let full_name = "ClipSync-550e8400-e29b-41d4-a716-446655440000._clipsync._tcp.local.";
        let peer_id = MdnsDiscovery::extract_peer_id(full_name);
        assert!(peer_id.is_some());
        assert_eq!(
            peer_id.unwrap().to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rebuild_requests_coalesce_and_are_rate_limited() {
        let requests = RebuildHandle::default();
        let runs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = runs.clone();
        let _worker = tokio::spawn(run_rebuilds(
            requests.clone(),
            REBUILD_DEBOUNCE,
            REBUILD_MIN_INTERVAL,
            move |reason| {
                recorded
                    .lock()
                    .unwrap()
                    .push((tokio::time::Instant::now(), reason));
                async {}
            },
        ));
        let ip: IpAddr = "192.168.1.5".parse().unwrap();
        let count = || runs.lock().unwrap().len();

        tokio::time::sleep(Duration::from_secs(300)).await;
        assert_eq!(count(), 0, "no request, no rebuild");

        requests.request(RebuildReason::NetworkRecovered);
        requests.request(RebuildReason::InterfaceAdded(ip));
        tokio::time::sleep(REBUILD_DEBOUNCE * 2).await;
        assert_eq!(count(), 1, "a burst is one rebuild");
        assert_eq!(runs.lock().unwrap()[0].1, RebuildReason::NetworkRecovered);

        requests.request(RebuildReason::InterfaceAdded(ip));
        tokio::time::sleep(REBUILD_MIN_INTERVAL / 2).await;
        assert_eq!(count(), 1, "cooldown defers the next rebuild");
        tokio::time::sleep(REBUILD_MIN_INTERVAL).await;
        assert_eq!(count(), 2, "a deferred request still runs");
        let runs_now = runs.lock().unwrap().clone();
        assert!(runs_now[1].0 - runs_now[0].0 >= REBUILD_MIN_INTERVAL);

        tokio::time::sleep(REBUILD_MIN_INTERVAL * 10).await;
        assert_eq!(count(), 2, "no further rebuilds without a cause");
    }

    #[tokio::test]
    async fn failed_rebuild_keeps_watching_for_an_address() {
        let mut discovery = MdnsDiscovery::new(PeerManager::new()).unwrap();
        discovery.start().await.unwrap();
        // A link-local listener has nothing advertisable, so registration fails
        // without announcing anything on the network.
        let mut service = ServiceInfo::default();
        service.listen_bind = Some("[fe80::1]:9".parse().unwrap());
        discovery.shared.state.lock().await.service = Some(service);

        discovery
            .shared
            .rebuild(RebuildReason::NetworkRecovered, &discovery.rebuild)
            .await;

        let state = discovery.shared.state.lock().await;
        assert!(state.registered.is_none());
        let watcher = state.monitor.as_ref().expect("interface watcher survives");
        assert!(!watcher.is_finished());
        drop(state);
        discovery.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_mdns_lifecycle() {
        let peer_manager = PeerManager::new();
        let mut discovery = MdnsDiscovery::new(peer_manager).unwrap();

        // Start should succeed
        assert!(discovery.start().await.is_ok());

        // Stop should succeed
        assert!(discovery.stop().await.is_ok());
    }
}

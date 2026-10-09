//! Listener-compatible address selection and peer dial ordering.

use crate::discovery::types::DiscoveryMethod;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PEER_ADDRESSES: usize = 16;
const MAX_ADVERTISED_ADDRESSES: usize = 8;
pub const ADDRESS_SNAPSHOT_TTL_SECS: i64 = 300;

/// Addresses suitable for mDNS when the daemon listens on `bind`.
pub fn advertise_for_listener(bind: SocketAddr, interface_ips: &[IpAddr]) -> Vec<IpAddr> {
    let mut out = Vec::new();
    match bind {
        SocketAddr::V4(v4) if v4.ip().is_unspecified() => {
            for ip in interface_ips {
                if let IpAddr::V4(v) = ip {
                    if is_advertisable_ipv4(*v) {
                        push_unique(&mut out, *ip);
                    }
                }
            }
        }
        SocketAddr::V6(v6) if v6.ip().is_unspecified() => {
            for ip in interface_ips {
                if let IpAddr::V6(v) = ip {
                    if is_advertisable_ipv6_global(*v) {
                        push_unique(&mut out, *ip);
                    }
                }
            }
        }
        SocketAddr::V4(v4) if v4.ip().is_loopback() => {
            push_unique(&mut out, IpAddr::V4(*v4.ip()));
        }
        SocketAddr::V6(v6) if v6.ip().is_loopback() => {
            push_unique(&mut out, IpAddr::V6(*v6.ip()));
        }
        SocketAddr::V4(v4) => push_unique(&mut out, IpAddr::V4(*v4.ip())),
        SocketAddr::V6(v6) => {
            if is_advertisable_ipv6_global(*v6.ip()) {
                push_unique(&mut out, IpAddr::V6(*v6.ip()));
            }
        }
    }

    sort_advertise_candidates(&mut out);
    out.truncate(MAX_ADVERTISED_ADDRESSES);
    out
}

fn is_advertisable_ipv4(v4: Ipv4Addr) -> bool {
    !v4.is_unspecified() && !v4.is_multicast() && !v4.is_broadcast() && !v4.is_loopback()
}

fn is_advertisable_ipv6_global(v6: Ipv6Addr) -> bool {
    !v6.is_unspecified() && !v6.is_multicast() && !v6.is_loopback() && !v6.is_unicast_link_local()
}

fn is_ipv6_unique_local(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

fn push_unique(out: &mut Vec<IpAddr>, ip: IpAddr) {
    if !out.contains(&ip) {
        out.push(ip);
    }
}

/// Per-discovery-method address snapshot.
#[derive(Debug, Clone)]
pub struct MethodAddressSnapshot {
    pub addresses: Vec<SocketAddr>,
    pub updated_at: i64,
}

/// Normalize a dial target; reject unusable peer endpoints.
pub fn normalize_dial_address(addr: SocketAddr, service_port: u16) -> Option<SocketAddr> {
    let ip = addr.ip();
    if ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    if is_loopback(ip) {
        return Some(SocketAddr::new(ip, dial_port(addr.port(), service_port)));
    }
    match addr {
        SocketAddr::V4(v4) => (!v4.ip().is_broadcast())
            .then(|| SocketAddr::new(ip, dial_port(addr.port(), service_port))),
        SocketAddr::V6(v6) => {
            if v6.ip().is_unicast_link_local() || v6.scope_id() != 0 {
                return None;
            }
            Some(SocketAddr::V6(SocketAddrV6::new(
                *v6.ip(),
                dial_port(v6.port(), service_port),
                v6.flowinfo(),
                0,
            )))
        }
    }
}

fn dial_port(addr_port: u16, service_port: u16) -> u16 {
    if addr_port == 0 {
        service_port
    } else {
        addr_port
    }
}

/// Replace one method's snapshot and rebuild the merged peer address list.
pub fn apply_method_address_update(
    snapshots: &mut HashMap<DiscoveryMethod, MethodAddressSnapshot>,
    method: DiscoveryMethod,
    update: &[SocketAddr],
    service_port: u16,
    now: i64,
) -> Vec<SocketAddr> {
    let normalized = normalize_snapshot_addresses(update, service_port);
    snapshots.insert(
        method,
        MethodAddressSnapshot {
            addresses: normalized,
            updated_at: now,
        },
    );
    rebuild_peer_addresses(snapshots, service_port, now)
}

pub fn normalize_snapshot_addresses(addrs: &[SocketAddr], service_port: u16) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for addr in addrs {
        if let Some(norm) = normalize_dial_address(*addr, service_port) {
            push_unique_socket(&mut out, norm);
        }
    }
    sort_dial_candidates(&mut out);
    out
}

pub fn rebuild_peer_addresses(
    snapshots: &HashMap<DiscoveryMethod, MethodAddressSnapshot>,
    service_port: u16,
    now: i64,
) -> Vec<SocketAddr> {
    let mut merged = Vec::new();
    for snapshot in snapshots.values() {
        if now - snapshot.updated_at > ADDRESS_SNAPSHOT_TTL_SECS {
            continue;
        }
        for addr in &snapshot.addresses {
            if let Some(norm) = normalize_dial_address(*addr, service_port) {
                push_unique_socket(&mut merged, norm);
            }
        }
    }
    sort_dial_candidates(&mut merged);
    merged.truncate(MAX_PEER_ADDRESSES);
    merged
}

fn push_unique_socket(out: &mut Vec<SocketAddr>, addr: SocketAddr) {
    if !out.contains(&addr) {
        out.push(addr);
    }
}

/// Order dial targets: prefer IPv4 LAN, deprioritize link-local and loopback.
pub fn sort_dial_candidates(addresses: &mut [SocketAddr]) {
    addresses.sort_by_key(address_rank);
}

fn sort_advertise_candidates(addresses: &mut [IpAddr]) {
    addresses.sort_by_key(|ip| address_rank(&SocketAddr::new(*ip, 0)));
}

fn address_rank(addr: &SocketAddr) -> u8 {
    match addr.ip() {
        IpAddr::V4(v4) if v4.is_loopback() => 6,
        IpAddr::V4(v4) if v4.is_private() || v4.is_link_local() => 0,
        IpAddr::V4(_) => 1,
        IpAddr::V6(v6) if v6.is_loopback() => 7,
        IpAddr::V6(v6) if v6.is_unicast_link_local() => 5,
        IpAddr::V6(v6) if is_ipv6_unique_local(v6) => 3,
        IpAddr::V6(_) => 2,
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_wildcard_bind_advertises_only_ipv4_candidates() {
        let bind = "0.0.0.0:19484".parse().unwrap();
        let ifaces = vec![
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
        ];
        let advertised = advertise_for_listener(bind, &ifaces);
        assert_eq!(
            advertised,
            vec![
                IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            ]
        );
    }

    #[test]
    fn ipv6_wildcard_bind_advertises_only_global_ipv6() {
        let bind = "[::]:19484".parse().unwrap();
        let ifaces = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        ];
        let advertised = advertise_for_listener(bind, &ifaces);
        assert_eq!(
            advertised,
            vec![
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
                IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            ]
        );
    }

    #[test]
    fn explicit_ipv4_loopback_bind_does_not_advertise_ipv6_loopback() {
        let bind = "127.0.0.1:8484".parse().unwrap();
        let ifaces = vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ];
        let advertised = advertise_for_listener(bind, &ifaces);
        assert_eq!(advertised, vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    }

    #[test]
    fn method_snapshot_replaces_stale_same_method_and_honors_port_change() {
        let now = unix_now();
        let mut snapshots = HashMap::new();
        let old = vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
            19485,
        )];
        let merged =
            apply_method_address_update(&mut snapshots, DiscoveryMethod::Mdns, &old, 19485, now);
        assert_eq!(merged, old);

        let updated = vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
            0,
        )];
        let merged = apply_method_address_update(
            &mut snapshots,
            DiscoveryMethod::Mdns,
            &updated,
            19484,
            now + 1,
        );
        assert_eq!(
            merged,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
                19484
            )]
        );

        snapshots.insert(
            DiscoveryMethod::Manual,
            MethodAddressSnapshot {
                addresses: vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                    19484,
                )],
                updated_at: now - ADDRESS_SNAPSHOT_TTL_SECS - 1,
            },
        );
        let rebuilt = rebuild_peer_addresses(&snapshots, 19484, now + 1);
        assert_eq!(
            rebuilt,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
                19484
            )]
        );
    }

    #[test]
    fn fresh_manual_snapshot_retained_alongside_mdns_refresh() {
        let now = unix_now();
        let mut snapshots = HashMap::new();
        apply_method_address_update(
            &mut snapshots,
            DiscoveryMethod::Manual,
            &[SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                19484,
            )],
            19484,
            now,
        );
        let merged = apply_method_address_update(
            &mut snapshots,
            DiscoveryMethod::Mdns,
            &[SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 0, 18)),
                19484,
            )],
            19484,
            now,
        );
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn rejects_scoped_ipv6_dial_candidate() {
        let scoped = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
            9090,
            0,
            1,
        ));
        assert!(normalize_dial_address(scoped, 9090).is_none());
    }

    #[test]
    fn dial_order_prefers_ipv4_over_ula() {
        let port = 9090;
        let mut addrs = vec![
            SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), port),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), port),
        ];
        sort_dial_candidates(&mut addrs);
        assert!(matches!(addrs[0].ip(), IpAddr::V4(_)));
    }
}

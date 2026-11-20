//! Network diagnostics (protocol.md §13): problems are detected and named, never silent.
//! Heuristics here are first versions; Phase 4 validates them on real networks.

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::DeviceId;
use crate::signed::Platform;

use super::{Inner, lock, read};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkIssue {
    /// No active non-loopback interface.
    NoNetwork,
    /// Windows: the active network's profile is Public (reported by the host).
    WindowsPublicProfile,
    /// iOS / macOS Local Network permission or Android 17+ `ACCESS_LOCAL_NETWORK` missing
    /// (reported by the host).
    LocalNetworkDenied,
    /// A member's last known address is outside every local subnet.
    DifferentSubnet { device: DeviceId },
    /// Members should be on this network but none answers mDNS or the scan.
    ApIsolationSuspected,
    /// Netmask wider than /22; the scan covers only the /22 around this device.
    SubnetTooLarge { prefix: u8 },
    /// No free port in 52525–52534.
    PortInUse,
}

impl NetworkIssue {
    /// Stable code; the UI words it in the user's language.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoNetwork => "no_network",
            Self::WindowsPublicProfile => "windows_public_profile",
            Self::LocalNetworkDenied => "local_network_denied",
            Self::DifferentSubnet { .. } => "different_subnet",
            Self::ApIsolationSuspected => "ap_isolation_suspected",
            Self::SubnetTooLarge { .. } => "subnet_too_large",
            Self::PortInUse => "port_in_use",
        }
    }
}

/// A local IPv4 interface that can reach the home network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanInterface {
    /// OS name ("Wi-Fi", "en0", ...).
    pub name: String,
    /// Stable adapter id: the adapter GUID on Windows (`{...}`), the name elsewhere.
    pub id: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

/// Virtual and tunnel adapters that never lead to the other devices at home: Hyper-V / WSL /
/// Docker / VM host networks, VPN tunnels, Apple's peer-to-peer and sharing bridges.
fn is_virtual(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const PREFIXES: [&str; 14] = [
        "vethernet",
        "docker",
        "br-",
        "veth",
        "virbr",
        "vmnet",
        "vmware",
        "virtualbox",
        "vboxnet",
        "utun",
        "awdl",
        "llw",
        "bridge",
        "tailscale",
    ];
    PREFIXES.iter().any(|p| n.starts_with(p)) || n.contains("hyper-v") || n.contains("wsl")
}

/// Rank for publishing: typical home ranges first, so the address peers try first is the one
/// most likely to work.
fn preference(ip: &Ipv4Addr) -> u8 {
    match ip.octets() {
        [192, 168, ..] => 0,
        [10, ..] => 1,
        [172, b, ..] if (16..32).contains(&b) => 2,
        _ => 3,
    }
}

/// Interfaces kitedrop listens on and publishes: up, IPv4, not loopback / link-local /
/// point-to-point, not a virtual or tunnel adapter, and not a single-host (/31, /32) VPN address.
/// Ordered by [`preference`].
pub fn lan_interfaces() -> Vec<LanInterface> {
    let Ok(ifs) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut out: Vec<LanInterface> = ifs
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_p2p && !is_virtual(&i.name))
        .filter_map(|i| match &i.addr {
            if_addrs::IfAddr::V4(a)
                if !a.ip.is_loopback()
                    && !a.ip.is_link_local()
                    && !a.ip.is_unspecified()
                    && a.prefixlen < 31 =>
            {
                Some(LanInterface {
                    #[cfg(windows)]
                    id: i.adapter_name.clone(),
                    #[cfg(not(windows))]
                    id: i.name.clone(),
                    name: i.name.clone(),
                    ip: a.ip,
                    prefix: a.prefixlen,
                })
            }
            _ => None,
        })
        .collect();
    out.sort_by_key(|i| (preference(&i.ip), i.ip));
    out.dedup_by_key(|i| i.ip);
    out
}

/// `(address, prefix length)` of [`lan_interfaces`].
pub(crate) fn local_ipv4() -> Vec<(Ipv4Addr, u8)> {
    lan_interfaces()
        .into_iter()
        .map(|i| (i.ip, i.prefix))
        .collect()
}

pub(crate) fn in_subnet(ip: Ipv4Addr, net_ip: Ipv4Addr, prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = u32::MAX << (32 - u32::from(prefix.min(32)));
    (u32::from(ip) & mask) == (u32::from(net_ip) & mask)
}

pub(crate) fn detect(inner: &Inner) -> Vec<NetworkIssue> {
    let mut issues = Vec::new();
    let locals = local_ipv4();
    if locals.is_empty() && inner.cfg.advertise.is_none() {
        issues.push(NetworkIssue::NoNetwork);
    }
    if let Some(prefix) = locals.iter().map(|(_, p)| *p).filter(|p| *p < 22).min() {
        issues.push(NetworkIssue::SubnetTooLarge { prefix });
    }
    if let Some(e) = lock(&inner.listen_error).clone() {
        issues.push(e);
    }
    let me = inner.me();
    let mut any_listening_here = false;
    let mut any_online = false;
    {
        let g = read(&inner.group);
        let presence = read(&inner.presence);
        for m in &g.current().members {
            let id = m.identity().device_id();
            if id == me || m.platform == Platform::Ios {
                continue;
            }
            any_online |= presence.get(&id).is_some_and(|p| p.online);
            let addrs: Vec<Ipv4Addr> = g
                .record(&id)
                .map(|r| {
                    r.addresses
                        .iter()
                        .filter_map(|a| a.parse::<SocketAddrV4>().ok())
                        .map(|a| *a.ip())
                        .collect()
                })
                .unwrap_or_default();
            if addrs.is_empty() || locals.is_empty() {
                continue;
            }
            let here = addrs
                .iter()
                .any(|a| locals.iter().any(|(ip, p)| in_subnet(*a, *ip, *p)));
            any_listening_here |= here;
            if !here {
                issues.push(NetworkIssue::DifferentSubnet { device: id });
            }
        }
    }
    let scan = lock(&inner.scan);
    if any_listening_here
        && !any_online
        && scan.last_scan_found == Some(0)
        && scan.quiet_rounds >= 2
    {
        issues.push(NetworkIssue::ApIsolationSuspected);
    }
    drop(scan);
    issues.extend(inner.host.platform_issues());
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_adapters_are_skipped() {
        for n in [
            "vEthernet (Default Switch)",
            "vEthernet (WSL (Hyper-V firewall))",
            "docker0",
            "utun3",
            "awdl0",
            "bridge100",
        ] {
            assert!(is_virtual(n), "{n}");
        }
        for n in ["Wi-Fi", "Ethernet", "en0", "wlan0", "Ethernet 2"] {
            assert!(!is_virtual(n), "{n}");
        }
        let mut v = [
            Ipv4Addr::new(172, 24, 160, 1),
            Ipv4Addr::new(10, 1, 1, 1),
            Ipv4Addr::new(192, 168, 1, 111),
        ];
        v.sort_by_key(preference);
        assert_eq!(v[0], Ipv4Addr::new(192, 168, 1, 111));
    }

    #[test]
    fn subnet_math() {
        let ip = Ipv4Addr::new(192, 168, 1, 20);
        assert!(in_subnet(Ipv4Addr::new(192, 168, 1, 200), ip, 24));
        assert!(!in_subnet(Ipv4Addr::new(192, 168, 2, 1), ip, 24));
        assert!(in_subnet(Ipv4Addr::new(192, 168, 2, 1), ip, 22));
        assert!(in_subnet(Ipv4Addr::new(10, 0, 0, 1), ip, 0));
    }
}

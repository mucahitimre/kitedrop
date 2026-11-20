//! Reachability (protocol.md §12): last known address → mDNS → unicast scan sized by the real
//! netmask. Nothing is trusted until a probe signature proves the member's keys. Also runs the
//! periodic online check and notices network changes.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;

use crate::auth::ProbeResponse;
use crate::signed::Platform;
use crate::wire::DEFAULT_PORT;
use crate::{DeviceId, PublicIdentity};

use super::diagnostics;
use super::{Event, Inner, client, groupsync, lock, now, read};

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
const SCAN_TIMEOUT: Duration = Duration::from_millis(500);
const SCAN_PARALLEL: usize = 64;
const SCAN_MIN_INTERVAL_SECS: u64 = 600;
const MIN_PREFIX: u8 = 22;
#[cfg(feature = "mdns")]
const MDNS_TYPE: &str = "_kitedrop._tcp.local.";

#[derive(Default)]
pub(crate) struct ScanState {
    /// Network key (sorted local subnets) → time of the last scan there.
    last: HashMap<String, u64>,
    /// Members found by the last full scan on the current network.
    pub last_scan_found: Option<u32>,
    /// Consecutive presence rounds in which no listening member answered. The isolation
    /// warning waits for two, so a slow first round after start or wake does not raise it.
    pub quiet_rounds: u32,
    network: String,
    #[cfg(feature = "mdns")]
    daemon: Option<mdns_sd::ServiceDaemon>,
}

fn network_key(locals: &[(Ipv4Addr, u8)]) -> String {
    locals
        .iter()
        .map(|(ip, p)| format!("{ip}/{p}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Candidate addresses for a member, most likely first, without duplicates.
pub(crate) fn candidates(inner: &Inner, id: &DeviceId) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(a) = read(&inner.presence)
        .get(id)
        .and_then(|p| p.address.clone())
    {
        out.push(a);
    }
    if let Ok(Some((a, _))) = inner.store.peer_address(id) {
        out.push(a);
    }
    if let Some(r) = read(&inner.group).record(id) {
        out.extend(r.addresses.iter().cloned());
    }
    if let Some(found) = lock(&inner.discovered).get(id) {
        out.extend(found.iter().rev().cloned());
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert(a.clone()));
    out
}

async fn probe_addrs(
    inner: &Inner,
    expected: &PublicIdentity,
    addrs: &[String],
) -> Option<(String, ProbeResponse)> {
    for addr in addrs {
        match client::probe(&inner.http, addr, PROBE_TIMEOUT).await {
            Ok(r) if r.identity() == *expected => return Some((addr.clone(), r)),
            Ok(_) => tracing::debug!("{addr} answered with another identity"),
            Err(e) => tracing::debug!("probe {addr}: {e}"),
        }
    }
    None
}

/// Probes a member (or former member) at its known addresses; updates its online state.
pub(crate) async fn probe_known(inner: &Inner, id: &DeviceId) -> Option<(String, ProbeResponse)> {
    let expected = {
        let g = read(&inner.group);
        g.member(id).map(|m| m.identity()).or_else(|| {
            g.chain()
                .iter()
                .rev()
                .find_map(|v| v.list.member(id).map(|m| m.identity()))
        })
    }?;
    let found = probe_addrs(inner, &expected, &candidates(inner, id)).await;
    if read(&inner.group).is_member(id) {
        inner.set_presence(id, found.is_some(), found.as_ref().map(|(a, _)| a.as_str()));
    }
    found
}

/// Probes one member and, when it answers, syncs group state if either side is behind.
pub(crate) async fn probe_one(
    inner: &Arc<Inner>,
    id: &DeviceId,
) -> Option<(String, ProbeResponse)> {
    let found = probe_known(inner, id).await?;
    sync_if_needed(inner, id, &found.0, &found.1).await;
    Some(found)
}

async fn sync_if_needed(inner: &Arc<Inner>, id: &DeviceId, addr: &str, r: &ProbeResponse) {
    let (gid, gv, seq, my_seq) = {
        let g = read(&inner.group);
        (
            g.group_id(),
            g.version(),
            g.record_seq(id),
            g.record_seq(&inner.me()),
        )
    };
    if r.group_id != gid {
        return;
    }
    let behind = r.group_version > gv || r.record_seq > seq;
    let ahead = r.group_version < gv;
    if !behind && !ahead && my_seq == 0 {
        return;
    }
    if behind || ahead {
        let i = inner.clone();
        let id = *id;
        let addr = addr.to_owned();
        tokio::spawn(async move {
            if let Err(e) = groupsync::push_to(&i, &id, &addr).await {
                tracing::info!("group sync failed: {e}");
            }
        });
    }
}

fn listening_members(inner: &Inner) -> Vec<DeviceId> {
    read(&inner.group)
        .current()
        .members
        .iter()
        .filter(|m| m.platform != Platform::Ios)
        .map(|m| m.identity().device_id())
        .filter(|id| *id != inner.me())
        .collect()
}

/// Probes every listening member; scans the subnet if some cannot be found.
pub(crate) async fn probe_all(inner: &Arc<Inner>) {
    let ids = listening_members(inner);
    let results: Vec<bool> = futures_util::stream::iter(ids)
        .map(|id| {
            let inner = inner.clone();
            async move { probe_one(&inner, &id).await.is_some() }
        })
        .buffer_unordered(8)
        .collect()
        .await;
    if results.iter().any(|ok| !ok) && inner.cfg.scan {
        scan(inner, false).await;
    }
    {
        let any_online = {
            let presence = read(&inner.presence);
            listening_members(inner)
                .iter()
                .any(|id| presence.get(id).is_some_and(|p| p.online))
        };
        let mut s = lock(&inner.scan);
        s.quiet_rounds = if any_online {
            0
        } else {
            s.quiet_rounds.saturating_add(1)
        };
    }
    let issues = diagnostics::detect(inner);
    inner.emit(Event::NetworkIssues(issues));
}

/// Unicast scan of the local subnets (capped at /22) on the default port and every port that
/// members published. At most once per 10 minutes per network unless `force`.
pub(crate) async fn scan(inner: &Arc<Inner>, force: bool) {
    let locals = diagnostics::local_ipv4();
    if locals.is_empty() {
        return;
    }
    let key = network_key(&locals);
    {
        let mut s = lock(&inner.scan);
        let t = now();
        if !force
            && s.last
                .get(&key)
                .is_some_and(|l| t.saturating_sub(*l) < SCAN_MIN_INTERVAL_SECS)
        {
            return;
        }
        s.last.insert(key, t);
    }
    let mut ports = vec![DEFAULT_PORT];
    for id in listening_members(inner) {
        if let Some(r) = read(&inner.group).record(&id) {
            for a in &r.addresses {
                if let Some(p) = a.rsplit_once(':').and_then(|(_, p)| p.parse().ok())
                    && !ports.contains(&p)
                {
                    ports.push(p);
                }
            }
        }
    }
    let mut targets = Vec::new();
    for (ip, prefix) in &locals {
        let prefix = (*prefix).max(MIN_PREFIX);
        let mask = u32::MAX << (32 - u32::from(prefix));
        let base = u32::from(*ip) & mask;
        let size = 1u32 << (32 - u32::from(prefix));
        for host in 1..size.saturating_sub(1) {
            let addr = Ipv4Addr::from(base + host);
            if addr == *ip {
                continue;
            }
            for p in &ports {
                targets.push(format!("{addr}:{p}"));
            }
        }
    }
    let members: HashMap<DeviceId, PublicIdentity> = {
        let g = read(&inner.group);
        g.current()
            .members
            .iter()
            .map(|m| (m.identity().device_id(), m.identity()))
            .collect()
    };
    let me = inner.me();
    let found: Vec<(DeviceId, String)> = futures_util::stream::iter(targets)
        .map(|addr| {
            let http = &inner.http;
            async move {
                let r = client::probe(http, &addr, SCAN_TIMEOUT).await.ok()?;
                Some((r.identity(), addr))
            }
        })
        .buffer_unordered(SCAN_PARALLEL)
        .filter_map(|x| async move { x })
        .filter_map(|(ident, addr)| {
            let id = ident.device_id();
            let ok = id != me && members.get(&id) == Some(&ident);
            async move { ok.then_some((id, addr)) }
        })
        .collect()
        .await;
    lock(&inner.scan).last_scan_found = Some(found.len() as u32);
    for (id, addr) in found {
        inner.add_address(id, addr.clone());
        inner.set_presence(&id, true, Some(&addr));
    }
}

/// Background loop: presence probes at the configured interval, network change detection.
pub(crate) async fn run(inner: Arc<Inner>) {
    start_mdns(&inner);
    let mut last_net = network_key(&diagnostics::local_ipv4());
    lock(&inner.scan).network = last_net.clone();
    loop {
        probe_all(&inner).await;
        let interval = inner.cfg.presence_interval_secs.max(5);
        // Check for network changes every 5 s while waiting for the next round.
        let mut waited = 0;
        while waited < interval {
            tokio::time::sleep(Duration::from_secs(5)).await;
            waited += 5;
            let net = network_key(&diagnostics::local_ipv4());
            if net != last_net {
                last_net = net.clone();
                {
                    let mut s = lock(&inner.scan);
                    s.network = net;
                    s.quiet_rounds = 0;
                }
                let _ = inner.refresh_own_record();
                restart_mdns(&inner);
                break;
            }
        }
    }
}

#[cfg(feature = "mdns")]
fn start_mdns(inner: &Arc<Inner>) {
    if !inner.cfg.mdns {
        return;
    }
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::info!("mDNS unavailable: {e}");
            return;
        }
    };
    let hex_id: String = inner.me().iter().map(|b| format!("{b:02x}")).collect();
    let port = inner.port.load(std::sync::atomic::Ordering::Relaxed);
    if inner.cfg.listen && port != 0 {
        let props = [("v", "1"), ("id", hex_id.as_str())];
        let host = format!("kitedrop-{}.local.", &hex_id[..12]);
        match mdns_sd::ServiceInfo::new(MDNS_TYPE, &hex_id, &host, "", port, &props[..]) {
            Ok(info) => {
                if let Err(e) = daemon.register(info.enable_addr_auto()) {
                    tracing::info!("mDNS register failed: {e}");
                }
            }
            Err(e) => tracing::info!("mDNS service info: {e}"),
        }
    }
    if let Ok(rx) = daemon.browse(MDNS_TYPE) {
        let weak = Arc::downgrade(inner);
        std::thread::Builder::new()
            .name("kitedrop-mdns".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    let Some(inner) = weak.upgrade() else { break };
                    if let mdns_sd::ServiceEvent::ServiceResolved(svc) = ev {
                        let Some(id) = svc.get_property_val_str("id").and_then(parse_hex_id) else {
                            continue;
                        };
                        if id == inner.me() || !read(&inner.group).is_member(&id) {
                            continue;
                        }
                        let port = svc.get_port();
                        for ip in svc.get_addresses_v4() {
                            inner.add_address(id, format!("{ip}:{port}"));
                        }
                        // A member we could not reach may be back: retry its sends.
                        if !read(&inner.presence).get(&id).is_some_and(|p| p.online) {
                            inner.kick_target(&id);
                        }
                    }
                }
            })
            .ok();
    }
    lock(&inner.scan).daemon = Some(daemon);
}

#[cfg(feature = "mdns")]
fn restart_mdns(inner: &Arc<Inner>) {
    stop_mdns(inner);
    start_mdns(inner);
}

#[cfg(feature = "mdns")]
pub(crate) fn stop_mdns(inner: &Inner) {
    if let Some(d) = lock(&inner.scan).daemon.take() {
        let _ = d.shutdown();
    }
}

#[cfg(not(feature = "mdns"))]
fn start_mdns(_: &Arc<Inner>) {}

#[cfg(not(feature = "mdns"))]
fn restart_mdns(_: &Arc<Inner>) {}

#[cfg(not(feature = "mdns"))]
pub(crate) fn stop_mdns(_: &Inner) {}

#[cfg(feature = "mdns")]
fn parse_hex_id(s: &str) -> Option<DeviceId> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

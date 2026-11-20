//! Group propagation (protocol.md §3.2–3.3): push new versions and records to members, pull
//! when a probe shows a peer is ahead, resolve forks, handle removal of this device.

use std::sync::Arc;

use crate::group::{Applied, GroupState};
use crate::signed::Platform;
use crate::{DeviceId, Error, Result};

use super::client::Session;
use super::server::GroupMessage;
use super::{Event, Inner, discovery, now, read, sender, write};

#[derive(serde::Deserialize)]
struct GroupVersionAnswer {
    group_version: u64,
}

fn message(inner: &Inner) -> GroupMessage {
    let g = read(&inner.group);
    GroupMessage {
        versions: g.chain_signed(),
        records: g.records_signed(),
    }
}

/// Applies versions and records received from a peer, then handles the consequences.
pub(crate) fn apply(inner: &Arc<Inner>, msg: &GroupMessage) -> Result<()> {
    let outcome = if msg.versions.is_empty() {
        Applied::Unchanged
    } else {
        write(&inner.group).apply_versions(&msg.versions)?
    };
    let mut records_changed = false;
    // iPhones whose push token is new or changed: items may already wait for them (a record can
    // arrive after the item was parked), and they would otherwise get no push for those items.
    let mut new_tokens = Vec::new();
    for r in &msg.records {
        let token = |inner: &Inner| {
            read(&inner.group)
                .record(&r.signer)
                .and_then(|x| x.push_token.clone())
        };
        let before = token(inner);
        // Records of non-members (or stale ones) are skipped, not errors.
        let changed = write(&inner.group).apply_record(r).unwrap_or(false);
        records_changed |= changed;
        if changed {
            let after = token(inner);
            if after.is_some() && after != before {
                new_tokens.push(r.signer);
            }
        }
    }
    if outcome == Applied::Unchanged && !records_changed {
        return Ok(());
    }
    if !read(&inner.group).is_member(&inner.me()) {
        removed_from_group(inner)?;
        return Ok(());
    }
    inner.save_group()?;
    if let Applied::Advanced { lost, .. } = outcome {
        reapply_lost(inner, &lost)?;
        sender::fail_non_members(inner);
        // Pass a newer state on to the others; spawned so the HTTP answer is not delayed.
        let i = inner.clone();
        tokio::spawn(async move { push_all(&i).await });
    }
    for id in new_tokens {
        if inner.store.outbox_for(&id).is_ok_and(|v| !v.is_empty()) {
            sender::notify_push(inner, &id);
        }
    }
    inner.emit(Event::DevicesChanged);
    Ok(())
}

/// Changes that lost a fork are applied again on top of the winner (protocol.md §3.2). Any
/// device that sees the lost version may do this, not only its author: every member may change
/// the list, and a change whose effect is already in the winner produces no new version.
fn reapply_lost(inner: &Inner, lost: &[crate::signed::GroupList]) -> Result<()> {
    let mut g = write(&inner.group);
    for (i, list) in lost.iter().enumerate() {
        // The first lost version's parent is in the chain; later ones chain onto the lost ones.
        let change = match i {
            0 => g.lost_change(list),
            _ => Some(crate::group::Change::between(&lost[i - 1], list)),
        };
        let Some(change) = change else {
            continue;
        };
        let mut members = g.current().members.clone();
        change.apply(&mut members);
        if members != g.current().members {
            g.propose(&inner.keys, now(), |m| *m = members)?;
        }
    }
    drop(g);
    inner.save_group()
}

/// Another member removed this device: forget the group and start a new single-member one.
fn removed_from_group(inner: &Inner) -> Result<()> {
    let fresh = GroupState::create(
        &inner.keys,
        &inner.cfg.device_name,
        inner.cfg.platform,
        now(),
        crate::random(),
    )?;
    *write(&inner.group) = fresh;
    inner.store.reset_group()?;
    inner.save_group()?;
    sender::fail_non_members(inner);
    let _ = inner.refresh_own_record();
    inner.emit(Event::RemovedFromGroup);
    inner.emit(Event::DevicesChanged);
    Ok(())
}

/// `POST /v1/group` to one member at `addr`; pulls back if the peer is at least as new (it may
/// hold the winning side of a fork).
pub(crate) async fn push_to(inner: &Arc<Inner>, id: &DeviceId, addr: &str) -> Result<()> {
    let mut s = Session::open(&inner.http, &inner.keys, addr, *id).await?;
    let ours = read(&inner.group).version();
    match s
        .post_json::<GroupVersionAnswer>("/v1/group", &message(inner))
        .await
    {
        Ok(a) if a.group_version < ours => Ok(()),
        Ok(_) => {
            // The peer may hold the winning side of a fork; it may also not know us yet (then
            // the pull is refused, which is fine: it just learned our state from the push).
            if let Err(e) = pull_with(inner, &mut s).await {
                tracing::debug!("group pull after push: {e}");
            }
            Ok(())
        }
        Err(e @ Error::Remote { .. }) => pull_with(inner, &mut s).await.map_err(|_| e),
        Err(e) => Err(e),
    }
}

/// `GET /v1/group` over an open session and apply it.
pub(crate) async fn pull_with(inner: &Arc<Inner>, s: &mut Session<'_>) -> Result<()> {
    let msg: GroupMessage = s.get_json("/v1/group").await?;
    match apply(inner, &msg) {
        Err(Error::GroupConflict) => Ok(()), // different group or unchainable: ignore
        r => r,
    }
}

/// Pushes the current state to every reachable listening member.
pub(crate) async fn push_all(inner: &Arc<Inner>) {
    for id in listening_members(inner) {
        if let Some((addr, _)) = discovery::probe_one(inner, &id).await
            && let Err(e) = push_to(inner, &id, &addr).await
        {
            tracing::info!("group push failed: {e}");
        }
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

pub(crate) async fn remove(inner: &Arc<Inner>, id: DeviceId) -> Result<()> {
    if !read(&inner.group).is_member(&id) {
        return Err(Error::NotFound);
    }
    if id == inner.me() {
        return Err(Error::Malformed("remove this device from another member"));
    }
    write(&inner.group).propose(&inner.keys, now(), |m| {
        m.retain(|x| x.identity().device_id() != id)
    })?;
    inner.save_group()?;
    sender::fail_non_members(inner);
    inner.emit(Event::DevicesChanged);
    // Tell the removed device too, so it resets itself (it can still verify our signature).
    if let Some((addr, _)) = discovery::probe_known(inner, &id).await {
        let _ = push_to(inner, &id, &addr).await;
    }
    push_all(inner).await;
    Ok(())
}

pub(crate) async fn rename(inner: &Arc<Inner>, id: DeviceId, name: String) -> Result<()> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.len() > 64 {
        return Err(Error::Malformed("device name"));
    }
    if read(&inner.group).member(&id).ok_or(Error::NotFound)?.name == name {
        return Ok(());
    }
    write(&inner.group).propose(&inner.keys, now(), |m| {
        if let Some(x) = m.iter_mut().find(|x| x.identity().device_id() == id) {
            x.name.clone_from(&name);
        }
    })?;
    inner.save_group()?;
    inner.emit(Event::DevicesChanged);
    push_all(inner).await;
    Ok(())
}

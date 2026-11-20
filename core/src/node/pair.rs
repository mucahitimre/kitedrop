//! Pairing flows over HTTP (protocol.md §4): one-time QR sessions on the device that shows the
//! QR, and the scanning side for both the normal and the reverse QR (D15).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::auth::ProbeResponse;
use crate::group::GroupState;
use crate::pairing::{
    self, AcceptDelivery, AcceptMessage, AcceptPayload, CODE_TTL_SECS, Invite, JoinMessage,
    JoinRequest,
};
use crate::signed::Member;
use crate::wire::PROTOCOL_VERSION;
use crate::{DeviceId, Error, PublicIdentity, Result};

use super::{Event, Inner, client, groupsync, lock, now, read, sender, write};

const MAX_FAILURES: u32 = 5;

enum Stage {
    Open,
    /// Reverse flow: a member's offer was accepted; waiting for its accept message.
    Offered {
        member: PublicIdentity,
        name: String,
        nonce: [u8; 16],
    },
    Used,
}

struct PairSession {
    invite: Invite,
    created: u64,
    failures: u32,
    stage: Stage,
}

#[derive(Default)]
pub(crate) struct Sessions(HashMap<[u8; 16], PairSession>);

impl Sessions {
    fn purge(&mut self) {
        let t = now();
        self.0
            .retain(|_, s| t.saturating_sub(s.created) <= CODE_TTL_SECS * 2);
    }

    /// The live session for `psk_id` (any failure answers `code_invalid`, protocol.md §4.2).
    /// `join`: the request is a join (normal QR); otherwise an offer or accept, which a reverse QR
    /// takes, and a normal QR too while the device showing it is alone (D34).
    fn live(&mut self, psk_id: &[u8; 16], join: bool, alone: bool) -> Result<&mut PairSession> {
        let s = self.0.get_mut(psk_id).ok_or(Error::CodeInvalid)?;
        let kind_ok = if join {
            !s.invite.reverse
        } else {
            s.invite.reverse || alone
        };
        if !kind_ok
            || now().saturating_sub(s.created) > CODE_TTL_SECS
            || s.failures >= MAX_FAILURES
            || matches!(s.stage, Stage::Used)
        {
            return Err(Error::CodeInvalid);
        }
        Ok(s)
    }
}

pub(crate) fn create_invite(inner: &Inner, reverse: bool) -> Result<String> {
    let addresses = inner.own_addresses();
    if addresses.is_empty() {
        return Err(Error::Internal(
            "this device is not listening on the network, so it cannot show a pairing code".into(),
        ));
    }
    let invite = Invite {
        code: crate::random(),
        inviter: inner.keys.public().fingerprint(),
        addresses,
        reverse,
    };
    let uri = invite.to_uri();
    let mut sessions = lock(&inner.pairing);
    sessions.purge();
    sessions.0.insert(
        pairing::psk_id(&invite.code),
        PairSession {
            invite,
            created: now(),
            failures: 0,
            stage: Stage::Open,
        },
    );
    Ok(uri)
}

fn member_from(req: &JoinRequest) -> Member {
    Member {
        sig_pk: req.sig_pk,
        kem_pk: req.kem_pk,
        name: req.name.chars().take(64).collect(),
        platform: req.platform,
        added_at: now(),
    }
}

/// Adds the new device (if not already a member) and builds the accept payload.
fn admit(inner: &Inner, req: &JoinRequest) -> Result<AcceptPayload> {
    let id = req.identity().device_id();
    {
        let mut g = write(&inner.group);
        if !g.is_member(&id) {
            let m = member_from(req);
            g.propose(&inner.keys, now(), |members| members.push(m))?;
        }
    }
    inner.save_group()?;
    let g = read(&inner.group);
    let chain = g.chain();
    Ok(AcceptPayload {
        v: PROTOCOL_VERSION,
        nonce: req.nonce,
        group: g.current_version().signed.clone(),
        records: g.records_signed(),
        parent: (chain.len() > 1).then(|| chain[chain.len() - 2].signed.clone()),
    })
}

fn after_admit(inner: &Arc<Inner>, req: &JoinRequest) {
    inner.emit(Event::Paired {
        device: req.identity().device_id(),
        name: req.name.clone(),
    });
    inner.emit(Event::DevicesChanged);
    let i = inner.clone();
    tokio::spawn(async move { groupsync::push_all(&i).await });
}

/// `POST /v1/pair/join` on the member that shows a normal QR.
pub(crate) fn handle_join(inner: &Arc<Inner>, msg: &JoinMessage) -> Result<AcceptMessage> {
    let (code, req) = {
        let mut sessions = lock(&inner.pairing);
        let s = sessions.live(&msg.psk_id, true, false)?;
        match pairing::open_join(&inner.keys, &s.invite.code, msg) {
            Ok(req) => {
                s.stage = Stage::Used;
                (s.invite.code, req)
            }
            Err(_) => {
                s.failures += 1;
                return Err(Error::CodeInvalid);
            }
        }
    };
    let payload = admit(inner, &req)?;
    let accept = pairing::seal_accept(
        &inner.keys,
        &code,
        &req.identity(),
        &payload,
        &mut crate::os_rng(),
    )?;
    after_admit(inner, &req);
    Ok(accept)
}

fn alone(inner: &Inner) -> bool {
    read(&inner.group).current().members.len() == 1
}

/// The QR content as the joiner of an offer flow sees it (the reverse flag only selects which
/// fingerprint checks apply; the code and fingerprint are unchanged).
fn as_reverse(invite: &Invite) -> Invite {
    Invite {
        reverse: true,
        ..invite.clone()
    }
}

/// `POST /v1/pair/offer` on the device that shows a reverse QR, or a normal QR while alone.
pub(crate) fn handle_offer(inner: &Inner, msg: &JoinMessage) -> Result<JoinMessage> {
    let alone = alone(inner);
    let mut sessions = lock(&inner.pairing);
    let s = sessions.live(&msg.psk_id, false, alone)?;
    if !matches!(s.stage, Stage::Open) {
        return Err(Error::CodeInvalid);
    }
    let offer = match pairing::open_offer(&inner.keys, &s.invite.code, msg) {
        Ok(o) => o,
        Err(_) => {
            s.failures += 1;
            return Err(Error::CodeInvalid);
        }
    };
    let nonce = crate::random::<16>();
    let join = pairing::seal_join(
        &inner.keys,
        &inner.cfg.device_name,
        inner.cfg.platform,
        &as_reverse(&s.invite),
        &offer.identity(),
        nonce,
        &mut crate::os_rng(),
    )?;
    s.stage = Stage::Offered {
        member: offer.identity(),
        name: offer.name.clone(),
        nonce,
    };
    Ok(join)
}

/// `POST /v1/pair/accept` on the device that took an offer.
pub(crate) fn handle_accept(inner: &Arc<Inner>, msg: &AcceptDelivery) -> Result<()> {
    let alone = alone(inner);
    let (payload, member, name) = {
        let mut sessions = lock(&inner.pairing);
        let s = sessions.live(&msg.psk_id, false, alone)?;
        let Stage::Offered {
            member,
            name,
            nonce,
        } = &s.stage
        else {
            return Err(Error::CodeInvalid);
        };
        let opened = pairing::open_accept(
            &inner.keys,
            &as_reverse(&s.invite),
            member,
            nonce,
            &msg.accept,
        );
        match opened {
            Ok(p) => {
                let r = (p, member.clone(), name.clone());
                s.stage = Stage::Used;
                r
            }
            Err(_) => {
                s.failures += 1;
                return Err(Error::CodeInvalid);
            }
        }
    };
    adopt_group(inner, &payload, &member.device_id())?;
    inner.emit(Event::Paired {
        device: member.device_id(),
        name,
    });
    Ok(())
}

/// The joiner takes over the inviter's group: the accepted version becomes its anchor.
fn adopt_group(inner: &Arc<Inner>, payload: &AcceptPayload, inviter: &DeviceId) -> Result<()> {
    if payload.group.signer != *inviter {
        return Err(Error::BadSignature);
    }
    // Anchor on the parent when given, so a fork at the new version can still be resolved (D20).
    let mut g = match &payload.parent {
        Some(parent) => {
            let mut g = GroupState::from_anchor(parent.clone())?;
            g.apply_versions(std::slice::from_ref(&payload.group))?;
            if g.current_version().signed != payload.group {
                return Err(Error::GroupConflict);
            }
            g
        }
        None => GroupState::from_anchor(payload.group.clone())?,
    };
    if !g.is_member(&inner.me()) || !g.is_member(inviter) {
        return Err(Error::NotMember);
    }
    for r in &payload.records {
        let _ = g.apply_record(r);
    }
    *write(&inner.group) = g;
    inner.store.reset_group()?;
    inner.save_group()?;
    sender::fail_non_members(inner);
    inner.refresh_own_record()?;
    inner.emit(Event::DevicesChanged);
    let i = inner.clone();
    tokio::spawn(async move { groupsync::push_all(&i).await });
    Ok(())
}

/// Probes the QR's addresses and pins the shower's keys through the QR fingerprint.
async fn pin(inner: &Inner, invite: &Invite) -> Result<(String, ProbeResponse)> {
    let mut last = Error::Network("no address in the pairing code".into());
    for addr in &invite.addresses {
        match client::probe(&inner.http, addr, Duration::from_secs(3)).await {
            Ok(r) if r.identity().fingerprint() == invite.inviter => {
                return Ok((addr.clone(), r));
            }
            Ok(_) => last = Error::BadSignature,
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Scanner side (D34): a device already in a group with others never leaves it silently.
/// - reverse QR, or a normal QR shown by a device that is alone → add the shower to my group;
/// - normal QR while I am alone (or the shower is already in my group) → join the shower's group;
/// - both in groups with other members → `already_in_group`.
pub(crate) async fn scan(inner: &Arc<Inner>, uri: &str) -> Result<DeviceId> {
    let invite = Invite::from_uri(uri)?;
    let (addr, probe) = pin(inner, &invite).await?;
    let peer = probe.identity();
    let peer_id = peer.device_id();
    let (my_alone, same_group) = {
        let g = read(&inner.group);
        (
            g.current().members.len() == 1,
            g.group_id() == probe.group_id,
        )
    };
    // A group at version 1 holds only its creator.
    let peer_alone = probe.group_version == 1;
    let add_peer = invite.reverse || (!my_alone && !same_group && peer_alone);
    if !add_peer && !my_alone && !same_group {
        return Err(Error::AlreadyInGroup);
    }
    if add_peer {
        let offer = pairing::seal_offer(
            &inner.keys,
            &inner.cfg.device_name,
            inner.cfg.platform,
            &as_reverse(&invite),
            &peer,
            crate::random(),
            &mut crate::os_rng(),
        )?;
        let join: JoinMessage =
            client::post_plain(&inner.http, &addr, "/v1/pair/offer", &offer).await?;
        let req = pairing::open_join(&inner.keys, &invite.code, &join)?;
        if req.identity() != peer {
            return Err(Error::BadSignature);
        }
        let payload = admit(inner, &req)?;
        let accept = AcceptDelivery {
            psk_id: pairing::psk_id(&invite.code),
            accept: pairing::seal_accept(
                &inner.keys,
                &invite.code,
                &peer,
                &payload,
                &mut crate::os_rng(),
            )?,
        };
        let _: serde_json::Value =
            client::post_plain(&inner.http, &addr, "/v1/pair/accept", &accept).await?;
        inner.set_presence(&peer_id, true, Some(&addr));
        after_admit(inner, &req);
    } else {
        let nonce = crate::random::<16>();
        let join = pairing::seal_join(
            &inner.keys,
            &inner.cfg.device_name,
            inner.cfg.platform,
            &invite,
            &peer,
            nonce,
            &mut crate::os_rng(),
        )?;
        let accept: AcceptMessage =
            client::post_plain(&inner.http, &addr, "/v1/pair/join", &join).await?;
        let payload = pairing::open_accept(&inner.keys, &invite, &peer, &nonce, &accept)?;
        adopt_group(inner, &payload, &peer_id)?;
        inner.set_presence(&peer_id, true, Some(&addr));
        let name = read(&inner.group)
            .member(&peer_id)
            .map(|m| m.name.clone())
            .unwrap_or_default();
        inner.emit(Event::Paired {
            device: peer_id,
            name,
        });
    }
    Ok(peer_id)
}

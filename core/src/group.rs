//! Group state: the signed, versioned membership chain plus self-signed device records
//! (protocol.md, "Group state"). Pure logic, no I/O; [`crate::store`] persists it.
//!
//! The chain starts at an *anchor*: version 1 for the device that created the group, or the
//! version a joiner received from its inviter during pairing (authenticated by the pairing PSK).
//! Every later version must be signed by a member of its parent and carry the parent's hash.

use std::collections::BTreeMap;

use crate::signed::{
    DOMAIN_DEVICE_RECORD, DOMAIN_GROUP, DeviceRecord, GroupList, Member, Platform, SignedObject,
};
use crate::wire::{PROTOCOL_VERSION, sha256};
use crate::{DeviceId, DeviceKeys, Error, Result};

/// One verified version of the group list.
#[derive(Clone, Debug)]
pub struct Version {
    pub signed: SignedObject,
    pub list: GroupList,
    /// SHA-256 of `signed.body`; the next version's `prev` and the fork tie-breaker.
    pub hash: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct StoredRecord {
    pub signed: SignedObject,
    pub record: DeviceRecord,
}

/// What applying candidate versions did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    Unchanged,
    /// Moved to a newer version. `lost` holds versions dropped because a competing version won
    /// a fork (smaller hash); the caller re-applies its own lost changes on top.
    Advanced {
        from: u64,
        to: u64,
        lost: Vec<GroupList>,
    },
}

/// Membership change between two versions; used to re-apply a change that lost a fork.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub added: Vec<Member>,
    pub removed: Vec<DeviceId>,
    pub renamed: Vec<(DeviceId, String)>,
}

impl Change {
    pub fn between(parent: &GroupList, child: &GroupList) -> Self {
        let ids = |l: &GroupList| -> BTreeMap<DeviceId, Member> {
            l.members
                .iter()
                .map(|m| (m.identity().device_id(), m.clone()))
                .collect()
        };
        let (p, c) = (ids(parent), ids(child));
        let mut change = Change::default();
        for (id, m) in &c {
            match p.get(id) {
                None => change.added.push(m.clone()),
                Some(old) if old.name != m.name => change.renamed.push((*id, m.name.clone())),
                Some(_) => {}
            }
        }
        change.removed = p
            .keys()
            .filter(|id| !c.contains_key(*id))
            .copied()
            .collect();
        change
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.renamed.is_empty()
    }

    pub fn apply(&self, members: &mut Vec<Member>) {
        members.retain(|m| !self.removed.contains(&m.identity().device_id()));
        for (id, name) in &self.renamed {
            if let Some(m) = members.iter_mut().find(|m| m.identity().device_id() == *id) {
                m.name.clone_from(name);
            }
        }
        for m in &self.added {
            let id = m.identity().device_id();
            if !members.iter().any(|x| x.identity().device_id() == id) {
                members.push(m.clone());
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct GroupState {
    chain: Vec<Version>,
    records: BTreeMap<DeviceId, StoredRecord>,
}

fn parse_unverified(signed: &SignedObject) -> Result<GroupList> {
    let list: GroupList = serde_json::from_slice(&signed.body)?;
    if list.v != PROTOCOL_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    Ok(list)
}

/// Verifies `signed` against a signer taken from `signers` (the parent list, or the list
/// itself for an anchor).
fn verify_by(signed: &SignedObject, signers: &GroupList) -> Result<GroupList> {
    let member = signers.member(&signed.signer).ok_or(Error::NotMember)?;
    let list: GroupList = signed.verify(DOMAIN_GROUP, &member.identity())?;
    if list.v != PROTOCOL_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    Ok(list)
}

/// Fork rule (protocol.md §3.2, D27): does `candidate` replace `local`, two different versions
/// with the same number and parent?
///
/// A version signed by a device that the other version removes never wins: otherwise a removed
/// (for example stolen) device could sign a competing version and grind its hash until it is
/// smaller. If each version removes the other's signer, the one held first stays. Otherwise the
/// body with the lexicographically smaller SHA-256 wins.
fn fork_winner(local: &Version, candidate: &Version) -> bool {
    let candidate_signer_removed = local.list.member(&candidate.signed.signer).is_none();
    let local_signer_removed = candidate.list.member(&local.signed.signer).is_none();
    match (candidate_signer_removed, local_signer_removed) {
        (true, _) => false,
        (false, true) => true,
        (false, false) => candidate.hash < local.hash,
    }
}

impl GroupState {
    /// A new single-member group (version 1) with a random group id.
    pub fn create(
        keys: &DeviceKeys,
        name: &str,
        platform: Platform,
        now: u64,
        group_id: [u8; 16],
    ) -> Result<Self> {
        let me = keys.public();
        let list = GroupList {
            v: PROTOCOL_VERSION,
            group_id,
            version: 1,
            prev: None,
            members: vec![Member {
                sig_pk: me.sig_pk,
                kem_pk: me.kem_pk,
                name: name.to_owned(),
                platform,
                added_at: now,
            }],
            updated_at: now,
        };
        let signed = SignedObject::sign(keys, DOMAIN_GROUP, &list)?;
        Self::from_anchor(signed)
    }

    /// Starts from a trusted anchor version: self-consistent (signed by one of its own members).
    /// Used for version 1 and for the version received while pairing.
    pub fn from_anchor(signed: SignedObject) -> Result<Self> {
        let unverified = parse_unverified(&signed)?;
        let list = verify_by(&signed, &unverified)?;
        let hash = sha256(&signed.body);
        Ok(Self {
            chain: vec![Version { signed, list, hash }],
            records: BTreeMap::new(),
        })
    }

    /// Rebuilds a stored state: an anchor followed by a gap-free chain, plus records.
    pub fn from_parts(versions: Vec<SignedObject>, records: Vec<SignedObject>) -> Result<Self> {
        let mut it = versions.into_iter();
        let mut state = Self::from_anchor(it.next().ok_or(Error::Malformed("empty group"))?)?;
        for signed in it {
            let next = state.verify_child(&signed, state.chain.len() - 1)?;
            state.chain.push(next);
        }
        for r in &records {
            state.apply_record(r)?;
        }
        Ok(state)
    }

    pub fn current(&self) -> &GroupList {
        &self.chain.last().expect("chain is never empty").list
    }

    pub fn current_version(&self) -> &Version {
        self.chain.last().expect("chain is never empty")
    }

    pub fn version(&self) -> u64 {
        self.current().version
    }

    pub fn group_id(&self) -> [u8; 16] {
        self.current().group_id
    }

    pub fn chain(&self) -> &[Version] {
        &self.chain
    }

    pub fn chain_signed(&self) -> Vec<SignedObject> {
        self.chain.iter().map(|v| v.signed.clone()).collect()
    }

    pub fn member(&self, id: &DeviceId) -> Option<&Member> {
        self.current().member(id)
    }

    pub fn is_member(&self, id: &DeviceId) -> bool {
        self.member(id).is_some()
    }

    pub fn record(&self, id: &DeviceId) -> Option<&DeviceRecord> {
        self.records.get(id).map(|r| &r.record)
    }

    pub fn records_signed(&self) -> Vec<SignedObject> {
        self.records.values().map(|r| r.signed.clone()).collect()
    }

    pub fn record_seq(&self, id: &DeviceId) -> u64 {
        self.record(id).map_or(0, |r| r.seq)
    }

    fn verify_child(&self, signed: &SignedObject, parent_index: usize) -> Result<Version> {
        let parent = &self.chain[parent_index];
        let list = verify_by(signed, &parent.list)?;
        if list.group_id != parent.list.group_id {
            return Err(Error::GroupConflict);
        }
        if list.version != parent.list.version + 1 || list.prev != Some(parent.hash) {
            return Err(Error::GroupConflict);
        }
        Ok(Version {
            hash: sha256(&signed.body),
            signed: signed.clone(),
            list,
        })
    }

    /// Signs and applies the next version, built by `change` from the current member list.
    pub fn propose(
        &mut self,
        keys: &DeviceKeys,
        now: u64,
        change: impl FnOnce(&mut Vec<Member>),
    ) -> Result<SignedObject> {
        let cur = self.current_version();
        let mut members = cur.list.members.clone();
        change(&mut members);
        let list = GroupList {
            v: PROTOCOL_VERSION,
            group_id: cur.list.group_id,
            version: cur.list.version + 1,
            prev: Some(cur.hash),
            members,
            updated_at: now,
        };
        let signed = SignedObject::sign(keys, DOMAIN_GROUP, &list)?;
        let next = self.verify_child(&signed, self.chain.len() - 1)?;
        self.chain.push(next);
        self.drop_stale_records();
        Ok(signed)
    }

    /// Applies versions received from a peer (any order, may include versions we have).
    ///
    /// Extends the chain one step at a time; on a fork (a different valid version with the same
    /// number) the one whose body has the lexicographically smaller SHA-256 wins. Fails with
    /// [`Error::GroupConflict`] if the versions belong to another group or cannot be chained
    /// (the caller then pulls the peer's full chain).
    pub fn apply_versions(&mut self, candidates: &[SignedObject]) -> Result<Applied> {
        let mut parsed = Vec::with_capacity(candidates.len());
        for c in candidates {
            let list = parse_unverified(c)?;
            if list.group_id != self.group_id() {
                return Err(Error::GroupConflict);
            }
            parsed.push((list.version, c));
        }
        parsed.sort_by_key(|(v, _)| *v);

        let from = self.version();
        let anchor = self.chain[0].list.version;
        let mut lost = Vec::new();
        let mut gap = false;
        for (version, signed) in parsed {
            if version <= anchor {
                continue; // history before our anchor cannot be verified and does not matter
            }
            let index = (version - anchor) as usize; // position in chain
            if index > self.chain.len() {
                gap = true;
                break;
            }
            let Ok(next) = self.verify_child(signed, index - 1) else {
                continue; // not a child of our version (other fork) or invalid: ignore
            };
            if index == self.chain.len() {
                self.chain.push(next);
            } else if self.chain[index].hash == next.hash {
                continue;
            } else if fork_winner(&self.chain[index], &next) {
                lost.extend(self.chain.drain(index..).map(|v| v.list));
                self.chain.push(next);
            }
        }
        let to = self.version();
        if to == from && lost.is_empty() {
            return if gap {
                Err(Error::GroupConflict)
            } else {
                Ok(Applied::Unchanged)
            };
        }
        self.drop_stale_records();
        Ok(Applied::Advanced { from, to, lost })
    }

    /// Accepts a device record iff its signer is a current member signing its own record with a
    /// higher `seq` than the stored one. Returns whether it was stored.
    pub fn apply_record(&mut self, signed: &SignedObject) -> Result<bool> {
        let member = self.member(&signed.signer).ok_or(Error::NotMember)?;
        let record: DeviceRecord = signed.verify(DOMAIN_DEVICE_RECORD, &member.identity())?;
        if record.v != PROTOCOL_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if record.device_id != signed.signer {
            return Err(Error::BadSignature);
        }
        if self
            .records
            .get(&record.device_id)
            .is_some_and(|r| r.record.seq >= record.seq)
        {
            return Ok(false);
        }
        self.records.insert(
            record.device_id,
            StoredRecord {
                signed: signed.clone(),
                record,
            },
        );
        Ok(true)
    }

    fn drop_stale_records(&mut self) {
        let current = self.current().clone();
        self.records.retain(|id, _| current.member(id).is_some());
    }

    /// Change made by `lost` relative to its parent, for re-applying it on top of the winner.
    /// `parent` is looked up in the current chain; `None` if it is not there.
    pub fn lost_change(&self, lost: &GroupList) -> Option<Change> {
        let anchor = self.chain[0].list.version;
        let parent_version = lost.version.checked_sub(1)?;
        let parent = self
            .chain
            .get(parent_version.checked_sub(anchor)? as usize)?;
        (lost.prev == Some(parent.hash)).then(|| Change::between(&parent.list, lost))
    }
}

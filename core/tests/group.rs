//! Group chain rules (protocol.md §3.2–3.3): chaining, signer membership, forks, records.

use kitedrop_core::group::{Applied, Change, GroupState};
use kitedrop_core::signed::{
    DOMAIN_DEVICE_RECORD, DOMAIN_GROUP, DeviceRecord, Member, Platform, SignedObject,
};
use kitedrop_core::{DeviceKeys, PublicIdentity};

fn keys() -> DeviceKeys {
    DeviceKeys::generate(&mut kitedrop_core::os_rng())
}

fn member(k: &PublicIdentity, name: &str) -> Member {
    Member {
        sig_pk: k.sig_pk,
        kem_pk: k.kem_pk,
        name: name.into(),
        platform: Platform::Android,
        added_at: 1,
    }
}

fn record(k: &DeviceKeys, seq: u64) -> SignedObject {
    let r = DeviceRecord {
        v: 1,
        device_id: k.device_id(),
        seq,
        addresses: vec![format!("10.0.0.{seq}:52525")],
        push_token: None,
        updated_at: seq,
    };
    SignedObject::sign(k, DOMAIN_DEVICE_RECORD, &r).unwrap()
}

/// A creates the group and adds B and C; returns A's state and copies for B and C.
fn three() -> (DeviceKeys, DeviceKeys, DeviceKeys, GroupState) {
    let (a, b, c) = (keys(), keys(), keys());
    let mut g = GroupState::create(&a, "A", Platform::Macos, 1, [7; 16]).unwrap();
    let (mb, mc) = (member(&b.public(), "B"), member(&c.public(), "C"));
    g.propose(&a, 2, |m| m.push(mb)).unwrap();
    g.propose(&a, 3, |m| m.push(mc)).unwrap();
    (a, b, c, g)
}

#[test]
fn chain_extends_and_persists() {
    let (_, _, _, g) = three();
    assert_eq!(g.version(), 3);
    let copy = GroupState::from_parts(g.chain_signed(), g.records_signed()).unwrap();
    assert_eq!(copy.version(), 3);
    // A joiner anchored at v2 accepts v3.
    let mut joiner = GroupState::from_anchor(g.chain()[1].signed.clone()).unwrap();
    assert_eq!(
        joiner.apply_versions(&g.chain_signed()).unwrap(),
        Applied::Advanced {
            from: 2,
            to: 3,
            lost: vec![]
        }
    );
    // Re-applying is a no-op.
    assert_eq!(
        joiner.apply_versions(&g.chain_signed()).unwrap(),
        Applied::Unchanged
    );
}

#[test]
fn signer_must_be_member_of_parent() {
    let (a, b, _, mut g) = three();
    let b_id = b.device_id();
    g.propose(&a, 4, |m| m.retain(|x| x.identity().device_id() != b_id))
        .unwrap();
    // B, removed in v4, cannot sign v5.
    let err = g.clone().propose(&b, 5, |m| m.clear()).unwrap_err();
    assert_eq!(err.code(), "not_member");
    // A device at v3 ignores a v4 signed by a stranger, then accepts the real v4.
    let mut at_v3 = GroupState::from_parts(g.chain_signed()[..3].to_vec(), vec![]).unwrap();
    let forged = SignedObject::sign(&keys(), DOMAIN_GROUP, &g.current().clone()).unwrap();
    assert_eq!(at_v3.apply_versions(&[forged]).unwrap(), Applied::Unchanged);
    assert_eq!(
        at_v3.apply_versions(&g.chain_signed()[3..]).unwrap(),
        Applied::Advanced {
            from: 3,
            to: 4,
            lost: vec![]
        }
    );
}

#[test]
fn gap_is_a_conflict_and_other_group_is_rejected() {
    let (a, _, _, g) = three();
    let mut joiner = GroupState::from_anchor(g.chain()[0].signed.clone()).unwrap();
    // Only v3 without v2: cannot chain.
    let err = joiner.apply_versions(&g.chain_signed()[2..]).unwrap_err();
    assert_eq!(err.code(), "group_conflict");
    let other = GroupState::create(&a, "A", Platform::Macos, 1, [8; 16]).unwrap();
    let err = joiner.apply_versions(&other.chain_signed()).unwrap_err();
    assert_eq!(err.code(), "group_conflict");
}

#[test]
fn fork_smaller_hash_wins_everywhere_and_loser_reapplies() {
    let (a, b, c, g) = three();
    let mut at_a = g.clone();
    let mut at_b = g.clone();
    // Concurrent v4: A renames C, B adds D.
    let c_id = c.device_id();
    at_a.propose(&a, 10, |m| {
        m.iter_mut()
            .find(|x| x.identity().device_id() == c_id)
            .unwrap()
            .name = "C2".into()
    })
    .unwrap();
    let d = keys();
    let md = member(&d.public(), "D");
    at_b.propose(&b, 11, |m| m.push(md)).unwrap();
    let (va, vb) = (at_a.chain()[3].clone(), at_b.chain()[3].clone());
    assert_ne!(va.hash, vb.hash);

    let a_res = at_a.apply_versions(&at_b.chain_signed()).unwrap();
    let b_res = at_b.apply_versions(&at_a.chain_signed()).unwrap();
    // Both converge on the same v4.
    assert_eq!(at_a.chain()[3].hash, at_b.chain()[3].hash);
    let winner = if va.hash < vb.hash { &va } else { &vb };
    assert_eq!(at_a.chain()[3].hash, winner.hash);
    // Exactly one side lost a version.
    let lost: Vec<_> = [a_res, b_res]
        .into_iter()
        .filter_map(|r| match r {
            Applied::Advanced { lost, .. } if !lost.is_empty() => Some(lost),
            _ => None,
        })
        .collect();
    assert_eq!(lost.len(), 1);
    let lost_list = &lost[0][0];
    // The loser's change is re-applied on top of the winner as v5.
    let (loser_state, loser_keys) = if va.hash < vb.hash {
        (&mut at_b, &b)
    } else {
        (&mut at_a, &a)
    };
    let change = loser_state.lost_change(lost_list).unwrap();
    assert!(!change.is_empty());
    let mut members = loser_state.current().members.clone();
    change.apply(&mut members);
    loser_state
        .propose(loser_keys, 12, |m| *m = members)
        .unwrap();
    assert_eq!(loser_state.version(), 5);
    let names: Vec<_> = loser_state
        .current()
        .members
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert!(names.contains(&"C2") && names.contains(&"D"), "{names:?}");
}

#[test]
fn records_need_member_self_signature_and_higher_seq() {
    let (_, b, _, mut g) = three();
    assert!(g.apply_record(&record(&b, 2)).unwrap());
    assert!(!g.apply_record(&record(&b, 2)).unwrap());
    assert!(!g.apply_record(&record(&b, 1)).unwrap());
    assert!(g.apply_record(&record(&b, 3)).unwrap());
    assert_eq!(g.record(&b.device_id()).unwrap().seq, 3);
    let stranger = keys();
    assert_eq!(
        g.apply_record(&record(&stranger, 1)).unwrap_err().code(),
        "not_member"
    );
    // B signing a record for C is rejected.
    let mut bad = record(&b, 4);
    bad.signer = g.current().members[2].identity().device_id();
    assert!(g.apply_record(&bad).is_err());
}

#[test]
fn change_between_versions() {
    let (_, _, _, g) = three();
    let c = Change::between(&g.chain()[1].list, &g.chain()[2].list);
    assert_eq!(c.added.len(), 1);
    assert!(c.removed.is_empty() && c.renamed.is_empty());
}

/// Review finding: a removed (e.g. stolen) device signs a competing version that removes the
/// remover and grinds its hash below the remover's. It must not win.
#[test]
fn removed_device_cannot_undo_its_removal_by_grinding() {
    let (a, b, x) = (keys(), keys(), keys());
    let mut g = GroupState::create(&a, "A", Platform::Macos, 1, [7; 16]).unwrap();
    let (mb, mx) = (member(&b.public(), "B"), member(&x.public(), "X"));
    g.propose(&a, 2, |m| m.push(mb)).unwrap();
    g.propose(&a, 3, |m| m.push(mx)).unwrap();
    let base = g.clone();
    let (xid, aid) = (x.device_id(), a.device_id());
    g.propose(&a, 10, |m| m.retain(|y| y.identity().device_id() != xid))
        .unwrap();
    let target = g.current_version().hash;
    let mut forged = base.clone();
    for t in 10.. {
        let mut attempt = base.clone();
        attempt
            .propose(&x, t, |m| m.retain(|y| y.identity().device_id() != aid))
            .unwrap();
        if attempt.current_version().hash < target {
            forged = attempt;
            break;
        }
    }
    assert_eq!(
        g.apply_versions(&forged.chain_signed()).unwrap(),
        Applied::Unchanged
    );
    assert!(g.is_member(&aid) && !g.is_member(&xid));
}

/// A version that removes the signer of the competing version wins regardless of hash.
#[test]
fn removal_of_the_competing_signer_wins_the_fork() {
    let (a, b, c, g) = three();
    let (bid, cid) = (b.device_id(), c.device_id());
    // A removes B; B (concurrently, unaware) renames C. A's version must win at B and at A.
    for t in 10..40 {
        let mut at_a = g.clone();
        let mut at_b = g.clone();
        at_a.propose(&a, t, |m| m.retain(|y| y.identity().device_id() != bid))
            .unwrap();
        at_b.propose(&b, t, |m| {
            m.iter_mut()
                .find(|y| y.identity().device_id() == cid)
                .unwrap()
                .name = "C2".into()
        })
        .unwrap();
        let a_head = at_a.current_version().hash;
        at_b.apply_versions(&at_a.chain_signed()).unwrap();
        at_a.apply_versions(&at_b.chain_signed()).unwrap();
        assert_eq!(at_b.current_version().hash, a_head, "t={t}");
        assert_eq!(at_a.current_version().hash, a_head, "t={t}");
    }
}

/// Review finding: a joiner anchored on a version that then loses a fork must still follow the
/// winner. With the parent in the accept payload (D20) its anchor is the parent.
#[test]
fn joiner_follows_fork_at_its_first_version() {
    let (a, b, j) = (keys(), keys(), keys());
    let mut ga = GroupState::create(&a, "A", Platform::Macos, 1, [7; 16]).unwrap();
    let mb = member(&b.public(), "B");
    ga.propose(&a, 2, |m| m.push(mb)).unwrap();
    let mut gb = ga.clone();
    // Concurrently: A adds J (pairing), B renames itself; pick timestamps so A's version loses.
    for t in 10.. {
        let (mut ga2, mut gb2) = (ga.clone(), gb.clone());
        let mj = member(&j.public(), "J");
        ga2.propose(&a, t, |m| m.push(mj)).unwrap();
        gb2.propose(&b, t, |m| m[1].name = "B2".into()).unwrap();
        if gb2.current_version().hash < ga2.current_version().hash {
            (ga, gb) = (ga2, gb2);
            break;
        }
    }
    // J got v3 (A's, the loser) and its parent v2.
    let mut gj = GroupState::from_anchor(ga.chain()[1].signed.clone()).unwrap();
    gj.apply_versions(&[ga.current_version().signed.clone()])
        .unwrap();
    assert_eq!(gj.version(), 3);
    // A learns B's v3, loses, re-applies "add J" as v4, then removes B as v5.
    let Applied::Advanced { lost, .. } = ga.apply_versions(&gb.chain_signed()).unwrap() else {
        panic!("A must switch to B's version");
    };
    let change = ga.lost_change(&lost[0]).unwrap();
    let mut members = ga.current().members.clone();
    change.apply(&mut members);
    ga.propose(&a, 100, |m| *m = members).unwrap();
    let bid = b.device_id();
    ga.propose(&a, 101, |m| m.retain(|x| x.identity().device_id() != bid))
        .unwrap();
    gj.apply_versions(&ga.chain_signed()).unwrap();
    assert_eq!(gj.version(), 5);
    assert!(!gj.is_member(&bid) && gj.is_member(&j.device_id()));
}

//! Deterministic generator for `protocol/vectors/*.json`. Shared by `examples/gen_vectors.rs`
//! (writes the files) and `tests/vectors.rs` (checks the files are up to date).
//!
//! All secrets come from SHA-256 of fixed labels and all randomness from ChaCha20 seeded the
//! same way, so every run produces byte-identical output.

#![allow(dead_code)]

use std::collections::BTreeMap;

use kitedrop_core::auth::{ProbeResponse, RequestAuth, RequestTarget};
use kitedrop_core::envelope::{self, ContentHeader, ContentKind, ENC_LEN, PREFIX_LEN};
use kitedrop_core::pairing::{self, AcceptPayload, Invite};
use kitedrop_core::signed::{
    DOMAIN_DEVICE_RECORD, DOMAIN_GROUP, DOMAIN_RECEIPT, DeviceRecord, GroupList, Member, Platform,
    Receipt, ReceiptStatus, SignedObject,
};
use kitedrop_core::wire::sha256;
use kitedrop_core::{DeviceKeys, PublicIdentity};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;
use serde_json::{Value, json};

pub const NOW: u64 = 1_790_000_000; // fixed clock for every vector

pub fn seed(label: &str) -> [u8; 32] {
    sha256(format!("kitedrop test vector: {label}").as_bytes())
}

pub fn rng(label: &str) -> ChaCha20Rng {
    ChaCha20Rng::from_seed(seed(&format!("rng/{label}")))
}

pub fn id16(label: &str) -> [u8; 16] {
    seed(label)[..16].try_into().unwrap()
}

/// Test devices: A = Mac (desktop), B = Android, C = iPhone.
pub fn device(name: &str) -> DeviceKeys {
    DeviceKeys::from_secret_bytes(&seed(&format!("{name}/sig")), &seed(&format!("{name}/kem")))
        .unwrap()
}

pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

/// Splits an envelope into `prefix||enc` and its records.
pub fn split(env: &[u8]) -> (Vec<u8>, Vec<(u8, Vec<u8>)>) {
    let head = env[..PREFIX_LEN + ENC_LEN].to_vec();
    let mut recs = Vec::new();
    let mut i = PREFIX_LEN + ENC_LEN;
    while i < env.len() {
        let len = u32::from_be_bytes(env[i + 1..i + 5].try_into().unwrap()) as usize;
        recs.push((env[i], env[i + 5..i + 5 + len].to_vec()));
        i += 5 + len;
    }
    (head, recs)
}

pub fn join(head: &[u8], recs: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = head.to_vec();
    for (t, ct) in recs {
        out.push(*t);
        out.extend_from_slice(&(ct.len() as u32).to_be_bytes());
        out.extend_from_slice(ct);
    }
    out
}

fn h(b: &[u8]) -> String {
    hex::encode(b)
}

fn identities() -> Value {
    let mut devs = serde_json::Map::new();
    for (name, platform) in [("A", "macos"), ("B", "android"), ("C", "ios")] {
        let k = device(name);
        let (sig_sk, kem_sk) = k.secret_bytes();
        let p = k.public();
        devs.insert(
            name.into(),
            json!({
                "platform": platform,
                "sig_sk": h(&*sig_sk),
                "kem_sk": h(&*kem_sk),
                "sig_pk": h(&p.sig_pk),
                "kem_pk": h(&p.kem_pk),
                "fingerprint": h(&p.fingerprint()),
                "device_id": h(&p.device_id()),
            }),
        );
    }
    json!({
        "description": "Device identities used by all other vectors. fingerprint = SHA-256(frame(['kitedrop/v1/fingerprint', sig_pk, kem_pk])); device_id = fingerprint[0..16].",
        "devices": devs,
    })
}

struct EnvCase {
    name: &'static str,
    from: &'static str,
    to: &'static str,
    header: ContentHeader,
    content: Vec<u8>,
}

fn header(
    kind: ContentKind,
    name: &str,
    mime: &str,
    size: usize,
    chunk_size: u32,
) -> ContentHeader {
    ContentHeader {
        v: 1,
        kind,
        name: name.into(),
        mime: mime.into(),
        size: size as u64,
        created_at: NOW,
        chunk_size,
    }
}

pub fn seal_case(name: &str, from: &str, to: &str, hdr: &ContentHeader, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    envelope::seal(
        &device(from),
        &device(to).public(),
        id16(&format!("transfer/{name}")),
        hdr,
        content,
        &mut out,
        &mut rng(&format!("envelope/{name}")),
    )
    .unwrap();
    out
}

fn envelopes() -> Value {
    let text = b"hello from kitedrop".to_vec();
    let link = b"https://example.com/kitedrop?x=1".to_vec();
    let cases = vec![
        EnvCase {
            name: "text",
            from: "A",
            to: "B",
            header: header(
                ContentKind::Text,
                "",
                "text/plain; charset=utf-8",
                text.len(),
                65536,
            ),
            content: text,
        },
        EnvCase {
            name: "link",
            from: "B",
            to: "C",
            header: header(ContentKind::Link, "", "text/uri-list", link.len(), 65536),
            content: link,
        },
        EnvCase {
            name: "file-multichunk",
            from: "B",
            to: "A",
            header: header(ContentKind::File, "photo.jpg", "image/jpeg", 200, 64),
            content: pattern(200),
        },
        EnvCase {
            name: "file-exact-multiple",
            from: "A",
            to: "B",
            header: header(
                ContentKind::File,
                "notes.bin",
                "application/octet-stream",
                128,
                64,
            ),
            content: pattern(128),
        },
        EnvCase {
            name: "file-empty",
            from: "C",
            to: "A",
            header: header(ContentKind::File, "empty.txt", "text/plain", 0, 64),
            content: vec![],
        },
    ];

    let mut positive = Vec::new();
    for c in &cases {
        let env = seal_case(c.name, c.from, c.to, &c.header, &c.content);
        positive.push(json!({
            "name": c.name,
            "sender": c.from,
            "recipient": c.to,
            "transfer_id": h(&id16(&format!("transfer/{}", c.name))),
            "rng_seed": h(&seed(&format!("rng/envelope/{}", c.name))),
            "header": serde_json::to_value(&c.header).unwrap(),
            "content": h(&c.content),
            "envelope": h(&env),
        }));
    }

    // Negative cases, all derived from file-multichunk (B -> A, records: header, 3 chunks, final).
    let base = &cases[2];
    let env = seal_case(base.name, base.from, base.to, &base.header, &base.content);
    let (head, recs) = split(&env);
    let mut negative = Vec::new();
    let mut neg = |name: &str, opened_by: &str, bytes: Vec<u8>, error: &str, why: &str| {
        negative.push(json!({
            "name": name, "recipient": opened_by, "envelope": h(&bytes), "error": error, "why": why,
        }));
    };

    neg(
        "truncated-missing-final",
        "A",
        join(&head, &recs[..recs.len() - 1]),
        "truncated",
        "the final chunk record was dropped",
    );
    neg(
        "truncated-mid-record",
        "A",
        env[..env.len() - 5].to_vec(),
        "truncated",
        "stream cut inside the last record",
    );
    let mut r = recs.clone();
    r.swap(1, 2);
    neg(
        "reordered-chunks",
        "A",
        join(&head, &r),
        "decrypt_failed",
        "chunk records 1 and 2 swapped; AAD index and HPKE sequence no longer match",
    );
    let mut r = recs.clone();
    r[1].0 = envelope::RECORD_FINAL_CHUNK;
    neg(
        "final-flag-forged",
        "A",
        join(&head, &r),
        "decrypt_failed",
        "first chunk relabelled FINAL_CHUNK to truncate early; record type is in the AAD",
    );
    let mut r = recs.clone();
    r[2].1[3] ^= 0x01;
    neg(
        "tampered-ciphertext",
        "A",
        join(&head, &r),
        "decrypt_failed",
        "one ciphertext bit flipped",
    );
    let mut extra = env.clone();
    extra.push(0);
    neg(
        "trailing-byte",
        "A",
        extra,
        "trailing_data",
        "one byte appended after the final record",
    );
    let mut r = recs.clone();
    r.push(recs[1].clone());
    neg(
        "appended-record",
        "A",
        join(&head, &r),
        "trailing_data",
        "a chunk record replayed after the final record",
    );
    let mut spoof = env.clone();
    spoof[8..24].copy_from_slice(&device("C").device_id());
    neg(
        "sender-spoofed",
        "A",
        spoof,
        "decrypt_failed",
        "sender field rewritten to C; C's key does not authenticate the HPKE context and the prefix is in HPKE info",
    );
    neg(
        "wrong-recipient",
        "B",
        env.clone(),
        "wrong_recipient",
        "opened by a device the envelope is not addressed to",
    );
    let mut redirect = env.clone();
    redirect[24..40].copy_from_slice(&device("C").device_id());
    neg(
        "recipient-rewritten",
        "C",
        redirect,
        "decrypt_failed",
        "recipient field rewritten to C; C cannot decapsulate a key sealed to A",
    );
    let mut magic = env.clone();
    magic[0] = b'X';
    neg("bad-magic", "A", magic, "bad_magic", "first byte changed");

    json!({
        "description": "Envelopes (HPKE mode_auth, DHKEM(X25519,HKDF-SHA256)/HKDF-SHA256/ChaCha20-Poly1305). Senders use ChaCha20Rng(rng_seed) as the only randomness source, so sealing is reproducible. 'content' is the plaintext. Negative cases must fail with 'error' when opened by 'recipient' (sender resolved from the prefix through identities.json).",
        "positive": positive,
        "negative": negative,
    })
}

pub fn member(name: &str, device_name: &str, platform: Platform) -> Member {
    let p = device(name).public();
    Member {
        sig_pk: p.sig_pk,
        kem_pk: p.kem_pk,
        name: device_name.into(),
        platform,
        added_at: NOW,
    }
}

pub fn group_v1() -> SignedObject {
    let g = GroupList {
        v: 1,
        group_id: id16("group"),
        version: 1,
        prev: None,
        members: vec![member("A", "Alice's Mac", Platform::Macos)],
        updated_at: NOW,
    };
    SignedObject::sign(&device("A"), DOMAIN_GROUP, &g).unwrap()
}

pub fn group_v2() -> SignedObject {
    let g = GroupList {
        v: 1,
        group_id: id16("group"),
        version: 2,
        prev: Some(sha256(&group_v1().body)),
        members: vec![
            member("A", "Alice's Mac", Platform::Macos),
            member("B", "Pixel", Platform::Android),
        ],
        updated_at: NOW + 60,
    };
    SignedObject::sign(&device("A"), DOMAIN_GROUP, &g).unwrap()
}

pub fn record(name: &str, seq: u64, addresses: &[&str], push_token: Option<&str>) -> SignedObject {
    let r = DeviceRecord {
        v: 1,
        device_id: device(name).device_id(),
        seq,
        addresses: addresses.iter().map(|s| s.to_string()).collect(),
        push_token: push_token.map(String::from),
        updated_at: NOW,
    };
    SignedObject::sign(&device(name), DOMAIN_DEVICE_RECORD, &r).unwrap()
}

fn signed() -> Value {
    let receipt = Receipt {
        v: 1,
        transfer_id: id16("transfer/file-multichunk"),
        sender: device("B").device_id(),
        recipient: device("A").device_id(),
        status: ReceiptStatus::Stored,
        at: NOW + 5,
        code: None,
    };
    let receipt = SignedObject::sign(&device("A"), DOMAIN_RECEIPT, &receipt).unwrap();
    let mut tampered = receipt.clone();
    let last = tampered.body.len() - 2;
    tampered.body[last] ^= 0x01;

    let obj = |domain: &str, signer: &str, o: &SignedObject| {
        json!({
            "domain": domain,
            "signer": signer,
            "body_utf8": String::from_utf8(o.body.clone()).unwrap(),
            "signed_object": serde_json::to_value(o).unwrap(),
        })
    };
    json!({
        "description": "Signed objects. sig = Ed25519(frame([domain, body])). Verifiers check the signature over the exact body bytes, then parse JSON. 'signed_object' is the wire form (base64url fields).",
        "positive": [
            obj("kitedrop/v1/group", "A", &group_v1()),
            obj("kitedrop/v1/group", "A", &group_v2()),
            obj("kitedrop/v1/device-record", "A", &record("A", 7, &["192.168.1.20:52525"], None)),
            obj("kitedrop/v1/device-record", "C", &record("C", 3, &[], Some("ExponentPushToken[test-token-0000000000]"))),
            obj("kitedrop/v1/receipt", "A", &receipt),
        ],
        "negative": [
            { "name": "receipt-body-tampered", "domain": "kitedrop/v1/receipt", "signer": "A",
              "signed_object": serde_json::to_value(&tampered).unwrap(), "error": "bad_signature" },
            { "name": "receipt-wrong-domain", "domain": "kitedrop/v1/group", "signer": "A",
              "signed_object": serde_json::to_value(&receipt).unwrap(), "error": "bad_signature" },
            { "name": "receipt-wrong-signer", "domain": "kitedrop/v1/receipt", "signer": "B",
              "signed_object": serde_json::to_value(&receipt).unwrap(), "error": "bad_signature" },
        ],
    })
}

fn pairing_vectors() -> Value {
    let inviter = device("A");
    let joiner = device("B");
    let invite = Invite {
        code: seed("pair/code"),
        inviter: inviter.public().fingerprint(),
        addresses: vec!["192.168.1.20:52525".into(), "[fd00::20]:52525".into()],
        reverse: false,
    };
    let nonce = id16("pair/nonce");
    let join = pairing::seal_join(
        &joiner,
        "Pixel",
        Platform::Android,
        &invite,
        &inviter.public(),
        nonce,
        &mut rng("pair/join"),
    )
    .unwrap();
    let payload = AcceptPayload {
        v: 1,
        nonce,
        group: group_v2(),
        records: vec![record("A", 7, &["192.168.1.20:52525"], None)],
        parent: None,
    };
    let accept = pairing::seal_accept(
        &inviter,
        &invite.code,
        &joiner.public(),
        &payload,
        &mut rng("pair/accept"),
    )
    .unwrap();
    let mut bad_accept = accept.clone();
    bad_accept.ct[0] ^= 1;

    // Reverse QR (D15): B cannot scan, shows the QR itself; A (member) scans it.
    let rev_invite = Invite {
        code: seed("pair/rev-code"),
        inviter: joiner.public().fingerprint(),
        addresses: vec!["192.168.1.30:52525".into()],
        reverse: true,
    };
    let offer_nonce = id16("pair/rev-offer-nonce");
    let offer = pairing::seal_offer(
        &inviter,
        "Alice's Mac",
        Platform::Macos,
        &rev_invite,
        &joiner.public(),
        offer_nonce,
        &mut rng("pair/rev-offer"),
    )
    .unwrap();
    let rev_nonce = id16("pair/rev-join-nonce");
    let rev_join = pairing::seal_join(
        &joiner,
        "Desk",
        Platform::Windows,
        &rev_invite,
        &inviter.public(),
        rev_nonce,
        &mut rng("pair/rev-join"),
    )
    .unwrap();
    let rev_payload = AcceptPayload {
        nonce: rev_nonce,
        ..payload.clone()
    };
    let rev_accept = pairing::AcceptDelivery {
        psk_id: pairing::psk_id(&rev_invite.code),
        accept: pairing::seal_accept(
            &inviter,
            &rev_invite.code,
            &joiner.public(),
            &rev_payload,
            &mut rng("pair/rev-accept"),
        )
        .unwrap(),
    };

    json!({
        "description": "QR pairing. A (inviter) shows 'uri'; B (joiner) sends 'join' (HPKE mode_auth_psk, psk = code, psk_id = 'psk_id') and receives 'accept'. rng seeds: join uses rng('pair/join'), accept uses rng('pair/accept') (seeds listed).",
        "inviter": "A",
        "joiner": "B",
        "code": h(&invite.code),
        "psk_id": h(&pairing::psk_id(&invite.code)),
        "uri": invite.to_uri(),
        "joiner_name": "Pixel",
        "joiner_platform": "android",
        "nonce": h(&nonce),
        "join_rng_seed": h(&seed("rng/pair/join")),
        "accept_rng_seed": h(&seed("rng/pair/accept")),
        "join": serde_json::to_value(&join).unwrap(),
        "accept_payload": serde_json::to_value(&payload).unwrap(),
        "accept": serde_json::to_value(&accept).unwrap(),
        "reverse": {
            "description": "Reverse QR: B (joiner) shows 'uri' (r=1, f = B's fingerprint). A probes B, posts 'offer' to /v1/pair/offer (domains kitedrop/v1/pair-offer, kitedrop/v1/pair-offer-sig, info binds B's fingerprint); B answers 'join' sealed to A (as in the forward flow, info binds A's fingerprint); A posts 'accept' to /v1/pair/accept. rng seeds: rng('pair/rev-offer'), rng('pair/rev-join'), rng('pair/rev-accept').",
            "code": h(&rev_invite.code),
            "uri": rev_invite.to_uri(),
            "member_name": "Alice's Mac",
            "member_platform": "macos",
            "offer_nonce": h(&offer_nonce),
            "offer": serde_json::to_value(&offer).unwrap(),
            "joiner_name": "Desk",
            "joiner_platform": "windows",
            "nonce": h(&rev_nonce),
            "join": serde_json::to_value(&rev_join).unwrap(),
            "accept_payload": serde_json::to_value(&rev_payload).unwrap(),
            "accept": serde_json::to_value(&rev_accept).unwrap(),
        },
        "negative": [
            { "name": "join-wrong-code", "step": "open_join", "code": h(&seed("pair/other-code")), "error": "decrypt_failed",
              "why": "inviter holds a different code; psk_id does not match" },
            { "name": "accept-tampered", "step": "open_accept", "accept": serde_json::to_value(&bad_accept).unwrap(), "error": "decrypt_failed",
              "why": "one ciphertext bit flipped" },
        ],
    })
}

fn auth_vectors() -> Value {
    let a = device("A");
    let nonce = seed("probe/nonce");
    let probe = ProbeResponse::sign(&a, &nonce, id16("group"), 2, 7, NOW);

    let challenge = seed("challenge/1");
    let outbox = RequestTarget {
        server: a.device_id(),
        method: "GET",
        path: "/v1/outbox",
        content_length: 0,
    };
    let pull = RequestAuth::sign(&device("C"), &outbox, NOW, &challenge, 1);

    let hdr = header(
        ContentKind::Text,
        "",
        "text/plain; charset=utf-8",
        19,
        65536,
    );
    let env = seal_case("text", "A", "B", &hdr, b"hello from kitedrop");
    let b = device("B");
    let tid = id16("transfer/text");
    let upload = RequestTarget {
        server: b.device_id(),
        method: "POST",
        path: "/v1/inbox",
        content_length: env.len() as u64,
    };
    let up = RequestAuth::sign(&a, &upload, NOW, &tid, 0);

    let target = |t: &RequestTarget| {
        json!({
            "server": h(&t.server), "method": t.method, "path": t.path, "content_length": t.content_length,
        })
    };
    json!({
        "description": "Endpoint authentication. probe: listener signs frame(['kitedrop/v1/probe', device_id, nonce, group_id, u64 group_version, u64 record_seq, u64 ts]). request: client signs frame(['kitedrop/v1/request', server, client, method, path, u64 content_length, u64 ts, nonce, u64 seq]); u64 = 8-byte big-endian.",
        "probe": {
            "listener": "A",
            "nonce": h(&nonce),
            "response": serde_json::to_value(&probe).unwrap(),
        },
        "requests": [
            { "name": "outbox-pull-with-challenge", "client": "C", "target": target(&outbox),
              "authorization": pull.to_header() },
            { "name": "upload-bound-to-transfer-id", "client": "A", "target": target(&upload),
              "authorization": up.to_header(), "note": "nonce = transfer_id of envelope 'text', seq = 0" },
        ],
        "negative": [
            { "name": "path-changed", "client": "C", "authorization": pull.to_header(),
              "target": target(&RequestTarget { path: "/v1/outbox/x", ..outbox.clone() }), "error": "bad_signature" },
            { "name": "other-server", "client": "C", "authorization": pull.to_header(),
              "target": target(&RequestTarget { server: b.device_id(), ..outbox.clone() }), "error": "bad_signature" },
            { "name": "length-changed", "client": "A", "authorization": up.to_header(),
              "target": target(&RequestTarget { content_length: env.len() as u64 + 1, ..upload.clone() }), "error": "bad_signature" },
        ],
    })
}

/// File name → pretty JSON (with trailing newline).
pub fn generate() -> BTreeMap<&'static str, String> {
    let mut files = BTreeMap::new();
    for (name, v) in [
        ("identities.json", identities()),
        ("envelope.json", envelopes()),
        ("signed.json", signed()),
        ("pairing.json", pairing_vectors()),
        ("auth.json", auth_vectors()),
    ] {
        files.insert(name, serde_json::to_string_pretty(&v).unwrap() + "\n");
    }
    files
}

pub fn identity_from(vectors: &Value, name: &str) -> (DeviceKeys, PublicIdentity) {
    let d = &vectors["devices"][name];
    let b = |k: &str| -> [u8; 32] {
        hex::decode(d[k].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    };
    let keys = DeviceKeys::from_secret_bytes(&b("sig_sk"), &b("kem_sk")).unwrap();
    let public = keys.public();
    (keys, public)
}

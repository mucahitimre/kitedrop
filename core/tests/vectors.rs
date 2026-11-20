//! Runs the core against `protocol/vectors/*.json`. The checks read only the JSON files, the way
//! another implementation would; `vectors_are_current` additionally ensures the files match the
//! generator.

mod support;

use std::path::PathBuf;

use kitedrop_core::auth::{ProbeResponse, RequestAuth, RequestTarget};
use kitedrop_core::envelope::{self, ContentHeader, EnvelopeInfo};
use kitedrop_core::pairing::{self, AcceptMessage, AcceptPayload, Invite, JoinMessage};
use kitedrop_core::signed::SignedObject;
use kitedrop_core::{DeviceKeys, Error, PublicIdentity};
use serde_json::Value;
use support::vectors;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../protocol/vectors")
}

fn load(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

fn hex(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn ids() -> Value {
    load("identities.json")
}

fn keys(name: &str) -> DeviceKeys {
    vectors::identity_from(&ids(), name).0
}

fn public(name: &str) -> PublicIdentity {
    vectors::identity_from(&ids(), name).1
}

/// Resolves a sender device id against the identities file, like a group lookup.
fn resolve(info: &EnvelopeInfo) -> kitedrop_core::Result<PublicIdentity> {
    ["A", "B", "C"]
        .iter()
        .map(|n| public(n))
        .find(|p| p.device_id() == info.sender)
        .ok_or(Error::UnknownSender)
}

#[test]
fn vectors_are_current() {
    for (name, body) in vectors::generate() {
        let on_disk = std::fs::read_to_string(dir().join(name)).unwrap_or_default();
        assert!(
            on_disk == body,
            "protocol/vectors/{name} is stale: run `cargo run -p kitedrop-core --example gen_vectors`"
        );
    }
}

#[test]
fn identities_derive_from_secrets() {
    let v = ids();
    for (name, d) in v["devices"].as_object().unwrap() {
        let p = public(name);
        assert_eq!(hex(&d["sig_pk"]), p.sig_pk);
        assert_eq!(hex(&d["kem_pk"]), p.kem_pk);
        assert_eq!(hex(&d["fingerprint"]), p.fingerprint());
        assert_eq!(hex(&d["device_id"]), p.device_id());
    }
}

#[test]
fn envelopes_open() {
    let v = load("envelope.json");
    for c in v["positive"].as_array().unwrap() {
        let mut out = Vec::new();
        let recipient = keys(c["recipient"].as_str().unwrap());
        let (info, header) = envelope::open(
            &recipient,
            hex(&c["envelope"]).as_slice(),
            &mut out,
            resolve,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", c["name"]));
        let expected: ContentHeader = serde_json::from_value(c["header"].clone()).unwrap();
        assert_eq!(header, expected, "{}", c["name"]);
        assert_eq!(out, hex(&c["content"]), "{}", c["name"]);
        assert_eq!(info.transfer_id.to_vec(), hex(&c["transfer_id"]));
        assert_eq!(
            info.sender,
            public(c["sender"].as_str().unwrap()).device_id()
        );
    }
}

#[test]
fn envelopes_reseal_identically() {
    // Sealing with the documented rng seed must reproduce the vector byte for byte.
    let v = load("envelope.json");
    for c in v["positive"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let header: ContentHeader = serde_json::from_value(c["header"].clone()).unwrap();
        let env = vectors::seal_case(
            name,
            c["sender"].as_str().unwrap(),
            c["recipient"].as_str().unwrap(),
            &header,
            &hex(&c["content"]),
        );
        assert_eq!(env, hex(&c["envelope"]), "{name}");
    }
}

#[test]
fn envelope_attacks_fail() {
    let v = load("envelope.json");
    for c in v["negative"].as_array().unwrap() {
        let recipient = keys(c["recipient"].as_str().unwrap());
        let err = envelope::open(
            &recipient,
            hex(&c["envelope"]).as_slice(),
            std::io::sink(),
            resolve,
        )
        .expect_err(c["name"].as_str().unwrap());
        assert_eq!(
            err.code(),
            c["error"].as_str().unwrap(),
            "{}: {err}",
            c["name"]
        );
    }
}

#[test]
fn signed_objects() {
    let v = load("signed.json");
    for c in v["positive"].as_array().unwrap() {
        let o: SignedObject = serde_json::from_value(c["signed_object"].clone()).unwrap();
        let body: Value = o
            .verify(
                c["domain"].as_str().unwrap().as_bytes(),
                &public(c["signer"].as_str().unwrap()),
            )
            .unwrap();
        assert_eq!(
            body,
            serde_json::from_str::<Value>(c["body_utf8"].as_str().unwrap()).unwrap()
        );
    }
    for c in v["negative"].as_array().unwrap() {
        let o: SignedObject = serde_json::from_value(c["signed_object"].clone()).unwrap();
        let err = o
            .verify::<Value>(
                c["domain"].as_str().unwrap().as_bytes(),
                &public(c["signer"].as_str().unwrap()),
            )
            .expect_err(c["name"].as_str().unwrap());
        assert_eq!(err.code(), c["error"].as_str().unwrap());
    }
}

#[test]
fn pairing_round_trip() {
    let v = load("pairing.json");
    let invite = Invite::from_uri(v["uri"].as_str().unwrap()).unwrap();
    assert_eq!(invite.code.to_vec(), hex(&v["code"]));
    assert_eq!(invite.inviter, public("A").fingerprint());
    assert_eq!(pairing::psk_id(&invite.code).to_vec(), hex(&v["psk_id"]));

    let join: JoinMessage = serde_json::from_value(v["join"].clone()).unwrap();
    let req = pairing::open_join(&keys("A"), &invite.code, &join).unwrap();
    assert_eq!(req.identity(), public("B"));
    assert_eq!(req.name, v["joiner_name"].as_str().unwrap());
    assert_eq!(req.nonce.to_vec(), hex(&v["nonce"]));

    let accept: AcceptMessage = serde_json::from_value(v["accept"].clone()).unwrap();
    let nonce: [u8; 16] = hex(&v["nonce"]).try_into().unwrap();
    let payload = pairing::open_accept(&keys("B"), &invite, &public("A"), &nonce, &accept).unwrap();
    let expected: AcceptPayload = serde_json::from_value(v["accept_payload"].clone()).unwrap();
    assert_eq!(payload, expected);

    for c in v["negative"].as_array().unwrap() {
        let err = match c["step"].as_str().unwrap() {
            "open_join" => {
                let code: [u8; 32] = hex(&c["code"]).try_into().unwrap();
                pairing::open_join(&keys("A"), &code, &join).unwrap_err()
            }
            "open_accept" => {
                let bad: AcceptMessage = serde_json::from_value(c["accept"].clone()).unwrap();
                pairing::open_accept(&keys("B"), &invite, &public("A"), &nonce, &bad).unwrap_err()
            }
            s => panic!("unknown step {s}"),
        };
        assert_eq!(err.code(), c["error"].as_str().unwrap(), "{}", c["name"]);
    }

    // Reverse QR: B shows, A scans.
    let r = &v["reverse"];
    let rev = Invite::from_uri(r["uri"].as_str().unwrap()).unwrap();
    assert!(rev.reverse);
    assert_eq!(rev.inviter, public("B").fingerprint());
    let offer: JoinMessage = serde_json::from_value(r["offer"].clone()).unwrap();
    let member = pairing::open_offer(&keys("B"), &rev.code, &offer).unwrap();
    assert_eq!(member.identity(), public("A"));
    assert_eq!(member.nonce.to_vec(), hex(&r["offer_nonce"]));
    assert_eq!(
        pairing::open_join(&keys("B"), &rev.code, &offer)
            .unwrap_err()
            .code(),
        "decrypt_failed",
        "an offer must not open as a join"
    );
    let join: JoinMessage = serde_json::from_value(r["join"].clone()).unwrap();
    let req = pairing::open_join(&keys("A"), &rev.code, &join).unwrap();
    assert_eq!(req.identity(), public("B"));
    let delivery: pairing::AcceptDelivery = serde_json::from_value(r["accept"].clone()).unwrap();
    assert_eq!(delivery.psk_id, pairing::psk_id(&rev.code));
    let rev_nonce: [u8; 16] = hex(&r["nonce"]).try_into().unwrap();
    let payload =
        pairing::open_accept(&keys("B"), &rev, &public("A"), &rev_nonce, &delivery.accept).unwrap();
    let expected: AcceptPayload = serde_json::from_value(r["accept_payload"].clone()).unwrap();
    assert_eq!(payload, expected);
}

fn target(t: &Value) -> (Vec<u8>, String, String, u64) {
    (
        hex(&t["server"]),
        t["method"].as_str().unwrap().to_owned(),
        t["path"].as_str().unwrap().to_owned(),
        t["content_length"].as_u64().unwrap(),
    )
}

fn check_request(c: &Value) -> kitedrop_core::Result<()> {
    let (server, method, path, content_length) = target(&c["target"]);
    let auth = RequestAuth::from_header(c["authorization"].as_str().unwrap())?;
    let t = RequestTarget {
        server: server.try_into().unwrap(),
        method: &method,
        path: &path,
        content_length,
    };
    auth.verify(&public(c["client"].as_str().unwrap()), &t)
}

#[test]
fn endpoint_auth() {
    let v = load("auth.json");
    let p = &v["probe"];
    let resp: ProbeResponse = serde_json::from_value(p["response"].clone()).unwrap();
    let nonce: [u8; 32] = hex(&p["nonce"]).try_into().unwrap();
    resp.verify(&nonce).unwrap();
    assert_eq!(resp.identity(), public("A"));
    let mut other = nonce;
    other[0] ^= 1;
    assert_eq!(resp.verify(&other).unwrap_err().code(), "bad_signature");

    for c in v["requests"].as_array().unwrap() {
        check_request(c).unwrap_or_else(|e| panic!("{}: {e}", c["name"]));
    }
    for c in v["negative"].as_array().unwrap() {
        assert_eq!(
            check_request(c).unwrap_err().code(),
            c["error"].as_str().unwrap(),
            "{}",
            c["name"]
        );
    }
}

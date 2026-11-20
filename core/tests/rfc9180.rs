//! Checks the HPKE library against the official RFC 9180 test vectors for the kitedrop suite
//! (`protocol/vectors/rfc9180-subset.json`), independently of our own generator.

use std::convert::Infallible;

use hpke::kem::X25519HkdfSha256 as Kem;
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, PskBundle, Serializable};
use hpke::{aead::ChaCha20Poly1305 as Aead, kdf::HkdfSha256 as Kdf};
use serde_json::Value;

/// Hands out fixed bytes, so `setup_sender_with_rng` derives the RFC's ephemeral key from ikmE.
struct FixedRng(Vec<u8>);

impl rand_core::TryRng for FixedRng {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        unreachable!()
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        unreachable!()
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        let rest = self.0.split_off(dst.len());
        dst.copy_from_slice(&self.0);
        self.0 = rest;
        Ok(())
    }
}

impl rand_core::TryCryptoRng for FixedRng {}

fn hex(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

#[test]
fn hpke_matches_rfc9180() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../protocol/vectors/rfc9180-subset.json"
    );
    let file: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let vectors = file["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 2);
    for v in vectors {
        let (sk_r, pk_r) = Kem::derive_keypair(&hex(&v["ikmR"]));
        let (sk_s, pk_s) = Kem::derive_keypair(&hex(&v["ikmS"]));
        // The library stores X25519 scalars clamped, so compare public keys rather than skRm.
        assert_eq!(pk_r.to_bytes().to_vec(), hex(&v["pkRm"]));
        assert_eq!(pk_s.to_bytes().to_vec(), hex(&v["pkSm"]));
        let info = hex(&v["info"]);
        let auth_psk = v["mode"] == 3;
        let (psk, psk_id) = if auth_psk {
            (hex(&v["psk"]), hex(&v["psk_id"]))
        } else {
            (vec![], vec![])
        };

        let mode_s = if auth_psk {
            OpModeS::<Kem>::AuthPsk(
                (sk_s.clone(), pk_s.clone()),
                PskBundle::new(&psk, &psk_id).unwrap(),
            )
        } else {
            OpModeS::<Kem>::Auth((sk_s.clone(), pk_s.clone()))
        };
        let mut rng = FixedRng(hex(&v["ikmE"]));
        let (enc, mut ctx_s) =
            hpke::setup_sender_with_rng::<Aead, Kdf, Kem>(&mode_s, &pk_r, &info, &mut rng).unwrap();
        assert_eq!(enc.to_bytes().to_vec(), hex(&v["enc"]));

        let mode_r = if auth_psk {
            OpModeR::<Kem>::AuthPsk(pk_s.clone(), PskBundle::new(&psk, &psk_id).unwrap())
        } else {
            OpModeR::<Kem>::Auth(pk_s.clone())
        };
        let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(&hex(&v["enc"])).unwrap();
        let mut ctx_r =
            hpke::setup_receiver::<Aead, Kdf, Kem>(&mode_r, &sk_r, &enc, &info).unwrap();

        // Encryptions are consecutive sequence numbers starting at 0.
        for e in v["encryptions"].as_array().unwrap() {
            let (aad, pt, ct) = (hex(&e["aad"]), hex(&e["pt"]), hex(&e["ct"]));
            assert_eq!(ctx_s.seal(&pt, &aad).unwrap(), ct);
            assert_eq!(ctx_r.open(&ct, &aad).unwrap(), pt);
        }
    }
}

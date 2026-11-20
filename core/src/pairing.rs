//! QR pairing (protocol.md, "Pairing").
//!
//! The QR carries a 32-byte one-time code, the shower's fingerprint and its addresses. Every
//! pairing message is an HPKE mode_auth_psk single-shot seal with the code as PSK: each side
//! proves it saw the QR (PSK) and holds its X25519 key (auth). Hello messages (join, offer) are
//! also signed with the sender's Ed25519 key, and the scanner pins the shower's keys through the
//! fingerprint from the QR.
//!
//! Forward flow (a member shows the QR): joiner → `join` → member answers `accept`.
//! Reverse flow (the joiner shows the QR, D15): member → `offer` → joiner answers `join` →
//! member posts `accept` to the joiner.

use hpke::{Deserializable, OpModeR, OpModeS, PskBundle, Serializable};
use rand_core::CryptoRng;
use serde::{Deserialize, Serialize};

use crate::identity::{Aead, Kdf, Kem, parse_kem_public};
use crate::signed::{Platform, SignedObject};
use crate::wire::{
    PROTOCOL_VERSION, b64_array, b64_decode_array, b64_encode, b64_vec, frame, sha256,
};
use crate::{DeviceKeys, Error, Fingerprint, PublicIdentity, Result};

pub const CODE_LEN: usize = 32;
pub const CODE_TTL_SECS: u64 = 60;
pub const URI_PREFIX: &str = "kitedrop://pair?";

const DOMAIN_PSK_ID: &[u8] = b"kitedrop/v1/psk-id";
const DOMAIN_JOIN: &[u8] = b"kitedrop/v1/pair-join";
const DOMAIN_JOIN_SIG: &[u8] = b"kitedrop/v1/pair-join-sig";
const DOMAIN_ACCEPT: &[u8] = b"kitedrop/v1/pair-accept";
const DOMAIN_OFFER: &[u8] = b"kitedrop/v1/pair-offer";
const DOMAIN_OFFER_SIG: &[u8] = b"kitedrop/v1/pair-offer-sig";

pub fn psk_id(code: &[u8; CODE_LEN]) -> [u8; 16] {
    sha256(&frame(&[DOMAIN_PSK_ID, code]))[..16]
        .try_into()
        .expect("16 bytes")
}

/// Content of the pairing QR code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    pub code: [u8; CODE_LEN],
    /// Fingerprint of the device showing the QR (the inviter, or the joiner when `reverse`).
    pub inviter: Fingerprint,
    pub addresses: Vec<String>,
    /// Reverse QR (D15): shown by a device that wants to join; a member scans it.
    pub reverse: bool,
}

impl Invite {
    pub fn to_uri(&self) -> String {
        format!(
            "{URI_PREFIX}v={PROTOCOL_VERSION}&c={}&f={}&a={}{}",
            b64_encode(&self.code),
            b64_encode(&self.inviter),
            self.addresses.join(","),
            if self.reverse { "&r=1" } else { "" }
        )
    }

    pub fn from_uri(uri: &str) -> Result<Self> {
        let query = uri
            .strip_prefix(URI_PREFIX)
            .ok_or(Error::Malformed("pair uri"))?;
        let (mut v, mut code, mut inviter, mut addresses) = (None, None, None, Vec::new());
        let mut reverse = false;
        for pair in query.split('&') {
            let (k, val) = pair.split_once('=').ok_or(Error::Malformed("pair uri"))?;
            match k {
                "v" => v = val.parse::<u8>().ok(),
                "c" => code = b64_decode_array(val),
                "f" => inviter = b64_decode_array(val),
                "r" => reverse = val == "1",
                "a" => {
                    addresses = val
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                }
                _ => {} // unknown keys are ignored for forward compatibility
            }
        }
        if v != Some(PROTOCOL_VERSION) {
            return Err(Error::UnsupportedVersion);
        }
        Ok(Self {
            code: code.ok_or(Error::Malformed("pair code"))?,
            inviter: inviter.ok_or(Error::Malformed("pair fingerprint"))?,
            addresses,
            reverse,
        })
    }
}

/// `POST /v1/pair/join` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinMessage {
    #[serde(with = "b64_array")]
    pub psk_id: [u8; 16],
    #[serde(with = "b64_array")]
    pub joiner_kem_pk: [u8; 32],
    #[serde(with = "b64_array")]
    pub enc: [u8; 32],
    #[serde(with = "b64_vec")]
    pub ct: Vec<u8>,
}

/// Plaintext inside [`JoinMessage`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub v: u8,
    #[serde(with = "b64_array")]
    pub sig_pk: [u8; 32],
    #[serde(with = "b64_array")]
    pub kem_pk: [u8; 32],
    pub name: String,
    pub platform: Platform,
    #[serde(with = "b64_array")]
    pub nonce: [u8; 16],
    /// Ed25519 over frame([pair-join-sig, inviter fingerprint, psk_id, kem_pk, nonce]).
    #[serde(with = "b64_array")]
    pub sig: [u8; 64],
}

impl JoinRequest {
    pub fn identity(&self) -> PublicIdentity {
        PublicIdentity {
            sig_pk: self.sig_pk,
            kem_pk: self.kem_pk,
        }
    }
}

/// `POST /v1/pair/join` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptMessage {
    #[serde(with = "b64_array")]
    pub enc: [u8; 32],
    #[serde(with = "b64_vec")]
    pub ct: Vec<u8>,
}

/// Plaintext inside [`AcceptMessage`]: the new group list (already containing the joiner) and
/// the current device records of the members.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptPayload {
    pub v: u8,
    #[serde(with = "b64_array")]
    pub nonce: [u8; 16],
    pub group: SignedObject,
    pub records: Vec<SignedObject>,
    /// The version before `group`, so the joiner can resolve a fork at its first version (D20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SignedObject>,
}

fn psk_bundle<'a>(code: &'a [u8; CODE_LEN], id: &'a [u8; 16]) -> Result<PskBundle<'a>> {
    PskBundle::new(code, id).map_err(|_| Error::Malformed("psk"))
}

/// Sealed, signed self-introduction (join or offer) to the peer `to`.
#[allow(clippy::too_many_arguments)]
fn seal_hello(
    domains: (&[u8], &[u8]),
    me_keys: &DeviceKeys,
    name: &str,
    platform: Platform,
    code: &[u8; CODE_LEN],
    to: &PublicIdentity,
    nonce: [u8; 16],
    rng: &mut impl CryptoRng,
) -> Result<JoinMessage> {
    let (domain, sig_domain) = domains;
    let to_fp = to.fingerprint();
    let id = psk_id(code);
    let me = me_keys.public();
    let sig = me_keys.sign(sig_domain, &[&to_fp, &id, &me.kem_pk, &nonce]);
    let req = JoinRequest {
        v: PROTOCOL_VERSION,
        sig_pk: me.sig_pk,
        kem_pk: me.kem_pk,
        name: name.to_owned(),
        platform,
        nonce,
        sig,
    };
    let mode = OpModeS::<Kem>::AuthPsk(me_keys.kem_keypair(), psk_bundle(code, &id)?);
    let (enc, ct) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
        &mode,
        &to.kem_public()?,
        &frame(&[domain, &to_fp]),
        &serde_json::to_vec(&req)?,
        &[],
        rng,
    )
    .map_err(|_| Error::Malformed("peer key"))?;
    Ok(JoinMessage {
        psk_id: id,
        joiner_kem_pk: me.kem_pk,
        enc: enc.to_bytes().into(),
        ct,
    })
}

fn open_hello(
    domains: (&[u8], &[u8]),
    me_keys: &DeviceKeys,
    code: &[u8; CODE_LEN],
    msg: &JoinMessage,
) -> Result<JoinRequest> {
    let (domain, sig_domain) = domains;
    let id = psk_id(code);
    if id != msg.psk_id {
        return Err(Error::DecryptFailed);
    }
    let fp = me_keys.public().fingerprint();
    let sender_kem = parse_kem_public(&msg.joiner_kem_pk)?;
    let enc =
        <Kem as hpke::Kem>::EncappedKey::from_bytes(&msg.enc).map_err(|_| Error::DecryptFailed)?;
    let mode = OpModeR::<Kem>::AuthPsk(sender_kem, psk_bundle(code, &id)?);
    let pt = hpke::single_shot_open::<Aead, Kdf, Kem>(
        &mode,
        me_keys.kem_private(),
        &enc,
        &frame(&[domain, &fp]),
        &msg.ct,
        &[],
    )
    .map_err(|_| Error::DecryptFailed)?;
    let req: JoinRequest = serde_json::from_slice(&pt)?;
    if req.v != PROTOCOL_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    if req.kem_pk != msg.joiner_kem_pk {
        return Err(Error::Malformed("sender key mismatch"));
    }
    req.identity()
        .verify(sig_domain, &[&fp, &id, &req.kem_pk, &req.nonce], &req.sig)?;
    Ok(req)
}

/// Joiner side. `inviter` must already be checked against `invite.inviter` (fingerprint). In the
/// reverse flow `invite` is the joiner's own QR content and `inviter` the member from the offer.
#[allow(clippy::too_many_arguments)]
pub fn seal_join(
    joiner: &DeviceKeys,
    name: &str,
    platform: Platform,
    invite: &Invite,
    inviter: &PublicIdentity,
    nonce: [u8; 16],
    rng: &mut impl CryptoRng,
) -> Result<JoinMessage> {
    if !invite.reverse && inviter.fingerprint() != invite.inviter {
        return Err(Error::BadSignature);
    }
    seal_hello(
        (DOMAIN_JOIN, DOMAIN_JOIN_SIG),
        joiner,
        name,
        platform,
        &invite.code,
        inviter,
        nonce,
        rng,
    )
}

/// Inviter (member) side. The caller looks up the live, unused code by `msg.psk_id`, and marks
/// it used once this returns `Ok`.
pub fn open_join(
    inviter: &DeviceKeys,
    code: &[u8; CODE_LEN],
    msg: &JoinMessage,
) -> Result<JoinRequest> {
    open_hello((DOMAIN_JOIN, DOMAIN_JOIN_SIG), inviter, code, msg)
}

/// Reverse flow, member side: `POST /v1/pair/offer` body to the joiner whose QR (`invite`,
/// `reverse` set) was scanned and whose keys (`joiner`) were pinned by probing.
pub fn seal_offer(
    member: &DeviceKeys,
    name: &str,
    platform: Platform,
    invite: &Invite,
    joiner: &PublicIdentity,
    nonce: [u8; 16],
    rng: &mut impl CryptoRng,
) -> Result<JoinMessage> {
    if !invite.reverse || joiner.fingerprint() != invite.inviter {
        return Err(Error::BadSignature);
    }
    seal_hello(
        (DOMAIN_OFFER, DOMAIN_OFFER_SIG),
        member,
        name,
        platform,
        &invite.code,
        joiner,
        nonce,
        rng,
    )
}

/// Reverse flow, joiner side: opens an offer with the code of the QR this device shows. The
/// result identifies the member; the joiner answers with [`seal_join`] to that member.
pub fn open_offer(
    joiner: &DeviceKeys,
    code: &[u8; CODE_LEN],
    msg: &JoinMessage,
) -> Result<JoinRequest> {
    open_hello((DOMAIN_OFFER, DOMAIN_OFFER_SIG), joiner, code, msg)
}

/// Reverse flow: `POST /v1/pair/accept` body, member → joiner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptDelivery {
    #[serde(with = "b64_array")]
    pub psk_id: [u8; 16],
    #[serde(flatten)]
    pub accept: AcceptMessage,
}

fn accept_info(inviter: &Fingerprint, joiner: &Fingerprint) -> Vec<u8> {
    frame(&[DOMAIN_ACCEPT, inviter, joiner])
}

/// Inviter side, after [`open_join`] succeeded and the new group version is signed.
pub fn seal_accept(
    inviter: &DeviceKeys,
    code: &[u8; CODE_LEN],
    joiner: &PublicIdentity,
    payload: &AcceptPayload,
    rng: &mut impl CryptoRng,
) -> Result<AcceptMessage> {
    let id = psk_id(code);
    let mode = OpModeS::<Kem>::AuthPsk(inviter.kem_keypair(), psk_bundle(code, &id)?);
    let (enc, ct) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
        &mode,
        &joiner.kem_public()?,
        &accept_info(&inviter.public().fingerprint(), &joiner.fingerprint()),
        &serde_json::to_vec(payload)?,
        &[],
        rng,
    )
    .map_err(|_| Error::Malformed("joiner key"))?;
    Ok(AcceptMessage {
        enc: enc.to_bytes().into(),
        ct,
    })
}

/// Joiner side. Completes mutual authentication: only the holder of the inviter's pinned
/// X25519 key who also knows the code can produce this message.
pub fn open_accept(
    joiner: &DeviceKeys,
    invite: &Invite,
    inviter: &PublicIdentity,
    nonce: &[u8; 16],
    msg: &AcceptMessage,
) -> Result<AcceptPayload> {
    if !invite.reverse && inviter.fingerprint() != invite.inviter {
        return Err(Error::BadSignature);
    }
    let id = psk_id(&invite.code);
    let enc =
        <Kem as hpke::Kem>::EncappedKey::from_bytes(&msg.enc).map_err(|_| Error::DecryptFailed)?;
    let mode = OpModeR::<Kem>::AuthPsk(inviter.kem_public()?, psk_bundle(&invite.code, &id)?);
    let pt = hpke::single_shot_open::<Aead, Kdf, Kem>(
        &mode,
        joiner.kem_private(),
        &enc,
        &accept_info(&inviter.fingerprint(), &joiner.public().fingerprint()),
        &msg.ct,
        &[],
    )
    .map_err(|_| Error::DecryptFailed)?;
    let payload: AcceptPayload = serde_json::from_slice(&pt)?;
    if payload.v != PROTOCOL_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    if &payload.nonce != nonce {
        return Err(Error::Malformed("nonce mismatch"));
    }
    Ok(payload)
}

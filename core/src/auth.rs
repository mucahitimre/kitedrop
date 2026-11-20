//! Endpoint authentication (protocol.md, "Endpoint authentication").
//!
//! - Probe: the caller sends a random nonce; the listener signs it. Proves the listener's identity.
//! - Signed requests: the caller signs method, path, body length and a nonce bound to the
//!   listener. The nonce is a listener-issued challenge (interactive endpoints) or, for uploads,
//!   the envelope's transfer id (non-interactive, so iOS background uploads work).

use serde::{Deserialize, Serialize};

use crate::wire::{b64_array, b64_decode, b64_decode_array, b64_encode};
use crate::{DeviceId, DeviceKeys, Error, PublicIdentity, Result};

const DOMAIN_PROBE: &[u8] = b"kitedrop/v1/probe";
const DOMAIN_REQUEST: &[u8] = b"kitedrop/v1/request";
pub const SCHEME: &str = "KD1";
pub const CHALLENGE_TTL_SECS: u64 = 120;

/// `POST /v1/probe` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeRequest {
    #[serde(with = "b64_array")]
    pub nonce: [u8; 32],
}

/// `POST /v1/probe` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResponse {
    pub v: u8,
    #[serde(with = "b64_array")]
    pub sig_pk: [u8; 32],
    #[serde(with = "b64_array")]
    pub kem_pk: [u8; 32],
    #[serde(with = "b64_array")]
    pub group_id: [u8; 16],
    pub group_version: u64,
    pub record_seq: u64,
    pub ts: u64,
    #[serde(with = "b64_array")]
    pub sig: [u8; 64],
}

impl ProbeResponse {
    pub fn sign(
        keys: &DeviceKeys,
        nonce: &[u8; 32],
        group_id: [u8; 16],
        group_version: u64,
        record_seq: u64,
        ts: u64,
    ) -> Self {
        let me = keys.public();
        let sig = keys.sign(
            DOMAIN_PROBE,
            &probe_fields(
                &me.device_id(),
                nonce,
                &group_id,
                group_version,
                record_seq,
                ts,
            )
            .each(),
        );
        Self {
            v: crate::wire::PROTOCOL_VERSION,
            sig_pk: me.sig_pk,
            kem_pk: me.kem_pk,
            group_id,
            group_version,
            record_seq,
            ts,
            sig,
        }
    }

    pub fn identity(&self) -> PublicIdentity {
        PublicIdentity {
            sig_pk: self.sig_pk,
            kem_pk: self.kem_pk,
        }
    }

    /// Checks the signature over our `nonce`. The caller then compares [`Self::identity`] with
    /// the expected member (or, while pairing, its fingerprint with the QR).
    pub fn verify(&self, nonce: &[u8; 32]) -> Result<()> {
        let id = self.identity();
        let f = probe_fields(
            &id.device_id(),
            nonce,
            &self.group_id,
            self.group_version,
            self.record_seq,
            self.ts,
        );
        id.verify(DOMAIN_PROBE, &f.each(), &self.sig)
    }
}

struct Fields(Vec<Vec<u8>>);

impl Fields {
    fn each(&self) -> Vec<&[u8]> {
        self.0.iter().map(Vec::as_slice).collect()
    }
}

fn probe_fields(
    id: &DeviceId,
    nonce: &[u8; 32],
    group_id: &[u8; 16],
    gv: u64,
    seq: u64,
    ts: u64,
) -> Fields {
    Fields(vec![
        id.to_vec(),
        nonce.to_vec(),
        group_id.to_vec(),
        gv.to_be_bytes().to_vec(),
        seq.to_be_bytes().to_vec(),
        ts.to_be_bytes().to_vec(),
    ])
}

/// What a signed request commits to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestTarget<'a> {
    pub server: DeviceId,
    pub method: &'a str,
    /// Path including query string, exactly as sent.
    pub path: &'a str,
    pub content_length: u64,
}

/// Parsed `Authorization: KD1 ...` header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestAuth {
    pub device: DeviceId,
    pub ts: u64,
    pub nonce: Vec<u8>,
    pub seq: u64,
    pub sig: [u8; 64],
}

impl RequestAuth {
    pub fn sign(
        keys: &DeviceKeys,
        target: &RequestTarget,
        ts: u64,
        nonce: &[u8],
        seq: u64,
    ) -> Self {
        let device = keys.device_id();
        let f = request_fields(&device, target, ts, nonce, seq);
        Self {
            device,
            ts,
            nonce: nonce.to_vec(),
            seq,
            sig: keys.sign(DOMAIN_REQUEST, &f.each()),
        }
    }

    /// Signature check only. Nonce freshness, `seq` monotonicity, timestamp window and
    /// membership are the listener's policy (protocol.md).
    pub fn verify(&self, client: &PublicIdentity, target: &RequestTarget) -> Result<()> {
        if client.device_id() != self.device {
            return Err(Error::BadSignature);
        }
        let f = request_fields(&self.device, target, self.ts, &self.nonce, self.seq);
        client.verify(DOMAIN_REQUEST, &f.each(), &self.sig)
    }

    pub fn to_header(&self) -> String {
        format!(
            "{SCHEME} device={},ts={},nonce={},seq={},sig={}",
            b64_encode(&self.device),
            self.ts,
            b64_encode(&self.nonce),
            self.seq,
            b64_encode(&self.sig)
        )
    }

    pub fn from_header(value: &str) -> Result<Self> {
        let bad = Error::Malformed("authorization header");
        let params = value
            .strip_prefix(SCHEME)
            .and_then(|s| s.strip_prefix(' '))
            .ok_or(Error::Malformed("authorization scheme"))?;
        let (mut device, mut ts, mut nonce, mut seq, mut sig) = (None, None, None, None, None);
        for kv in params.split(',') {
            let (k, v) = kv
                .trim()
                .split_once('=')
                .ok_or(Error::Malformed("authorization header"))?;
            match k {
                "device" => device = b64_decode_array(v),
                "ts" => ts = v.parse().ok(),
                "nonce" => nonce = b64_decode(v),
                "seq" => seq = v.parse().ok(),
                "sig" => sig = b64_decode_array(v),
                _ => return Err(bad),
            }
        }
        match (device, ts, nonce, seq, sig) {
            (Some(device), Some(ts), Some(nonce), Some(seq), Some(sig))
                if nonce.len() == 16 || nonce.len() == 32 =>
            {
                Ok(Self {
                    device,
                    ts,
                    nonce,
                    seq,
                    sig,
                })
            }
            _ => Err(bad),
        }
    }
}

fn request_fields(device: &DeviceId, t: &RequestTarget, ts: u64, nonce: &[u8], seq: u64) -> Fields {
    Fields(vec![
        t.server.to_vec(),
        device.to_vec(),
        t.method.as_bytes().to_vec(),
        t.path.as_bytes().to_vec(),
        t.content_length.to_be_bytes().to_vec(),
        ts.to_be_bytes().to_vec(),
        nonce.to_vec(),
        seq.to_be_bytes().to_vec(),
    ])
}

//! Signed objects: delivery receipts, the group membership list and per-device records
//! (protocol.md, "Signed objects").
//!
//! A signed object carries the exact body bytes it was signed over. Verifiers check the
//! signature on those bytes and only then parse them, so no canonical serialization is needed.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::wire::{b64_array, b64_vec};
use crate::{DeviceId, DeviceKeys, Error, PublicIdentity, Result, TransferId};

pub const DOMAIN_RECEIPT: &[u8] = b"kitedrop/v1/receipt";
pub const DOMAIN_GROUP: &[u8] = b"kitedrop/v1/group";
pub const DOMAIN_DEVICE_RECORD: &[u8] = b"kitedrop/v1/device-record";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedObject {
    #[serde(with = "b64_array")]
    pub signer: DeviceId,
    #[serde(with = "b64_vec")]
    pub body: Vec<u8>,
    #[serde(with = "b64_array")]
    pub sig: [u8; 64],
}

impl SignedObject {
    pub fn sign<T: Serialize>(keys: &DeviceKeys, domain: &[u8], body: &T) -> Result<Self> {
        let body = serde_json::to_vec(body)?;
        let sig = keys.sign(domain, &[&body]);
        Ok(Self {
            signer: keys.device_id(),
            body,
            sig,
        })
    }

    /// Verifies the signature with `signer`'s key and returns the parsed body.
    pub fn verify<T: DeserializeOwned>(&self, domain: &[u8], signer: &PublicIdentity) -> Result<T> {
        if signer.device_id() != self.signer {
            return Err(Error::BadSignature);
        }
        signer.verify(domain, &[&self.body], &self.sig)?;
        Ok(serde_json::from_slice(&self.body)?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptStatus {
    /// Content decrypted, verified and durably stored at its destination.
    Stored,
    /// The recipient refused the item for good (outbox pulls only; `code` says why). A direct
    /// upload is refused with an HTTP error instead.
    Rejected,
}

/// Signed by the recipient. The only thing that moves a send to `delivered`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub v: u8,
    #[serde(with = "b64_array")]
    pub transfer_id: TransferId,
    #[serde(with = "b64_array")]
    pub sender: DeviceId,
    #[serde(with = "b64_array")]
    pub recipient: DeviceId,
    pub status: ReceiptStatus,
    pub at: u64,
    /// Error code for `rejected` (protocol.md §14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
    Macos,
    Windows,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    #[serde(with = "b64_array")]
    pub sig_pk: [u8; 32],
    #[serde(with = "b64_array")]
    pub kem_pk: [u8; 32],
    pub name: String,
    pub platform: Platform,
    pub added_at: u64,
}

impl Member {
    pub fn identity(&self) -> PublicIdentity {
        PublicIdentity {
            sig_pk: self.sig_pk,
            kem_pk: self.kem_pk,
        }
    }
}

/// Versioned membership list. Version N+1 must be signed by a member of version N.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupList {
    pub v: u8,
    #[serde(with = "b64_array")]
    pub group_id: [u8; 16],
    pub version: u64,
    /// SHA-256 of the previous version's signed body; absent for version 1.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_b64_32")]
    pub prev: Option<[u8; 32]>,
    pub members: Vec<Member>,
    pub updated_at: u64,
}

impl GroupList {
    pub fn member(&self, id: &DeviceId) -> Option<&Member> {
        self.members
            .iter()
            .find(|m| &m.identity().device_id() == id)
    }
}

/// Self-signed, frequently changing reachability data of one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub v: u8,
    #[serde(with = "b64_array")]
    pub device_id: DeviceId,
    /// Monotonic per device; receivers keep the highest.
    pub seq: u64,
    /// `ip:port` strings, most recent first. Empty for devices that do not listen (iPhone).
    pub addresses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_token: Option<String>,
    pub updated_at: u64,
}

mod opt_b64_32 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&crate::wire::b64_encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let s = Option::<String>::deserialize(d)?;
        s.map(|s| crate::wire::b64_decode_array(&s).ok_or_else(|| D::Error::custom("bad prev")))
            .transpose()
    }
}

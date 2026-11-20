//! Encodings shared by every message: unambiguous field framing for signature and HPKE inputs,
//! and base64url (no padding) for binary fields inside JSON.

use sha2::{Digest, Sha256};

pub const PROTOCOL_VERSION: u8 = 1;
pub const DEFAULT_PORT: u16 = 52525;

/// Concatenates fields as `u32 BE length || bytes`. Two different field lists never produce the
/// same output, so signatures and HPKE `info`/`aad` built with it cannot be confused.
pub fn frame(fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(fields.iter().map(|f| f.len() + 4).sum());
    for f in fields {
        let len = u32::try_from(f.len()).expect("field shorter than 4 GiB");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(f);
    }
    out
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn b64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()
}

pub fn b64_decode_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    b64_decode(s)?.try_into().ok()
}

/// serde adapter: `[u8; N]` as base64url.
pub mod b64_array {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::b64_encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[u8; N], D::Error> {
        let s = String::deserialize(d)?;
        super::b64_decode_array(&s).ok_or_else(|| D::Error::custom("bad base64url field"))
    }
}

/// serde adapter: `Vec<u8>` as base64url.
pub mod b64_vec {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::b64_encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        super::b64_decode(&s).ok_or_else(|| D::Error::custom("bad base64url field"))
    }
}

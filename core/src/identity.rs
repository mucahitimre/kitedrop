//! Device identity: an Ed25519 signing key and an X25519 HPKE key, plus the fingerprint and
//! device id derived from both public keys (protocol.md, "Identity").

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hpke::{Deserializable, Kem as _, Serializable};
use rand_core::CryptoRng;
use zeroize::Zeroizing;

use crate::wire::{frame, sha256};
use crate::{Error, Result};

pub(crate) type Kem = hpke::kem::X25519HkdfSha256;
pub(crate) type Kdf = hpke::kdf::HkdfSha256;
pub(crate) type Aead = hpke::aead::ChaCha20Poly1305;
pub(crate) type KemPrivateKey = <Kem as hpke::Kem>::PrivateKey;
pub(crate) type KemPublicKey = <Kem as hpke::Kem>::PublicKey;

pub type DeviceId = [u8; 16];
pub type Fingerprint = [u8; 32];
pub type TransferId = [u8; 16];

/// Public half of a device identity. This is what group members store about each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicIdentity {
    pub sig_pk: [u8; 32],
    pub kem_pk: [u8; 32],
}

impl PublicIdentity {
    pub fn fingerprint(&self) -> Fingerprint {
        sha256(&frame(&[
            b"kitedrop/v1/fingerprint",
            &self.sig_pk,
            &self.kem_pk,
        ]))
    }

    pub fn device_id(&self) -> DeviceId {
        self.fingerprint()[..16].try_into().expect("16 of 32 bytes")
    }

    pub(crate) fn kem_public(&self) -> Result<KemPublicKey> {
        parse_kem_public(&self.kem_pk)
    }

    /// Verifies `sig` over `frame([domain, fields...])`.
    pub fn verify(&self, domain: &[u8], fields: &[&[u8]], sig: &[u8; 64]) -> Result<()> {
        let key = VerifyingKey::from_bytes(&self.sig_pk).map_err(|_| Error::BadSignature)?;
        let msg = signing_input(domain, fields);
        key.verify_strict(&msg, &Signature::from_bytes(sig))
            .map_err(|_| Error::BadSignature)
    }
}

/// Full device identity including private keys. Platform code persists the bytes from
/// [`DeviceKeys::secret_bytes`] in the OS secure store and never anywhere else.
pub struct DeviceKeys {
    sig: SigningKey,
    kem_sk: KemPrivateKey,
    kem_pk: KemPublicKey,
}

impl DeviceKeys {
    pub fn generate(rng: &mut impl CryptoRng) -> Self {
        let sig = SigningKey::generate(rng);
        let (kem_sk, kem_pk) = Kem::gen_keypair_with_rng(rng);
        Self {
            sig,
            kem_sk,
            kem_pk,
        }
    }

    pub fn from_secret_bytes(sig_sk: &[u8; 32], kem_sk: &[u8; 32]) -> Result<Self> {
        let sig = SigningKey::from_bytes(sig_sk);
        let kem_sk =
            KemPrivateKey::from_bytes(kem_sk).map_err(|_| Error::Malformed("kem private key"))?;
        let kem_pk = Kem::sk_to_pk(&kem_sk);
        Ok(Self {
            sig,
            kem_sk,
            kem_pk,
        })
    }

    /// `(ed25519 secret, x25519 secret)`.
    pub fn secret_bytes(&self) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
        let kem: [u8; 32] = self.kem_sk.to_bytes().into();
        (Zeroizing::new(self.sig.to_bytes()), Zeroizing::new(kem))
    }

    pub fn public(&self) -> PublicIdentity {
        PublicIdentity {
            sig_pk: self.sig.verifying_key().to_bytes(),
            kem_pk: self.kem_pk.to_bytes().into(),
        }
    }

    pub fn device_id(&self) -> DeviceId {
        self.public().device_id()
    }

    /// Signs `frame([domain, fields...])`.
    pub fn sign(&self, domain: &[u8], fields: &[&[u8]]) -> [u8; 64] {
        self.sig.sign(&signing_input(domain, fields)).to_bytes()
    }

    pub(crate) fn kem_keypair(&self) -> (KemPrivateKey, KemPublicKey) {
        (self.kem_sk.clone(), self.kem_pk.clone())
    }

    pub(crate) fn kem_private(&self) -> &KemPrivateKey {
        &self.kem_sk
    }
}

pub(crate) fn parse_kem_public(bytes: &[u8; 32]) -> Result<KemPublicKey> {
    KemPublicKey::from_bytes(bytes).map_err(|_| Error::Malformed("kem public key"))
}

fn signing_input(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut all: Vec<&[u8]> = Vec::with_capacity(fields.len() + 1);
    all.push(domain);
    all.extend_from_slice(fields);
    frame(&all)
}

//! kitedrop core: identity, envelope encryption, signed group state, pairing and request
//! authentication. The wire format is specified in `docs/protocol.md`; every public function
//! here maps to a section of that document.

pub mod auth;
pub mod envelope;
pub mod error;
pub mod group;
pub mod identity;
pub mod node;
pub mod pairing;
pub mod signed;
pub mod store;
pub mod wire;

pub use error::{Error, Result};
pub use identity::{DeviceId, DeviceKeys, Fingerprint, PublicIdentity, TransferId};
pub use node::{
    Config, Content, DeviceInfo, Event, Host, NetworkIssue, Node, PreparedUpload, Progress,
    ProgressPhase,
};

/// Operating-system CSPRNG, for callers that do not need deterministic output.
pub fn os_rng() -> impl rand_core::CryptoRng {
    rand_core::UnwrapErr(getrandom::SysRng)
}

/// `N` bytes from the operating-system CSPRNG (nonces, transfer ids, pairing codes).
pub fn random<const N: usize>() -> [u8; N] {
    use rand_core::Rng;
    let mut out = [0u8; N];
    os_rng().fill_bytes(&mut out);
    out
}

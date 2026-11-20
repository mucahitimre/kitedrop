//! Envelope: the single format for every transfer (protocol.md, "Envelope").
//!
//! An envelope is an HPKE (RFC 9180) mode_auth context from the sender's X25519 key to the
//! recipient's X25519 key, followed by a sequence of records sealed with that context. Record 0
//! is the encrypted content header; records 1.. carry the content in fixed-size chunks, the last
//! one typed `FINAL_CHUNK`. HPKE's own sequence counter provides the per-record nonce, and the
//! AAD binds transfer id, record index and record type, so truncation, reordering and
//! final-flag tampering all fail authentication.

use std::io::{ErrorKind, Read, Write};

use hpke::{Deserializable, OpModeR, OpModeS, Serializable};
use rand_core::CryptoRng;
use serde::{Deserialize, Serialize};

use crate::identity::{Aead, Kdf, Kem};
use crate::wire::{PROTOCOL_VERSION, frame};
use crate::{DeviceId, DeviceKeys, Error, PublicIdentity, Result, TransferId};

pub const MAGIC: [u8; 4] = *b"KTDP";
/// DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305, mode_auth.
pub const SUITE_V1: u8 = 1;
/// magic(4) version(1) suite(1) reserved(2) sender(16) recipient(16) transfer_id(16)
pub const PREFIX_LEN: usize = 56;
pub const ENC_LEN: usize = 32;
pub const TAG_LEN: usize = 16;
pub const DEFAULT_CHUNK_SIZE: u32 = 64 * 1024;
pub const MIN_CHUNK_SIZE: u32 = 64;
pub const MAX_CHUNK_SIZE: u32 = 1024 * 1024;
pub const MAX_HEADER_LEN: usize = 16 * 1024;
pub const MAX_NAME_LEN: usize = 255;

pub const RECORD_HEADER: u8 = 0x01;
pub const RECORD_CHUNK: u8 = 0x02;
pub const RECORD_FINAL_CHUNK: u8 = 0x03;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    File,
    Text,
    Link,
}

/// Encrypted metadata, record 0 of every envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentHeader {
    pub v: u8,
    pub kind: ContentKind,
    /// File name without any path component; empty for text and links.
    pub name: String,
    pub mime: String,
    /// Exact plaintext content length in bytes.
    pub size: u64,
    /// Sender clock, Unix seconds. Receivers reject envelopes outside the replay window.
    pub created_at: u64,
    pub chunk_size: u32,
}

impl ContentHeader {
    fn validate(&self) -> Result<()> {
        if self.v != PROTOCOL_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let cs = self.chunk_size;
        if !(MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE).contains(&cs) || !cs.is_power_of_two() {
            return Err(Error::Malformed("chunk_size"));
        }
        if self.name.len() > MAX_NAME_LEN
            || self.name.contains(['/', '\\', '\0'])
            || self.name == "."
            || self.name == ".."
        {
            return Err(Error::Malformed("name"));
        }
        if (self.kind == ContentKind::File) == self.name.is_empty() {
            return Err(Error::Malformed("name"));
        }
        Ok(())
    }
}

/// Clear-text routing fields of an envelope. Authenticated (bound into HPKE `info`), not secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeInfo {
    pub sender: DeviceId,
    pub recipient: DeviceId,
    pub transfer_id: TransferId,
}

impl EnvelopeInfo {
    pub fn to_prefix(&self) -> [u8; PREFIX_LEN] {
        let mut p = [0u8; PREFIX_LEN];
        p[..4].copy_from_slice(&MAGIC);
        p[4] = PROTOCOL_VERSION;
        p[5] = SUITE_V1;
        p[8..24].copy_from_slice(&self.sender);
        p[24..40].copy_from_slice(&self.recipient);
        p[40..56].copy_from_slice(&self.transfer_id);
        p
    }

    pub fn from_prefix(p: &[u8; PREFIX_LEN]) -> Result<Self> {
        if p[..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        if p[4] != PROTOCOL_VERSION || p[5] != SUITE_V1 || p[6..8] != [0, 0] {
            return Err(Error::UnsupportedVersion);
        }
        let id = |r: std::ops::Range<usize>| p[r].try_into().expect("16 bytes");
        Ok(Self {
            sender: id(8..24),
            recipient: id(24..40),
            transfer_id: id(40..56),
        })
    }
}

fn hpke_info(prefix: &[u8; PREFIX_LEN]) -> Vec<u8> {
    frame(&[b"kitedrop/v1/envelope", prefix])
}

fn record_aad(transfer_id: &TransferId, index: u64, record_type: u8) -> Vec<u8> {
    frame(&[
        b"kitedrop/v1/record",
        transfer_id,
        &index.to_be_bytes(),
        &[record_type],
    ])
}

fn write_record(out: &mut impl Write, record_type: u8, ct: &[u8]) -> Result<()> {
    let len = u32::try_from(ct.len()).map_err(|_| Error::RecordTooLarge)?;
    out.write_all(&[record_type])?;
    out.write_all(&len.to_be_bytes())?;
    out.write_all(ct)?;
    Ok(())
}

/// Reads until `buf` is full or EOF; returns bytes read.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(n)
}

/// Encrypts `content` into an envelope written to `out`, streaming one chunk at a time.
/// Fails with [`Error::SizeMismatch`] if `content` does not yield exactly `header.size` bytes.
/// Returns the number of envelope bytes written.
pub fn seal(
    sender: &DeviceKeys,
    recipient: &PublicIdentity,
    transfer_id: TransferId,
    header: &ContentHeader,
    mut content: impl Read,
    mut out: impl Write,
    rng: &mut impl CryptoRng,
) -> Result<u64> {
    header.validate()?;
    let info = EnvelopeInfo {
        sender: sender.device_id(),
        recipient: recipient.device_id(),
        transfer_id,
    };
    let prefix = info.to_prefix();
    let mode = OpModeS::<Kem>::Auth(sender.kem_keypair());
    let (enc, mut ctx) = hpke::setup_sender_with_rng::<Aead, Kdf, Kem>(
        &mode,
        &recipient.kem_public()?,
        &hpke_info(&prefix),
        rng,
    )
    .map_err(|_| Error::Malformed("recipient key"))?;

    let mut written = (PREFIX_LEN + ENC_LEN) as u64;
    out.write_all(&prefix)?;
    out.write_all(&enc.to_bytes())?;

    let header_json = serde_json::to_vec(header)?;
    if header_json.len() > MAX_HEADER_LEN {
        return Err(Error::RecordTooLarge);
    }
    let ct = ctx
        .seal(&header_json, &record_aad(&transfer_id, 0, RECORD_HEADER))
        .map_err(|_| Error::DecryptFailed)?;
    write_record(&mut out, RECORD_HEADER, &ct)?;
    written += 5 + ct.len() as u64;

    // Look-ahead by one chunk so the last chunk can be typed FINAL_CHUNK.
    let cs = header.chunk_size as usize;
    let mut cur = vec![0u8; cs];
    let mut next = vec![0u8; cs];
    let mut cur_len = read_full(&mut content, &mut cur)?;
    let mut total = cur_len as u64;
    let mut index = 1u64;
    loop {
        let next_len = if cur_len == cs {
            read_full(&mut content, &mut next)?
        } else {
            0
        };
        let record_type = if next_len == 0 {
            RECORD_FINAL_CHUNK
        } else {
            RECORD_CHUNK
        };
        let ct = ctx
            .seal(
                &cur[..cur_len],
                &record_aad(&transfer_id, index, record_type),
            )
            .map_err(|_| Error::DecryptFailed)?;
        write_record(&mut out, record_type, &ct)?;
        written += 5 + ct.len() as u64;
        if record_type == RECORD_FINAL_CHUNK {
            break;
        }
        std::mem::swap(&mut cur, &mut next);
        cur_len = next_len;
        total += next_len as u64;
        index += 1;
    }
    if total != header.size {
        return Err(Error::SizeMismatch);
    }
    out.flush()?;
    Ok(written)
}

/// Reads one record. Returns `None` on a clean EOF before the record starts.
fn read_record(input: &mut impl Read, max_ct: usize) -> Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 5];
    match read_full(input, &mut head)? {
        0 => return Ok(None),
        5 => {}
        _ => return Err(Error::Truncated),
    }
    let len = u32::from_be_bytes(head[1..5].try_into().expect("4 bytes")) as usize;
    if len > max_ct {
        return Err(Error::RecordTooLarge);
    }
    let mut ct = vec![0u8; len];
    if read_full(input, &mut ct)? != len {
        return Err(Error::Truncated);
    }
    Ok(Some((head[0], ct)))
}

/// Decrypts an envelope from `input`, streaming plaintext into `out`.
///
/// `resolve_sender` sees the authenticated-later routing fields first; it must return the
/// sender's public identity (from the group list) or reject the envelope (unknown sender,
/// replayed transfer id, ...). Plaintext written to `out` is untrusted until this returns `Ok`:
/// write to a temporary file and move it into place only on success.
pub fn open(
    recipient: &DeviceKeys,
    mut input: impl Read,
    mut out: impl Write,
    resolve_sender: impl FnOnce(&EnvelopeInfo) -> Result<PublicIdentity>,
) -> Result<(EnvelopeInfo, ContentHeader)> {
    let mut prefix = [0u8; PREFIX_LEN];
    if read_full(&mut input, &mut prefix)? != PREFIX_LEN {
        return Err(Error::Truncated);
    }
    let info = EnvelopeInfo::from_prefix(&prefix)?;
    if info.recipient != recipient.device_id() {
        return Err(Error::WrongRecipient);
    }
    let sender = resolve_sender(&info)?;
    if sender.device_id() != info.sender {
        return Err(Error::UnknownSender);
    }

    let mut enc_bytes = [0u8; ENC_LEN];
    if read_full(&mut input, &mut enc_bytes)? != ENC_LEN {
        return Err(Error::Truncated);
    }
    let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(&enc_bytes)
        .map_err(|_| Error::DecryptFailed)?;
    let mode = OpModeR::<Kem>::Auth(sender.kem_public()?);
    let mut ctx = hpke::setup_receiver::<Aead, Kdf, Kem>(
        &mode,
        recipient.kem_private(),
        &enc,
        &hpke_info(&prefix),
    )
    .map_err(|_| Error::DecryptFailed)?;

    let tid = info.transfer_id;
    let (rt, ct) = read_record(&mut input, MAX_HEADER_LEN + TAG_LEN)?.ok_or(Error::Truncated)?;
    if rt != RECORD_HEADER {
        return Err(Error::Malformed("first record must be the header"));
    }
    let pt = ctx
        .open(&ct, &record_aad(&tid, 0, RECORD_HEADER))
        .map_err(|_| Error::DecryptFailed)?;
    let header: ContentHeader = serde_json::from_slice(&pt)?;
    header.validate()?;

    let max_ct = header.chunk_size as usize + TAG_LEN;
    let mut total = 0u64;
    let mut index = 1u64;
    loop {
        let (rt, ct) = read_record(&mut input, max_ct)?.ok_or(Error::Truncated)?;
        match rt {
            RECORD_CHUNK if ct.len() != max_ct => return Err(Error::Malformed("short chunk")),
            RECORD_CHUNK | RECORD_FINAL_CHUNK => {}
            _ => return Err(Error::Malformed("record type")),
        }
        let pt = ctx
            .open(&ct, &record_aad(&tid, index, rt))
            .map_err(|_| Error::DecryptFailed)?;
        total += pt.len() as u64;
        if total > header.size {
            return Err(Error::SizeMismatch);
        }
        out.write_all(&pt)?;
        if rt == RECORD_FINAL_CHUNK {
            break;
        }
        index += 1;
    }
    if total != header.size {
        return Err(Error::SizeMismatch);
    }
    let mut extra = [0u8; 1];
    if read_full(&mut input, &mut extra)? != 0 {
        return Err(Error::TrailingData);
    }
    out.flush()?;
    Ok((info, header))
}

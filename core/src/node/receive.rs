//! Receiving envelopes, by upload (`POST /v1/inbox`) or by pulling outboxes (iPhone), following
//! the receiver rules of protocol.md §5.4: replay check, streaming decryption into a temporary
//! file, replay window, durable store, then the signed receipt.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use serde::Deserialize;

use crate::envelope::{self, ContentHeader, ContentKind};
use crate::signed::{DOMAIN_RECEIPT, Platform, Receipt, ReceiptStatus, SignedObject};
use crate::store::Received;
use crate::wire::{PROTOCOL_VERSION, b64_array};
use crate::{DeviceId, Error, Result, TransferId};

use super::client::{self, Session};
use super::{Event, Inner, MAX_TEXT_LEN, discovery, groupsync, lock, now, read};

/// Replay window for `created_at` (protocol.md §5.4 step 4).
const MAX_AGE_SECS: u64 = 30 * 24 * 3600;
const MAX_FUTURE_SECS: u64 = 24 * 3600;
/// Seen-cache retention; longer than the replay window.
pub(crate) const SEEN_RETENTION_SECS: u64 = 31 * 24 * 3600;
const BODY_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

struct InflightGuard<'a> {
    inner: &'a Inner,
    key: (DeviceId, TransferId),
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        lock(&self.inner.inflight).remove(&self.key);
    }
}

/// Counts bytes read, so the body length can be checked against the signed length.
struct Counting<R> {
    inner: R,
    n: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n += k as u64;
        Ok(k)
    }
}

fn partial_dir(inner: &Inner) -> PathBuf {
    inner.cfg.download_dir.join(".kitedrop-partial")
}

/// Handles an upload body. Idempotent: a replayed `(sender, transfer_id)` gets the stored receipt.
pub(crate) async fn accept_upload<S, E>(
    inner: &Arc<Inner>,
    sender: DeviceId,
    tid: TransferId,
    len: u64,
    body: S,
) -> Result<SignedObject>
where
    S: Stream<Item = std::result::Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    receive_stream(inner, sender, tid, len, body).await
}

async fn receive_stream<S, E>(
    inner: &Arc<Inner>,
    sender: DeviceId,
    tid: TransferId,
    len: u64,
    body: S,
) -> Result<SignedObject>
where
    S: Stream<Item = std::result::Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let key = (sender, tid);
    if !lock(&inner.inflight).insert(key) {
        return Err(Error::RateLimited);
    }
    let _guard = InflightGuard { inner, key };

    // A stalled peer (lost Wi-Fi, sleeping laptop, or someone trickling a captured request) must
    // not hold the transfer forever: give up after BODY_IDLE without data.
    let mut body = Box::pin(futures_util::stream::unfold(body, |mut s| async move {
        match tokio::time::timeout(BODY_IDLE, s.next()).await {
            Ok(Some(item)) => Some((item.map_err(std::io::Error::other), s)),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "no data for 60 s",
                )),
                s,
            )),
        }
    }));
    if let Some(receipt) = inner.store.seen(&sender, &tid)?.flatten() {
        // Replay or retry after a lost response: drain, answer with the stored receipt.
        while body.try_next().await?.is_some() {}
        return Ok(receipt);
    }

    let dir = partial_dir(inner);
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = dir.join(format!("{}-{}.part", hex(&sender), hex(&tid)));
    let reader = tokio_util::io::SyncIoBridge::new(tokio_util::io::StreamReader::new(body));
    let worker = inner.clone();
    let tmp2 = tmp.clone();
    let opened = tokio::task::spawn_blocking(move || -> Result<ContentHeader> {
        let mut input = Counting {
            inner: reader,
            n: 0,
        };
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp2)?);
        let (_, header) = envelope::open(&worker.keys, &mut input, &mut out, |info| {
            if info.sender != sender || info.transfer_id != tid {
                return Err(Error::BadSignature);
            }
            worker.member_identity(&sender).ok_or(Error::UnknownSender)
        })?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        if input.n != len {
            return Err(Error::SizeMismatch);
        }
        Ok(header)
    })
    .await
    .map_err(|e| Error::Internal(format!("receive task: {e}")))?;
    let result = match opened {
        Ok(header) => store_received(inner, sender, tid, header, &tmp).await,
        Err(e) => Err(e),
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Steps 4–5 of the receiver rules for a decrypted, verified temporary file.
async fn store_received(
    inner: &Arc<Inner>,
    sender: DeviceId,
    tid: TransferId,
    header: ContentHeader,
    tmp: &Path,
) -> Result<SignedObject> {
    let t = now();
    if header.created_at.saturating_add(MAX_AGE_SECS) < t || header.created_at > t + MAX_FUTURE_SECS
    {
        return Err(Error::Expired);
    }
    let from = inner.device_info(&sender).ok_or(Error::UnknownSender)?;
    let worker = inner.clone();
    let tmp = tmp.to_owned();
    let h = header.clone();
    let location = tokio::task::spawn_blocking(move || -> Result<Option<PathBuf>> {
        match h.kind {
            ContentKind::File => {
                let dest = reserve_name(&worker.cfg.download_dir, &h.name)?;
                if let Err(e) = std::fs::rename(&tmp, &dest) {
                    let _ = std::fs::remove_file(&dest);
                    return Err(e.into());
                }
                match worker.host.deliver_file(&dest, &h, &from) {
                    Ok(p) => Ok(Some(p)),
                    Err(msg) => {
                        let _ = std::fs::remove_file(&dest);
                        Err(Error::Internal(format!(
                            "host could not store the file: {msg}"
                        )))
                    }
                }
            }
            ContentKind::Text | ContentKind::Link => {
                if h.size > MAX_TEXT_LEN {
                    return Err(Error::RecordTooLarge);
                }
                let text = String::from_utf8(std::fs::read(&tmp)?)
                    .map_err(|_| Error::Malformed("text is not UTF-8"))?;
                let _ = std::fs::remove_file(&tmp);
                worker
                    .host
                    .deliver_text(h.kind, &text, &from)
                    .map_err(|msg| {
                        Error::Internal(format!("host could not store the text: {msg}"))
                    })?;
                Ok(None)
            }
        }
    })
    .await
    .map_err(|e| Error::Internal(format!("store task: {e}")))??;

    let receipt = Receipt {
        v: PROTOCOL_VERSION,
        transfer_id: tid,
        sender,
        recipient: inner.me(),
        status: ReceiptStatus::Stored,
        at: now(),
        code: None,
    };
    let signed = SignedObject::sign(&inner.keys, DOMAIN_RECEIPT, &receipt)?;
    let item = Received {
        sender,
        transfer_id: tid,
        kind: header.kind,
        name: header.name,
        size: header.size,
        location,
        at: receipt.at,
    };
    inner.store.mark_received(&item, &signed)?;
    inner.emit(Event::Received(item));
    Ok(signed)
}

/// Makes a file name safe on every target file system.
pub(crate) fn sanitize_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let s = s.trim_start().to_owned();
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    match () {
        _ if s.is_empty() || s == "." || s == ".." => "file".to_owned(),
        _ if reserved => format!("_{s}"),
        _ => s,
    }
}

/// Creates an empty file with a free name (`name`, `name (2)`, …) so concurrent receives never
/// overwrite each other or existing files.
pub(crate) fn reserve_name(dir: &Path, name: &str) -> Result<PathBuf> {
    let name = sanitize_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 1..10_000u32 {
        let candidate = if n == 1 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut f) => {
                f.flush()?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Internal("no free file name".into()))
}

#[derive(Deserialize)]
struct OutboxList {
    items: Vec<OutboxEntry>,
}

#[derive(Deserialize)]
struct OutboxEntry {
    #[serde(with = "b64_array")]
    transfer_id: TransferId,
}

/// iPhone side of protocol.md §9: pull from every reachable listening member.
pub(crate) async fn poll_all(inner: &Arc<Inner>) -> Result<u32> {
    let members: Vec<(DeviceId, Platform)> = read(&inner.group)
        .current()
        .members
        .iter()
        .map(|m| (m.identity().device_id(), m.platform))
        .filter(|(id, p)| *id != inner.me() && *p != Platform::Ios)
        .collect();
    let mut stored = 0;
    for (id, _) in members {
        match poll_one(inner, &id).await {
            Ok(n) => stored += n,
            Err(e) => tracing::info!("outbox poll of a member failed: {e}"),
        }
    }
    let _ = inner
        .store
        .purge_seen(now().saturating_sub(SEEN_RETENTION_SECS));
    Ok(stored)
}

async fn poll_one(inner: &Arc<Inner>, id: &DeviceId) -> Result<u32> {
    let Some((addr, probe)) = discovery::probe_one(inner, id).await else {
        return Ok(0);
    };
    let mut session = Session::open(&inner.http, &inner.keys, &addr, *id).await?;
    let (gv, seq) = {
        let g = read(&inner.group);
        (g.version(), g.record_seq(id))
    };
    if probe.group_version != gv || probe.record_seq > seq {
        groupsync::pull_with(inner, &mut session).await?;
    }
    if !read(&inner.group).is_member(id) {
        return Ok(0);
    }
    let list: OutboxList = session.get_json("/v1/outbox").await?;
    let mut stored = 0;
    for item in list.items {
        let tid = item.transfer_id;
        // One bad item must not hold back the others (design §3).
        let receipt = match inner.store.seen(id, &tid)?.flatten() {
            Some(r) => r,
            None => match pull_item(inner, &mut session, id, &tid).await {
                Ok(r) => {
                    stored += 1;
                    r
                }
                Err(e) if e.is_transient() || e.code() == "truncated" => {
                    tracing::info!("outbox item not pulled, retrying next time: {e}");
                    continue;
                }
                // Refused for good: tell the sender instead of leaving it waiting until TTL.
                Err(e) => rejection(inner, id, &tid, &e)?,
            },
        };
        let posted: Result<serde_json::Value> = session.post_json("/v1/receipts", &receipt).await;
        if let Err(e) = posted {
            if e.code() != "challenge_invalid" {
                return Err(e);
            }
            // The challenge expired during a long download: open a new session and resend.
            session = Session::open(&inner.http, &inner.keys, &addr, *id).await?;
            let _: serde_json::Value = session.post_json("/v1/receipts", &receipt).await?;
        }
    }
    Ok(stored)
}

async fn pull_item<'a>(
    inner: &'a Arc<Inner>,
    session: &mut Session<'a>,
    id: &DeviceId,
    tid: &TransferId,
) -> Result<SignedObject> {
    let path = client::outbox_item_path(tid);
    let resp = match session.get_stream(&path).await {
        Err(e) if e.code() == "challenge_invalid" => {
            *session = Session::open(&inner.http, &inner.keys, &session.addr.clone(), *id).await?;
            session.get_stream(&path).await?
        }
        r => r?,
    };
    let len = resp
        .content_length()
        .ok_or(Error::Malformed("content-length required"))?;
    receive_stream(inner, *id, *tid, len, Box::pin(resp.bytes_stream())).await
}

fn rejection(
    inner: &Inner,
    sender: &DeviceId,
    tid: &TransferId,
    e: &Error,
) -> Result<SignedObject> {
    let receipt = Receipt {
        v: PROTOCOL_VERSION,
        transfer_id: *tid,
        sender: *sender,
        recipient: inner.me(),
        status: ReceiptStatus::Rejected,
        at: now(),
        code: Some(e.code().to_owned()),
    };
    SignedObject::sign(&inner.keys, DOMAIN_RECEIPT, &receipt)
}

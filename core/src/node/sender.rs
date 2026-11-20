//! Send queue and outbox (protocol.md §9, §11): every send is sealed once into a spool file,
//! persisted, then delivered by upload with retry and backoff, or parked in the outbox for an
//! iPhone. A send is `delivered` only after the recipient's signed receipt.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::envelope::{self, ContentHeader, ContentKind, DEFAULT_CHUNK_SIZE};
use crate::signed::{DOMAIN_RECEIPT, Platform, Receipt, ReceiptStatus, SignedObject};
use crate::store::{Transfer, TransferState};
use crate::wire::PROTOCOL_VERSION;
use crate::{DeviceId, Error, Result, TransferId};

use super::client::{self, Session};
use super::{
    Content, Event, Inner, MAX_TEXT_LEN, PreparedUpload, Progress, ProgressPhase, discovery,
    groupsync, lock, now, read,
};

const EXPO_PUSH_URL: &str = "https://exp.host/--/api/v2/push/send";
const PUSH_COALESCE_SECS: u64 = 30;
const MAX_PARALLEL: usize = 4;
/// Smaller sends show no progress: they finish before a bar would be useful.
const PROGRESS_MIN_BYTES: u64 = 1024 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Counts the bytes of one phase of a large send and reports them (throttled) as
/// [`Event::TransferProgress`]; the latest value is kept for [`super::Node::progress`].
pub(crate) struct Reporter {
    inner: Arc<Inner>,
    tid: TransferId,
    progress: Progress,
    last: Option<Instant>,
}

impl Reporter {
    pub(crate) fn new(
        inner: &Arc<Inner>,
        tid: TransferId,
        phase: ProgressPhase,
        total: u64,
    ) -> Option<Self> {
        (total >= PROGRESS_MIN_BYTES).then(|| Self {
            inner: inner.clone(),
            tid,
            progress: Progress {
                phase,
                done: 0,
                total,
            },
            last: None,
        })
    }

    pub(crate) fn add(&mut self, n: u64) {
        self.progress.done = (self.progress.done + n).min(self.progress.total);
        let t = Instant::now();
        let due = self
            .last
            .is_none_or(|l| t.duration_since(l) >= PROGRESS_INTERVAL);
        if due || self.progress.done == self.progress.total {
            self.last = Some(t);
            lock(&self.inner.progress).insert(self.tid, self.progress);
            self.inner.emit(Event::TransferProgress {
                transfer_id: self.tid,
                progress: self.progress,
            });
        }
    }
}

fn clear_progress(inner: &Inner, tid: &TransferId) {
    lock(&inner.progress).remove(tid);
}

/// Reader that reports how much of the source has been read (sealing progress).
struct Counting<R> {
    inner: R,
    reporter: Option<Reporter>,
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(r) = &mut self.reporter {
            r.add(n as u64);
        }
        Ok(n)
    }
}

/// Removes the `sending` mark of a transfer when its attempt ends.
struct SendingGuard {
    inner: Arc<Inner>,
    tid: TransferId,
}

impl Drop for SendingGuard {
    fn drop(&mut self) {
        lock(&self.inner.sending).remove(&self.tid);
        self.inner.kick.notify_one();
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Small extension → MIME table for the common cases; everything else is a byte stream.
pub(crate) fn guess_mime(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("heic") => "image/heic",
        Some("webp") => "image/webp",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("zip") => "application/zip",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

struct Source {
    kind: ContentKind,
    name: String,
    mime: String,
    size: u64,
    file: Option<PathBuf>,
    bytes: Vec<u8>,
}

fn resolve_content(content: Content) -> Result<Source> {
    Ok(match content {
        Content::File { path, name, mime } => {
            let meta = std::fs::metadata(&path)?;
            if !meta.is_file() {
                return Err(Error::Malformed("not a file"));
            }
            let name = name.unwrap_or_else(|| {
                path.file_name()
                    .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned())
            });
            let name = super::receive::sanitize_name(&name);
            Source {
                kind: ContentKind::File,
                mime: mime.unwrap_or_else(|| guess_mime(&name).to_owned()),
                name,
                size: meta.len(),
                file: Some(path),
                bytes: Vec::new(),
            }
        }
        Content::Text(t) | Content::Link(t) if t.len() as u64 > MAX_TEXT_LEN => {
            return Err(Error::RecordTooLarge);
        }
        Content::Text(t) => Source {
            kind: ContentKind::Text,
            name: String::new(),
            mime: "text/plain; charset=utf-8".into(),
            size: t.len() as u64,
            file: None,
            bytes: t.into_bytes(),
        },
        Content::Link(t) => Source {
            kind: ContentKind::Link,
            name: String::new(),
            mime: "text/uri-list".into(),
            size: t.len() as u64,
            file: None,
            bytes: t.into_bytes(),
        },
    })
}

/// Writes `t` unless the stored row is already final; returns whether it was written.
fn persist(inner: &Inner, t: &mut Transfer) -> Result<bool> {
    t.updated_at = now();
    let written = inner.store.update_transfer(t)?;
    if written {
        inner.emit(Event::TransferChanged(t.clone()));
    }
    Ok(written)
}

pub(crate) async fn enqueue(
    inner: &Arc<Inner>,
    targets: Vec<DeviceId>,
    content: Content,
) -> Result<Vec<TransferId>> {
    if targets.is_empty() {
        return Err(Error::Malformed("no target"));
    }
    let mut platforms = Vec::with_capacity(targets.len());
    {
        let g = read(&inner.group);
        for id in &targets {
            if *id == inner.me() {
                return Err(Error::Malformed("cannot send to self"));
            }
            platforms.push(g.member(id).ok_or(Error::NotMember)?.platform);
        }
    }
    let src = Arc::new(resolve_content(content)?);
    let mut ids = Vec::new();
    let mut rows = Vec::new();
    for (target, platform) in targets.into_iter().zip(platforms) {
        let tid = crate::random::<16>();
        let t = now();
        let row = Transfer {
            transfer_id: tid,
            target,
            kind: src.kind,
            name: src.name.clone(),
            mime: src.mime.clone(),
            size: src.size,
            envelope: inner.spool().join(format!("{}.ktdp", hex(&tid))),
            envelope_len: 0,
            state: TransferState::Queued,
            reason: None,
            via_outbox: platform == Platform::Ios,
            attempts: 0,
            next_attempt_at: t,
            created_at: t,
            updated_at: t,
            expires_at: t + inner.cfg.ttl_secs,
        };
        inner.store.insert_transfer(&row)?;
        inner.emit(Event::TransferChanged(row.clone()));
        ids.push(tid);
        rows.push(row);
    }
    // Seal in the background; the sends are already visible as `queued`.
    let worker = inner.clone();
    tokio::spawn(async move {
        for mut row in rows {
            seal_one(&worker, &src, &mut row).await;
        }
        worker.kick.notify_one();
    });
    Ok(ids)
}

async fn seal_one(inner: &Arc<Inner>, src: &Arc<Source>, row: &mut Transfer) {
    let Some(recipient) = inner.member_identity(&row.target) else {
        fail(inner, row, "removed_from_group");
        return;
    };
    let keys_owner = inner.clone();
    let src2 = src.clone();
    let path = row.envelope.clone();
    let tid = row.transfer_id;
    // Err((reason, error)): the reason distinguishes an unreadable source from other failures.
    let sealed =
        tokio::task::spawn_blocking(move || -> std::result::Result<u64, (String, Error)> {
            let unreadable = |e: std::io::Error| ("file_unreadable".to_owned(), Error::from(e));
            let other = |e: Error| (e.code().to_owned(), e);
            let source: Box<dyn std::io::Read> = match &src2.file {
                Some(p) => Box::new(std::io::BufReader::new(
                    std::fs::File::open(p).map_err(unreadable)?,
                )),
                None => Box::new(std::io::Cursor::new(src2.bytes.clone())),
            };
            let input = Counting {
                inner: source,
                reporter: Reporter::new(&keys_owner, tid, ProgressPhase::Sealing, src2.size),
            };
            let header = ContentHeader {
                v: PROTOCOL_VERSION,
                kind: src2.kind,
                name: src2.name.clone(),
                mime: src2.mime.clone(),
                size: src2.size,
                created_at: now(),
                chunk_size: DEFAULT_CHUNK_SIZE,
            };
            let file = std::fs::File::create(&path).map_err(|e| other(e.into()))?;
            let mut out = std::io::BufWriter::new(file);
            let n = envelope::seal(
                &keys_owner.keys,
                &recipient,
                tid,
                &header,
                input,
                &mut out,
                &mut crate::os_rng(),
            )
            .map_err(|e| match e {
                // The source changed size or failed while being read.
                Error::SizeMismatch => ("file_unreadable".to_owned(), e),
                e => other(e),
            })?;
            // Sync through the handle we wrote with: on Windows, FlushFileBuffers on a read-only
            // handle fails with "access denied" (seen 2026-11-19: every send from Windows failed).
            out.into_inner()
                .map_err(|e| other(e.into_error().into()))?
                .sync_all()
                .map_err(|e| other(e.into()))?;
            Ok(n)
        })
        .await
        .unwrap_or_else(|e| Err(("internal".to_owned(), Error::Internal(e.to_string()))));
    clear_progress(inner, &row.transfer_id);
    match sealed {
        Ok(len) => {
            row.envelope_len = len;
            if row.via_outbox {
                row.state = TransferState::Waiting;
                let _ = persist(inner, row);
                notify_push(inner, &row.target);
                update_outbox_count(inner);
            } else {
                let _ = persist(inner, row);
            }
        }
        Err((reason, e)) => {
            tracing::warn!("sealing failed ({reason}): {e}");
            fail(inner, row, &reason);
        }
    }
}

fn fail(inner: &Inner, t: &mut Transfer, reason: &str) {
    t.state = TransferState::Failed;
    t.reason = Some(reason.to_owned());
    if !persist(inner, t).unwrap_or(false) {
        return; // already final
    }
    let _ = std::fs::remove_file(&t.envelope);
    if t.via_outbox {
        update_outbox_count(inner);
    }
}

fn update_outbox_count(inner: &Inner) {
    let count = inner
        .store
        .open_transfers()
        .map(|v| v.iter().filter(|t| t.via_outbox).count() as u32)
        .unwrap_or(0);
    if inner.outbox_count.swap(count, Ordering::Relaxed) != count {
        inner.emit(Event::OutboxPending { count });
    }
}

/// Content-free push to the iPhone (protocol.md §10), at most one per target per 30 s.
pub(crate) fn notify_push(inner: &Arc<Inner>, target: &DeviceId) {
    let Some(token) = read(&inner.group)
        .record(target)
        .and_then(|r| r.push_token.clone())
    else {
        return;
    };
    let t = now();
    {
        let mut last = lock(&inner.last_push);
        if let Some(&p) = last.get(target)
            && t.saturating_sub(p) < PUSH_COALESCE_SECS
        {
            // Coalesce: one push at the end of the window, if items are still waiting then.
            if lock(&inner.push_scheduled).insert(*target) {
                let (worker, id) = (inner.clone(), *target);
                let delay = PUSH_COALESCE_SECS - t.saturating_sub(p);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    lock(&worker.push_scheduled).remove(&id);
                    if worker.store.outbox_for(&id).is_ok_and(|v| !v.is_empty()) {
                        notify_push(&worker, &id);
                    }
                });
            }
            return;
        }
        last.insert(*target, t);
    }
    let body = serde_json::json!({
        "to": token,
        "title": "kitedrop",
        "body": format!("New item from {}", inner.cfg.device_name),
        "sound": "default",
    })
    .to_string();
    let host = inner.host.clone();
    tokio::task::spawn_blocking(move || match host.https_post(EXPO_PUSH_URL, &body) {
        Ok(status) if (200..300).contains(&status) => {}
        Ok(status) => tracing::info!("push answered HTTP {status}"),
        Err(e) => tracing::info!("push not sent: {e}"),
    });
}

fn backoff(inner: &Inner, attempts: u32) -> u64 {
    let s = &inner.cfg.retry_schedule_secs;
    let base = s
        .get(attempts.saturating_sub(1) as usize)
        .or(s.last())
        .copied()
        .unwrap_or(300);
    // ±20 % jitter
    let r = u64::from(crate::random::<2>()[0]) % 41; // 0..=40
    (base * (80 + r) / 100).max(1)
}

pub(crate) async fn run(inner: Arc<Inner>) {
    clean_spool(&inner);
    update_outbox_count(&inner);
    loop {
        let next = tick(&inner);
        let wait = next.map_or(60, |t| t.saturating_sub(now()).clamp(1, 60));
        tokio::select! {
            _ = inner.kick.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}

/// Deletes spool files that no open transfer refers to (left behind when a delete failed, for
/// example a cancelled envelope still open on Windows).
fn clean_spool(inner: &Inner) {
    let Ok(open) = inner.store.open_transfers() else {
        return;
    };
    let keep: std::collections::HashSet<PathBuf> = open.into_iter().map(|t| t.envelope).collect();
    if let Ok(dir) = std::fs::read_dir(inner.spool()) {
        for e in dir.flatten() {
            if !keep.contains(&e.path()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Starts due attempts and expires old sends. Returns the earliest future attempt time.
fn tick(inner: &Arc<Inner>) -> Option<u64> {
    let Ok(open) = inner.store.open_transfers() else {
        return None;
    };
    let t = now();
    let mut next: Option<u64> = None;
    for mut row in open {
        if lock(&inner.sending).contains(&row.transfer_id) {
            continue; // an attempt is running; it writes the outcome
        }
        if row.expires_at <= t {
            fail(inner, &mut row, "expired");
            continue;
        }
        if row.envelope_len == 0 {
            if row.updated_at + 3600 < t {
                fail(inner, &mut row, "interrupted"); // sealing never finished (crash)
            }
            continue;
        }
        if row.via_outbox {
            continue;
        }
        if row.next_attempt_at > t {
            next = Some(next.map_or(row.next_attempt_at, |n| n.min(row.next_attempt_at)));
            continue;
        }
        {
            let mut sending = lock(&inner.sending);
            if sending.contains(&row.transfer_id) || sending.len() >= MAX_PARALLEL {
                continue;
            }
            sending.insert(row.transfer_id);
        }
        let guard = SendingGuard {
            inner: inner.clone(),
            tid: row.transfer_id,
        };
        let worker = inner.clone();
        tokio::spawn(async move {
            attempt(&worker, row).await;
            drop(guard);
        });
    }
    let _ = inner
        .store
        .purge_seen(t.saturating_sub(super::receive::SEEN_RETENTION_SECS));
    next
}

async fn attempt(inner: &Arc<Inner>, mut t: Transfer) {
    if !Path::new(&t.envelope).exists() {
        fail(inner, &mut t, "file_unreadable");
        return;
    }
    let first = t.attempts == 0;
    t.state = TransferState::Sending;
    t.attempts += 1;
    t.reason = None;
    if persist(inner, &mut t).is_err() {
        return;
    }
    let delivered = deliver(inner, &t, first).await;
    clear_progress(inner, &t.transfer_id);
    match delivered {
        Ok(receipt) => {
            if let Err(e) = complete(inner, &t.transfer_id, &t.target, &receipt) {
                tracing::warn!("receipt rejected: {e}");
                retry_later(inner, &mut t, e.code());
            }
        }
        Err(Offline) => retry_later(inner, &mut t, "offline"),
        Err(Failed(e)) if e.is_transient() => retry_later(inner, &mut t, e.code()),
        Err(Failed(e)) => fail(inner, &mut t, &format!("rejected:{}", e.code())),
    }
}

fn retry_later(inner: &Inner, t: &mut Transfer, reason: &str) {
    t.state = TransferState::Waiting;
    t.reason = Some(reason.to_owned());
    t.next_attempt_at = now() + backoff(inner, t.attempts);
    let _ = persist(inner, t);
}

enum DeliverError {
    Offline,
    Failed(Error),
}
use DeliverError::{Failed, Offline};

impl From<Error> for DeliverError {
    fn from(e: Error) -> Self {
        Failed(e)
    }
}

async fn deliver(
    inner: &Arc<Inner>,
    t: &Transfer,
    first: bool,
) -> std::result::Result<SignedObject, DeliverError> {
    let (addr, _) = discovery::probe_one(inner, &t.target)
        .await
        .ok_or(Offline)?;
    if !first {
        // A previous attempt (or the iPhone's background upload) may have arrived already.
        if let Ok(r) = fetch_receipt(inner, &addr, &t.target, &t.transfer_id).await {
            return Ok(r);
        }
    }
    let progress = |inner: &Arc<Inner>| {
        let mut r = Reporter::new(
            inner,
            t.transfer_id,
            ProgressPhase::Uploading,
            t.envelope_len,
        );
        move |n: u64| {
            if let Some(r) = &mut r {
                r.add(n);
            }
        }
    };
    let up = client::upload(
        &inner.http,
        &inner.keys,
        &addr,
        t.target,
        &t.transfer_id,
        &t.envelope,
        progress(inner),
    );
    match up.await {
        Err(Error::Remote { code, .. }) if code == "not_member" => {
            // The target has not seen the group version that added us yet: push it, retry once.
            groupsync::push_to(inner, &t.target, &addr).await?;
            Ok(client::upload(
                &inner.http,
                &inner.keys,
                &addr,
                t.target,
                &t.transfer_id,
                &t.envelope,
                progress(inner),
            )
            .await?)
        }
        r => Ok(r?),
    }
}

async fn fetch_receipt(
    inner: &Inner,
    addr: &str,
    target: &DeviceId,
    tid: &TransferId,
) -> Result<SignedObject> {
    let mut s = Session::open(&inner.http, &inner.keys, addr, *target).await?;
    let r: client::ReceiptResponse = s.get_json(&client::receipt_path(tid)).await?;
    Ok(r.receipt)
}

/// Verifies `receipt` from `recipient` for transfer `tid` and marks it `delivered`.
pub(crate) fn complete(
    inner: &Inner,
    tid: &TransferId,
    recipient: &DeviceId,
    signed: &SignedObject,
) -> Result<()> {
    let identity = inner.member_identity(recipient).ok_or(Error::NotMember)?;
    let receipt: Receipt = signed.verify(DOMAIN_RECEIPT, &identity)?;
    if receipt.transfer_id != *tid
        || receipt.sender != inner.me()
        || receipt.recipient != *recipient
    {
        return Err(Error::BadSignature);
    }
    let mut t = inner.store.transfer(tid)?.ok_or(Error::NotFound)?;
    if t.target != *recipient {
        return Err(Error::NotFound);
    }
    if t.state == TransferState::Delivered {
        return Ok(());
    }
    inner.store.set_receipt(tid, signed)?;
    if receipt.status == ReceiptStatus::Rejected {
        let code = receipt.code.as_deref().unwrap_or("internal");
        fail(inner, &mut t, &format!("rejected:{code}"));
        return Ok(());
    }
    t.state = TransferState::Delivered;
    t.reason = None;
    persist(inner, &mut t)?;
    let _ = std::fs::remove_file(&t.envelope);
    if t.via_outbox {
        update_outbox_count(inner);
    }
    Ok(())
}

impl Inner {
    /// The target was just seen online: retry its waiting sends now.
    pub(crate) fn kick_target(&self, id: &DeviceId) {
        let _ = self.store.kick_target(id, now());
        self.kick.notify_one();
    }
}

pub(crate) fn retry_now(inner: &Inner, tid: &TransferId) -> Result<()> {
    let mut t = inner.store.transfer(tid)?.ok_or(Error::NotFound)?;
    if t.state != TransferState::Waiting || t.via_outbox {
        return Ok(());
    }
    t.next_attempt_at = now();
    persist(inner, &mut t)?;
    inner.kick.notify_one();
    Ok(())
}

pub(crate) fn cancel(inner: &Inner, tid: &TransferId) -> Result<()> {
    let mut t = inner.store.transfer(tid)?.ok_or(Error::NotFound)?;
    if !t.state.is_final() {
        fail(inner, &mut t, "cancelled");
    }
    Ok(())
}

/// Sends to devices that are no longer members fail (`removed_from_group`).
pub(crate) fn fail_non_members(inner: &Inner) {
    let Ok(open) = inner.store.open_transfers() else {
        return;
    };
    for mut t in open {
        if !read(&inner.group).is_member(&t.target) {
            fail(inner, &mut t, "removed_from_group");
        }
    }
}

pub(crate) fn adopt(inner: &Inner, up: &PreparedUpload) -> Result<()> {
    if inner.store.transfer(&up.transfer_id)?.is_some() {
        return Ok(());
    }
    let t = now();
    let row = Transfer {
        transfer_id: up.transfer_id,
        target: up.target,
        kind: up.kind,
        name: up.name.clone(),
        mime: up.mime.clone(),
        size: up.size,
        envelope: up.envelope.clone(),
        envelope_len: up.content_length,
        state: TransferState::Sending,
        reason: None,
        via_outbox: false,
        attempts: 1,
        // Ask for the receipt at once. If the background upload is still running, a parallel
        // re-upload of the same transfer is refused (`rate_limited`) and retried later.
        next_attempt_at: t,
        created_at: up.created_at.min(t),
        updated_at: t,
        expires_at: t + inner.cfg.ttl_secs,
    };
    inner.store.insert_transfer(&row)?;
    inner.emit(Event::TransferChanged(row));
    inner.kick.notify_one();
    Ok(())
}

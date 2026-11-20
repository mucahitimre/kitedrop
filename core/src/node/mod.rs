//! The running device: listener, send queue, outbox, group sync, discovery and diagnostics,
//! behind one [`Node`] handle. This is the public API that the desktop app (directly) and the
//! mobile apps (through UniFFI) use; it is documented in `docs/api.md`.

mod client;
mod diagnostics;
mod discovery;
mod groupsync;
mod pair;
mod receive;
mod sender;
mod server;

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

use crate::envelope::{self, ContentHeader, ContentKind};
use crate::group::GroupState;
use crate::signed::{DOMAIN_DEVICE_RECORD, DeviceRecord, Platform, SignedObject};
use crate::store::{Received, Store, Transfer};
use crate::wire::{DEFAULT_PORT, PROTOCOL_VERSION};
use crate::{DeviceId, DeviceKeys, Error, Fingerprint, PublicIdentity, Result, TransferId};

pub use client::upload_authorization;
pub use diagnostics::{LanInterface, NetworkIssue, lan_interfaces};

/// Unix time in seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ports tried in order when none is configured (protocol.md §6).
pub const PORT_RANGE: std::ops::RangeInclusive<u16> = DEFAULT_PORT..=DEFAULT_PORT + 9;
pub const DEFAULT_TTL_SECS: u64 = 7 * 24 * 3600;
pub const MAX_TTL_SECS: u64 = 30 * 24 * 3600;
/// Text and links travel to the clipboard; larger content must be sent as a file.
pub const MAX_TEXT_LEN: u64 = 8 * 1024 * 1024;
pub const DEFAULT_RETRY_SCHEDULE: [u64; 7] = [2, 5, 15, 30, 60, 120, 300];

/// Static settings of a device. Paths must exist or be creatable.
#[derive(Clone, Debug)]
pub struct Config {
    /// Database and spool (sealed envelopes waiting to be sent).
    pub data_dir: PathBuf,
    /// Received files land here (`Downloads/kitedrop` on desktop and Android).
    pub download_dir: PathBuf,
    pub device_name: String,
    pub platform: Platform,
    /// Run the HTTP listener (every platform except iPhone).
    pub listen: bool,
    /// Listening port; `None` = first free port in 52525–52534, `Some(0)` = any (tests).
    pub port: Option<u16>,
    pub bind: IpAddr,
    /// Addresses to publish in the device record instead of the detected interface addresses
    /// (tests on loopback). `None` in production.
    pub advertise: Option<Vec<String>>,
    /// Expo push token (iPhone only).
    pub push_token: Option<String>,
    /// How long a send keeps retrying / an outbox item waits (default 7 days, max 30).
    pub ttl_secs: u64,
    /// Online-indicator probe interval: 60 s desktop on AC, 300 s on battery and Android.
    pub presence_interval_secs: u64,
    pub mdns: bool,
    pub scan: bool,
    pub retry_schedule_secs: Vec<u64>,
}

impl Config {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        download_dir: impl Into<PathBuf>,
        device_name: &str,
        platform: Platform,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            download_dir: download_dir.into(),
            device_name: device_name.to_owned(),
            platform,
            listen: platform != Platform::Ios,
            port: None,
            bind: IpAddr::from([0, 0, 0, 0]),
            advertise: None,
            push_token: None,
            ttl_secs: DEFAULT_TTL_SECS,
            presence_interval_secs: 60,
            mdns: true,
            scan: true,
            retry_schedule_secs: DEFAULT_RETRY_SCHEDULE.to_vec(),
        }
    }
}

/// What to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// A file, streamed from disk. `name`/`mime` default to the file name and a guess from the
    /// extension.
    File {
        path: PathBuf,
        name: Option<String>,
        mime: Option<String>,
    },
    Text(String),
    Link(String),
}

/// A group member as the UI shows it. Never disappears because discovery missed it; only
/// `online` changes (design §3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub device_id: DeviceId,
    pub fingerprint: Fingerprint,
    pub name: String,
    pub platform: Platform,
    pub is_self: bool,
    /// Answered a probe recently. Always `false` for iPhones (they do not listen).
    pub online: bool,
    /// Address of the last successful probe or upload.
    pub address: Option<String>,
    pub last_seen: Option<u64>,
}

/// Notifications for the UI and the platform layer.
#[derive(Clone, Debug)]
pub enum Event {
    TransferChanged(Transfer),
    Received(Received),
    /// Membership, names or reachability data changed; re-read [`Node::devices`].
    DevicesChanged,
    Presence {
        device: DeviceId,
        online: bool,
    },
    Paired {
        device: DeviceId,
        name: String,
    },
    /// Another member removed this device; the group state was reset.
    RemovedFromGroup,
    /// Items waiting in this device's outbox (desktop: hold a sleep assertion while > 0).
    OutboxPending {
        count: u32,
    },
    NetworkIssues(Vec<NetworkIssue>),
    /// A large send is being encrypted or uploaded (files of 1 MiB and more, at most ~5 per
    /// second per transfer). Not persisted; [`Node::progress`] has the latest value.
    TransferProgress {
        transfer_id: TransferId,
        progress: Progress,
    },
}

/// What a send in progress is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressPhase {
    /// Encrypting the content into the spool (before the first attempt).
    Sealing,
    /// Uploading the envelope to the target.
    Uploading,
}

impl ProgressPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sealing => "sealing",
            Self::Uploading => "uploading",
        }
    }
}

/// Bytes done of `total` in the current phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: ProgressPhase,
    pub done: u64,
    pub total: u64,
}

/// Platform services the core calls back into. Implemented by the desktop app and, through
/// UniFFI, by the mobile apps. Calls may come from any thread.
pub trait Host: Send + Sync + 'static {
    /// Text or link received (clipboard on desktop and Android). Called before the receipt is
    /// signed: an `Err` means "not stored", the sender gets no receipt and retries.
    fn deliver_text(&self, kind: ContentKind, text: &str, from: &DeviceInfo) -> Result<(), String>;
    /// A file was received and moved into the download folder at `path`. The host may move it
    /// further (iPhone: Photos) and returns the final location.
    fn deliver_file(
        &self,
        path: &Path,
        header: &ContentHeader,
        from: &DeviceInfo,
    ) -> Result<PathBuf, String>;
    /// HTTPS POST for push notifications (Expo push API); returns the HTTP status.
    fn https_post(&self, url: &str, json_body: &str) -> Result<u16, String>;
    fn event(&self, event: Event);
    /// Platform-only network checks (Windows network profile, Local Network permission).
    fn platform_issues(&self) -> Vec<NetworkIssue>;
}

struct ChallengeEntry {
    issued: u64,
    device: Option<DeviceId>,
    last_seq: u64,
}

#[derive(Clone, Debug, Default)]
struct Presence {
    online: bool,
    address: Option<String>,
    last_seen: Option<u64>,
}

pub(crate) struct Inner {
    keys: DeviceKeys,
    cfg: Config,
    store: Store,
    group: RwLock<GroupState>,
    host: Arc<dyn Host>,
    http: reqwest::Client,
    port: AtomicU16,
    challenges: Mutex<HashMap<[u8; 32], ChallengeEntry>>,
    pairing: Mutex<pair::Sessions>,
    inflight: Mutex<HashSet<(DeviceId, TransferId)>>,
    presence: RwLock<HashMap<DeviceId, Presence>>,
    discovered: Mutex<HashMap<DeviceId, Vec<String>>>,
    kick: tokio::sync::Notify,
    sending: Mutex<HashSet<TransferId>>,
    last_push: Mutex<HashMap<DeviceId, u64>>,
    push_scheduled: Mutex<HashSet<DeviceId>>,
    listen_error: Mutex<Option<NetworkIssue>>,
    scan: Mutex<discovery::ScanState>,
    push_token_override: Mutex<Option<String>>,
    outbox_count: std::sync::atomic::AtomicU32,
    progress: Mutex<HashMap<TransferId, Progress>>,
}

impl Inner {
    fn me(&self) -> DeviceId {
        self.keys.device_id()
    }

    fn spool(&self) -> PathBuf {
        self.cfg.data_dir.join("spool")
    }

    fn emit(&self, e: Event) {
        self.host.event(e);
    }

    fn member_identity(&self, id: &DeviceId) -> Option<PublicIdentity> {
        read(&self.group).member(id).map(|m| m.identity())
    }

    fn device_info(&self, id: &DeviceId) -> Option<DeviceInfo> {
        let g = read(&self.group);
        let m = g.member(id)?;
        let p = read(&self.presence).get(id).cloned().unwrap_or_default();
        let ident = m.identity();
        Some(DeviceInfo {
            device_id: *id,
            fingerprint: ident.fingerprint(),
            name: m.name.clone(),
            platform: m.platform,
            is_self: *id == self.me(),
            online: p.online || (*id == self.me() && self.port.load(Ordering::Relaxed) != 0),
            address: p.address,
            last_seen: p.last_seen,
        })
    }

    fn set_presence(&self, id: &DeviceId, online: bool, address: Option<&str>) {
        let changed = {
            let mut map = write(&self.presence);
            let p = map.entry(*id).or_default();
            let changed = p.online != online;
            p.online = online;
            if let Some(a) = address {
                p.address = Some(a.to_owned());
                p.last_seen = Some(now());
            }
            changed
        };
        if let Some(a) = address {
            let _ = self.store.set_peer_address(id, a, now());
        }
        if changed {
            self.emit(Event::Presence {
                device: *id,
                online,
            });
            if online {
                self.kick_target(id);
            }
        }
    }

    /// Remembers a discovered address (mDNS, scan, platform), most recent last, at most a few per
    /// device so spoofed announcements cannot slow down every probe.
    pub(crate) fn add_address(&self, id: DeviceId, address: String) {
        const MAX: usize = 4;
        let mut map = lock(&self.discovered);
        let list = map.entry(id).or_default();
        list.retain(|a| *a != address);
        list.push(address);
        if list.len() > MAX {
            list.remove(0);
        }
    }

    fn save_group(&self) -> Result<()> {
        let g = read(&self.group).clone();
        self.store.save_group(&g)
    }

    /// Addresses this device publishes in its record.
    fn own_addresses(&self) -> Vec<String> {
        if !self.cfg.listen {
            return Vec::new();
        }
        if let Some(a) = &self.cfg.advertise {
            return a.clone();
        }
        let port = self.port.load(Ordering::Relaxed);
        if port == 0 {
            return Vec::new();
        }
        diagnostics::local_ipv4()
            .into_iter()
            .map(|(ip, _)| format!("{ip}:{port}"))
            .collect()
    }

    /// Publishes a new self-signed device record if addresses or push token changed.
    /// Returns whether a new record was made.
    fn refresh_own_record(&self) -> Result<bool> {
        let me = self.me();
        let addresses = self.own_addresses();
        let token = self.cfg_push_token();
        let mut g = write(&self.group);
        let cur = g.record(&me).cloned();
        if cur
            .as_ref()
            .is_some_and(|r| r.addresses == addresses && r.push_token == token)
        {
            return Ok(false);
        }
        let record = DeviceRecord {
            v: PROTOCOL_VERSION,
            device_id: me,
            seq: cur.map_or(1, |r| r.seq + 1),
            addresses,
            push_token: token,
            updated_at: now(),
        };
        let signed = SignedObject::sign(&self.keys, DOMAIN_DEVICE_RECORD, &record)?;
        g.apply_record(&signed)?;
        drop(g);
        self.save_group()?;
        Ok(true)
    }

    fn cfg_push_token(&self) -> Option<String> {
        lock(&self.push_token_override)
            .clone()
            .or_else(|| self.cfg.push_token.clone())
    }
}

pub(crate) fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn write<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

/// Handle to a running device. Cheap to clone; stops when [`Node::shutdown`] is called or the
/// last handle is dropped.
#[derive(Clone)]
pub struct Node {
    inner: Arc<Inner>,
    rt: Arc<Runtime>,
}

struct Runtime(Mutex<Option<tokio::runtime::Runtime>>);

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(rt) = lock(&self.0).take() {
            rt.shutdown_background();
        }
    }
}

impl Node {
    /// Opens the state in `cfg.data_dir` (creating a single-member group on first start),
    /// starts the listener, the send queue, presence checks and discovery.
    pub fn start(cfg: Config, keys: DeviceKeys, host: Arc<dyn Host>) -> Result<Self> {
        if cfg.ttl_secs == 0 || cfg.ttl_secs > MAX_TTL_SECS {
            return Err(Error::Internal("ttl_secs must be 1 s … 30 days".into()));
        }
        std::fs::create_dir_all(&cfg.data_dir)?;
        std::fs::create_dir_all(cfg.data_dir.join("spool"))?;
        std::fs::create_dir_all(&cfg.download_dir)?;
        let store = Store::open(&cfg.data_dir.join("kitedrop.db"))?;
        let group = match store.load_group()? {
            Some(g) if g.is_member(&keys.device_id()) => g,
            _ => {
                let g = GroupState::create(
                    &keys,
                    &cfg.device_name,
                    cfg.platform,
                    now(),
                    crate::random(),
                )?;
                store.reset_group()?;
                store.save_group(&g)?;
                g
            }
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("kitedrop")
            .build()?;
        let inner = Arc::new(Inner {
            keys,
            store,
            group: RwLock::new(group),
            host,
            http: client::http_client(),
            port: AtomicU16::new(0),
            challenges: Mutex::default(),
            pairing: Mutex::default(),
            inflight: Mutex::default(),
            presence: RwLock::default(),
            discovered: Mutex::default(),
            kick: tokio::sync::Notify::new(),
            sending: Mutex::default(),
            last_push: Mutex::default(),
            push_scheduled: Mutex::default(),
            listen_error: Mutex::default(),
            scan: Mutex::default(),
            push_token_override: Mutex::default(),
            outbox_count: std::sync::atomic::AtomicU32::new(u32::MAX),
            progress: Mutex::new(HashMap::new()),
            cfg,
        });
        if inner.cfg.listen {
            rt.block_on(server::start(&inner))?;
        }
        inner.refresh_own_record()?;
        rt.spawn(sender::run(inner.clone()));
        rt.spawn(discovery::run(inner.clone()));
        let node = Self {
            inner,
            rt: Arc::new(Runtime(Mutex::new(Some(rt)))),
        };
        node.spawn(|i| async move { groupsync::push_all(&i).await });
        Ok(node)
    }

    fn spawn<F, Fut>(&self, f: F)
    where
        F: FnOnce(Arc<Inner>) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(rt) = lock(&self.rt.0).as_ref() {
            rt.spawn(f(self.inner.clone()));
        }
    }

    /// Runs `fut` on the node's runtime and waits for it from any executor (or none).
    async fn run<T: Send + 'static>(
        &self,
        fut: impl std::future::Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T> {
        let handle = lock(&self.rt.0)
            .as_ref()
            .map(|rt| rt.spawn(fut))
            .ok_or_else(|| Error::Internal("node stopped".into()))?;
        handle
            .await
            .map_err(|e| Error::Internal(format!("task: {e}")))?
    }

    /// Stops all background work. Persistent state stays; [`Node::start`] resumes it.
    pub fn shutdown(&self) {
        if let Some(rt) = lock(&self.rt.0).take() {
            rt.shutdown_background();
        }
        discovery::stop_mdns(&self.inner);
    }

    pub fn device_id(&self) -> DeviceId {
        self.inner.me()
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.inner.keys.public().fingerprint()
    }

    /// Actual listening port, if listening.
    pub fn port(&self) -> Option<u16> {
        match self.inner.port.load(Ordering::Relaxed) {
            0 => None,
            p => Some(p),
        }
    }

    /// Every group member including this device, in group-list order.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        let ids: Vec<DeviceId> = read(&self.inner.group)
            .current()
            .members
            .iter()
            .map(|m| m.identity().device_id())
            .collect();
        ids.iter()
            .filter_map(|id| self.inner.device_info(id))
            .collect()
    }

    /// Public keys of every member (for sealing outside a node, e.g. the iOS share extension).
    pub fn member_keys(&self) -> Vec<PublicIdentity> {
        read(&self.inner.group)
            .current()
            .members
            .iter()
            .map(|m| m.identity())
            .collect()
    }

    /// Addresses to try for a member, most likely first (last success, published, discovered).
    /// For callers that connect without a node, e.g. the iOS share extension.
    pub fn addresses(&self, id: &DeviceId) -> Vec<String> {
        discovery::candidates(&self.inner, id)
    }

    pub fn group_version(&self) -> u64 {
        read(&self.inner.group).version()
    }

    /// Queues `content` for each target and returns one transfer id per target, in order.
    /// The send is persisted (`queued`) before this returns; progress arrives as
    /// [`Event::TransferChanged`].
    pub async fn send(&self, targets: Vec<DeviceId>, content: Content) -> Result<Vec<TransferId>> {
        let inner = self.inner.clone();
        self.run(async move { sender::enqueue(&inner, targets, content).await })
            .await
    }

    pub fn transfers(&self, limit: u32) -> Result<Vec<Transfer>> {
        self.inner.store.transfers(limit)
    }

    pub fn transfer(&self, tid: &TransferId) -> Result<Option<Transfer>> {
        self.inner.store.transfer(tid)
    }

    /// Progress of a large send that is being encrypted or uploaded right now.
    pub fn progress(&self, tid: &TransferId) -> Option<Progress> {
        lock(&self.inner.progress).get(tid).copied()
    }

    /// Retries a waiting transfer now (same envelope and id). Failed sends are final; send again.
    pub fn retry(&self, tid: &TransferId) -> Result<()> {
        sender::retry_now(&self.inner, tid)
    }

    /// Gives up on a transfer that is not final yet: `failed(cancelled)`.
    pub fn cancel(&self, tid: &TransferId) -> Result<()> {
        sender::cancel(&self.inner, tid)
    }

    pub fn received(&self, limit: u32) -> Result<Vec<Received>> {
        self.inner.store.received(limit)
    }

    /// Starts showing a pairing QR (valid 60 s, one use) and returns its URI. `reverse` makes a
    /// QR for joining a group from a device that cannot scan (D15).
    pub fn create_invite(&self, reverse: bool) -> Result<String> {
        pair::create_invite(&self.inner, reverse)
    }

    /// Acts on a scanned pairing QR: joins the inviter's group (normal QR) or adds the device
    /// that shows it to this group (reverse QR). Returns the other device's id.
    pub async fn pair(&self, uri: &str) -> Result<DeviceId> {
        let inner = self.inner.clone();
        let uri = uri.to_owned();
        self.run(async move { pair::scan(&inner, &uri).await })
            .await
    }

    pub async fn remove_device(&self, id: DeviceId) -> Result<()> {
        let inner = self.inner.clone();
        self.run(async move { groupsync::remove(&inner, id).await })
            .await
    }

    pub async fn rename_device(&self, id: DeviceId, name: String) -> Result<()> {
        let inner = self.inner.clone();
        self.run(async move { groupsync::rename(&inner, id, name).await })
            .await
    }

    /// iPhone: pull items addressed to this device from every reachable member's outbox, store
    /// them, send receipts and apply newer group state. Returns how many items were stored.
    pub async fn poll_outboxes(&self) -> Result<u32> {
        let inner = self.inner.clone();
        self.run(async move { receive::poll_all(&inner).await })
            .await
    }

    /// Network changed or the app came to the foreground: republish addresses, probe every
    /// member now, retry waiting sends.
    pub fn refresh(&self) {
        self.spawn(|i| async move {
            let _ = i.refresh_own_record();
            discovery::probe_all(&i).await;
            groupsync::push_all(&i).await;
        });
    }

    pub fn diagnostics(&self) -> Vec<NetworkIssue> {
        diagnostics::detect(&self.inner)
    }

    /// iPhone: new Expo push token.
    pub fn set_push_token(&self, token: Option<String>) -> Result<()> {
        *lock(&self.inner.push_token_override) = token;
        if self.inner.refresh_own_record()? {
            self.spawn(|i| async move { groupsync::push_all(&i).await });
        }
        Ok(())
    }

    /// Address of a member found by platform discovery (iOS Bonjour through the Network
    /// framework). The core probes it before trusting it.
    pub fn add_discovered(&self, device: DeviceId, address: String) {
        self.inner.add_address(device, address);
        self.spawn(|i| async move {
            discovery::probe_one(&i, &device).await;
        });
    }

    /// iPhone: registers an upload that the share extension handed to a background session
    /// (see [`prepare_upload`]). The transfer is `sending` until its receipt is fetched.
    pub fn adopt_upload(&self, upload: &PreparedUpload) -> Result<()> {
        sender::adopt(&self.inner, upload)
    }
}

/// An upload sealed and signed by the iPhone share extension, which has no running node: the
/// extension gives `url`, `authorization` and the envelope file to a background `URLSession`,
/// and passes this record to the app (shared container) for [`Node::adopt_upload`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedUpload {
    #[serde(with = "crate::wire::b64_array")]
    pub transfer_id: TransferId,
    #[serde(with = "crate::wire::b64_array")]
    pub target: DeviceId,
    pub url: String,
    pub authorization: String,
    pub content_length: u64,
    pub envelope: PathBuf,
    pub kind: ContentKind,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub created_at: u64,
}

/// Seals `content` for `target` into `envelope_path` and signs the upload request to
/// `address`. Stateless; memory use is one chunk regardless of size.
pub fn prepare_upload(
    keys: &DeviceKeys,
    target: &PublicIdentity,
    address: &str,
    header: &ContentHeader,
    content: impl std::io::Read,
    envelope_path: &Path,
) -> Result<PreparedUpload> {
    let transfer_id = crate::random::<16>();
    let file = std::fs::File::create(envelope_path)?;
    let len = envelope::seal(
        keys,
        target,
        transfer_id,
        header,
        content,
        std::io::BufWriter::new(file),
        &mut crate::os_rng(),
    )?;
    Ok(PreparedUpload {
        transfer_id,
        target: target.device_id(),
        url: format!("http://{address}/v1/inbox"),
        authorization: upload_authorization(keys, target.device_id(), &transfer_id, len, now()),
        content_length: len,
        envelope: envelope_path.to_owned(),
        kind: header.kind,
        name: header.name.clone(),
        mime: header.mime.clone(),
        size: header.size,
        created_at: header.created_at,
    })
}

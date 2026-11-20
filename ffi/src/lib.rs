//! UniFFI surface of kitedrop-core for Swift (iOS app and share extension) and Kotlin (Android
//! app and service): the frozen `Node` API of docs/api.md, plus stateless helpers for the iOS
//! share extension. Ids and keys cross the boundary as base64url strings / byte arrays.

use std::path::PathBuf;
use std::sync::Arc;

use kitedrop_core::envelope::{ContentHeader, ContentKind, DEFAULT_CHUNK_SIZE};
use kitedrop_core::signed::Platform;
use kitedrop_core::store::{Received, Transfer};
use kitedrop_core::wire::{PROTOCOL_VERSION, b64_decode_array, b64_encode};
use kitedrop_core::{
    Config, Content, DeviceId, DeviceInfo, DeviceKeys, Event, Host, NetworkIssue, Node,
    PreparedUpload, Progress, PublicIdentity,
};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// `detail`, not `message`: a `message` field clashes with `Throwable.message` in Kotlin.
    #[error("{code}: {detail}")]
    Failed { code: String, detail: String },
}

impl From<kitedrop_core::Error> for FfiError {
    fn from(e: kitedrop_core::Error) -> Self {
        FfiError::Failed {
            code: e.code().into(),
            detail: e.to_string(),
        }
    }
}

impl From<std::io::Error> for FfiError {
    fn from(e: std::io::Error) -> Self {
        kitedrop_core::Error::from(e).into()
    }
}

fn invalid(what: &str) -> FfiError {
    FfiError::Failed {
        code: "malformed".into(),
        detail: what.into(),
    }
}

/// Error a platform callback returns; the core turns it into "not stored" (no receipt).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum HostError {
    #[error("{detail}")]
    Failed { detail: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for HostError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        HostError::Failed { detail: e.reason }
    }
}

fn id_from(s: &str) -> Result<DeviceId, FfiError> {
    b64_decode_array::<16>(s).ok_or_else(|| invalid("device or transfer id"))
}

fn ids_from(v: &[String]) -> Result<Vec<DeviceId>, FfiError> {
    v.iter().map(|s| id_from(s)).collect()
}

fn platform_str(p: Platform) -> String {
    match p {
        Platform::Ios => "ios",
        Platform::Android => "android",
        Platform::Macos => "macos",
        Platform::Windows => "windows",
    }
    .into()
}

fn kind_str(k: ContentKind) -> String {
    match k {
        ContentKind::File => "file",
        ContentKind::Text => "text",
        ContentKind::Link => "link",
    }
    .into()
}

fn keys_from(identity: &[u8]) -> Result<DeviceKeys, FfiError> {
    if identity.len() != 64 {
        return Err(invalid("identity must be 64 bytes"));
    }
    let sig: [u8; 32] = identity[..32].try_into().expect("32 bytes");
    let kem: [u8; 32] = identity[32..].try_into().expect("32 bytes");
    Ok(DeviceKeys::from_secret_bytes(&sig, &kem)?)
}

// ---- records ----

#[derive(uniffi::Record)]
pub struct NodeConfig {
    pub data_dir: String,
    pub download_dir: String,
    pub device_name: String,
    /// "ios" | "android" | "macos" | "windows"
    pub platform: String,
    pub listen: bool,
    pub presence_interval_secs: u64,
    pub mdns: bool,
    pub push_token: Option<String>,
}

#[derive(uniffi::Record)]
pub struct DeviceItem {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub is_self: bool,
    pub online: bool,
    pub address: Option<String>,
    pub last_seen: Option<u64>,
    /// Public keys, so the iOS share extension can seal to this device without a node.
    pub sig_pk: Vec<u8>,
    pub kem_pk: Vec<u8>,
    /// Addresses to try, most likely first (empty for iPhones).
    pub addresses: Vec<String>,
}

#[derive(uniffi::Record)]
pub struct TransferItem {
    pub id: String,
    pub target_id: String,
    pub kind: String,
    pub name: String,
    pub size: u64,
    /// queued | sending | delivered | waiting | failed
    pub state: String,
    pub reason: Option<String>,
    pub via_outbox: bool,
    pub created_at: u64,
    pub updated_at: u64,
    /// Large sends being encrypted or uploaded right now (only in `transfers()`).
    pub progress: Option<ProgressItem>,
}

/// `phase`: sealing | uploading; `done` of `total` bytes in that phase.
#[derive(uniffi::Record)]
pub struct ProgressItem {
    pub phase: String,
    pub done: u64,
    pub total: u64,
}

fn progress_item(p: Progress) -> ProgressItem {
    ProgressItem {
        phase: p.phase.as_str().into(),
        done: p.done,
        total: p.total,
    }
}

#[derive(uniffi::Record)]
pub struct ReceivedItem {
    pub id: String,
    pub sender_id: String,
    pub kind: String,
    pub name: String,
    pub size: u64,
    pub location: Option<String>,
    pub at: u64,
}

#[derive(uniffi::Enum)]
pub enum NodeEvent {
    TransferChanged {
        transfer: TransferItem,
    },
    Received {
        item: ReceivedItem,
    },
    DevicesChanged,
    Presence {
        device: String,
        online: bool,
    },
    Paired {
        device: String,
        name: String,
    },
    RemovedFromGroup,
    OutboxPending {
        count: u32,
    },
    NetworkIssues {
        codes: Vec<String>,
    },
    TransferProgress {
        transfer_id: String,
        progress: ProgressItem,
    },
}

fn transfer_item(t: &Transfer) -> TransferItem {
    TransferItem {
        id: b64_encode(&t.transfer_id),
        target_id: b64_encode(&t.target),
        kind: kind_str(t.kind),
        name: t.name.clone(),
        size: t.size,
        state: t.state.as_str().into(),
        reason: t.reason.clone(),
        via_outbox: t.via_outbox,
        created_at: t.created_at,
        updated_at: t.updated_at,
        progress: None,
    }
}

fn received_item(r: &Received) -> ReceivedItem {
    ReceivedItem {
        id: b64_encode(&r.transfer_id),
        sender_id: b64_encode(&r.sender),
        kind: kind_str(r.kind),
        name: r.name.clone(),
        size: r.size,
        location: r
            .location
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        at: r.at,
    }
}

fn device_item(
    d: &DeviceInfo,
    keys: Option<&PublicIdentity>,
    addresses: Vec<String>,
) -> DeviceItem {
    DeviceItem {
        id: b64_encode(&d.device_id),
        name: d.name.clone(),
        platform: platform_str(d.platform),
        is_self: d.is_self,
        online: d.online,
        address: d.address.clone(),
        last_seen: d.last_seen,
        sig_pk: keys.map(|k| k.sig_pk.to_vec()).unwrap_or_default(),
        kem_pk: keys.map(|k| k.kem_pk.to_vec()).unwrap_or_default(),
        addresses,
    }
}

// ---- platform callbacks ----

/// Implemented in Kotlin / Swift (see `kitedrop_core::Host`). Called from background threads.
#[uniffi::export(with_foreign)]
pub trait NodeHost: Send + Sync {
    /// Text or link received: put it on the clipboard. Error = not stored, no receipt.
    fn deliver_text(&self, kind: String, text: String, from_name: String) -> Result<(), HostError>;
    /// File received at `path` (in the download folder); move it if needed and return the final
    /// location (content URI on Android, Photos identifier or file path on iOS).
    fn deliver_file(
        &self,
        path: String,
        name: String,
        mime: String,
        from_name: String,
    ) -> Result<String, HostError>;
    /// HTTPS POST for the Expo push API; returns the HTTP status.
    fn https_post(&self, url: String, json_body: String) -> Result<u16, HostError>;
    fn on_event(&self, event: NodeEvent);
    /// Platform-only network checks, as issue codes (`local_network_denied`).
    fn platform_issues(&self) -> Vec<String>;
}

struct HostBridge(Arc<dyn NodeHost>);

impl Host for HostBridge {
    fn deliver_text(&self, kind: ContentKind, text: &str, from: &DeviceInfo) -> Result<(), String> {
        self.0
            .deliver_text(kind_str(kind), text.to_owned(), from.name.clone())
            .map_err(|e| e.to_string())
    }

    fn deliver_file(
        &self,
        path: &std::path::Path,
        header: &ContentHeader,
        from: &DeviceInfo,
    ) -> Result<PathBuf, String> {
        self.0
            .deliver_file(
                path.to_string_lossy().into_owned(),
                header.name.clone(),
                header.mime.clone(),
                from.name.clone(),
            )
            .map(PathBuf::from)
            .map_err(|e| e.to_string())
    }

    fn https_post(&self, url: &str, json_body: &str) -> Result<u16, String> {
        self.0
            .https_post(url.to_owned(), json_body.to_owned())
            .map_err(|e| e.to_string())
    }

    fn event(&self, e: Event) {
        let ev = match e {
            Event::TransferChanged(t) => NodeEvent::TransferChanged {
                transfer: transfer_item(&t),
            },
            Event::Received(r) => NodeEvent::Received {
                item: received_item(&r),
            },
            Event::DevicesChanged => NodeEvent::DevicesChanged,
            Event::Presence { device, online } => NodeEvent::Presence {
                device: b64_encode(&device),
                online,
            },
            Event::Paired { device, name } => NodeEvent::Paired {
                device: b64_encode(&device),
                name,
            },
            Event::RemovedFromGroup => NodeEvent::RemovedFromGroup,
            Event::OutboxPending { count } => NodeEvent::OutboxPending { count },
            Event::NetworkIssues(list) => NodeEvent::NetworkIssues {
                codes: list.iter().map(|i| i.code().to_owned()).collect(),
            },
            Event::TransferProgress {
                transfer_id,
                progress,
            } => NodeEvent::TransferProgress {
                transfer_id: b64_encode(&transfer_id),
                progress: progress_item(progress),
            },
        };
        self.0.on_event(ev);
    }

    fn platform_issues(&self) -> Vec<NetworkIssue> {
        self.0
            .platform_issues()
            .into_iter()
            .filter_map(|c| match c.as_str() {
                "local_network_denied" => Some(NetworkIssue::LocalNetworkDenied),
                "windows_public_profile" => Some(NetworkIssue::WindowsPublicProfile),
                _ => None,
            })
            .collect()
    }
}

// ---- the node ----

#[derive(uniffi::Object)]
pub struct KitedropNode {
    node: Node,
}

#[uniffi::export]
impl KitedropNode {
    /// Starts the device. `identity` is the 64-byte secret from [`generate_identity`], kept by the
    /// app in Keychain / Keystore-protected storage (design §5).
    #[uniffi::constructor]
    pub fn start(
        config: NodeConfig,
        identity: Vec<u8>,
        host: Arc<dyn NodeHost>,
    ) -> Result<Arc<Self>, FfiError> {
        let platform = match config.platform.as_str() {
            "ios" => Platform::Ios,
            "android" => Platform::Android,
            "macos" => Platform::Macos,
            "windows" => Platform::Windows,
            _ => return Err(invalid("platform")),
        };
        let mut cfg = Config::new(
            config.data_dir,
            config.download_dir,
            &config.device_name,
            platform,
        );
        cfg.listen = config.listen;
        cfg.presence_interval_secs = config.presence_interval_secs;
        cfg.mdns = config.mdns;
        cfg.push_token = config.push_token;
        let node = Node::start(cfg, keys_from(&identity)?, Arc::new(HostBridge(host)))?;
        Ok(Arc::new(Self { node }))
    }

    pub fn device_id(&self) -> String {
        b64_encode(&self.node.device_id())
    }

    pub fn port(&self) -> Option<u16> {
        self.node.port()
    }

    pub fn devices(&self) -> Vec<DeviceItem> {
        let keys = self.node.member_keys();
        self.node
            .devices()
            .iter()
            .map(|d| {
                let k = keys.iter().find(|k| k.device_id() == d.device_id);
                let addrs = if d.is_self {
                    Vec::new()
                } else {
                    self.node.addresses(&d.device_id)
                };
                device_item(d, k, addrs)
            })
            .collect()
    }

    pub async fn send_file(
        &self,
        targets: Vec<String>,
        path: String,
        name: Option<String>,
        mime: Option<String>,
    ) -> Result<Vec<String>, FfiError> {
        let content = Content::File {
            path: path.into(),
            name,
            mime,
        };
        self.send(targets, content).await
    }

    pub async fn send_text(
        &self,
        targets: Vec<String>,
        text: String,
    ) -> Result<Vec<String>, FfiError> {
        self.send(targets, Content::Text(text)).await
    }

    pub async fn send_link(
        &self,
        targets: Vec<String>,
        url: String,
    ) -> Result<Vec<String>, FfiError> {
        self.send(targets, Content::Link(url)).await
    }

    pub fn transfers(&self, limit: u32) -> Result<Vec<TransferItem>, FfiError> {
        Ok(self
            .node
            .transfers(limit)?
            .iter()
            .map(|t| TransferItem {
                progress: self.node.progress(&t.transfer_id).map(progress_item),
                ..transfer_item(t)
            })
            .collect())
    }

    pub fn received(&self, limit: u32) -> Result<Vec<ReceivedItem>, FfiError> {
        Ok(self
            .node
            .received(limit)?
            .iter()
            .map(received_item)
            .collect())
    }

    pub fn retry(&self, id: String) -> Result<(), FfiError> {
        Ok(self.node.retry(&id_from(&id)?)?)
    }

    pub fn cancel(&self, id: String) -> Result<(), FfiError> {
        Ok(self.node.cancel(&id_from(&id)?)?)
    }

    pub fn create_invite(&self, reverse: bool) -> Result<String, FfiError> {
        Ok(self.node.create_invite(reverse)?)
    }

    /// Acts on a scanned QR / pasted link; returns the other device's id.
    pub async fn pair(&self, uri: String) -> Result<String, FfiError> {
        Ok(b64_encode(&self.node.pair(&uri).await?))
    }

    pub async fn remove_device(&self, id: String) -> Result<(), FfiError> {
        Ok(self.node.remove_device(id_from(&id)?).await?)
    }

    pub async fn rename_device(&self, id: String, name: String) -> Result<(), FfiError> {
        Ok(self.node.rename_device(id_from(&id)?, name).await?)
    }

    pub async fn poll_outboxes(&self) -> Result<u32, FfiError> {
        Ok(self.node.poll_outboxes().await?)
    }

    pub fn refresh(&self) {
        self.node.refresh();
    }

    pub fn diagnostics(&self) -> Vec<String> {
        self.node
            .diagnostics()
            .iter()
            .map(|i| i.code().to_owned())
            .collect()
    }

    pub fn set_push_token(&self, token: Option<String>) -> Result<(), FfiError> {
        Ok(self.node.set_push_token(token)?)
    }

    /// iOS Bonjour result: `id_hex` is the TXT `id` (hex device id).
    pub fn add_discovered(&self, id_hex: String, address: String) {
        if let Some(id) = parse_hex_id(&id_hex) {
            self.node.add_discovered(id, address);
        }
    }

    /// iOS: registers an upload the share extension started (`prepared_json` from
    /// [`prepare_upload`]).
    pub fn adopt_upload(&self, prepared_json: String) -> Result<(), FfiError> {
        let up: PreparedUpload =
            serde_json::from_str(&prepared_json).map_err(|_| invalid("prepared upload"))?;
        Ok(self.node.adopt_upload(&up)?)
    }

    pub fn shutdown(&self) {
        self.node.shutdown();
    }
}

impl KitedropNode {
    async fn send(&self, targets: Vec<String>, content: Content) -> Result<Vec<String>, FfiError> {
        let ids = self.node.send(ids_from(&targets)?, content).await?;
        Ok(ids.iter().map(|t| b64_encode(t)).collect())
    }
}

fn parse_hex_id(s: &str) -> Option<DeviceId> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

// ---- stateless helpers ----

#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

/// A new device identity: 64 secret bytes (`ed25519 || x25519`) for the platform secure store.
#[uniffi::export]
pub fn generate_identity() -> Vec<u8> {
    let keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
    let (sig, kem) = keys.secret_bytes();
    [&sig[..], &kem[..]].concat()
}

/// Base64url device id of an identity (for display before the node runs).
#[uniffi::export]
pub fn identity_device_id(identity: Vec<u8>) -> Result<String, FfiError> {
    Ok(b64_encode(&keys_from(&identity)?.device_id()))
}

/// QR code modules for drawing a pairing code without a JS library: `size × size` cells,
/// row-major, 1 = dark. Includes no quiet zone.
#[derive(uniffi::Record)]
pub struct QrMatrix {
    pub size: u32,
    pub modules: Vec<u8>,
}

#[uniffi::export]
pub fn qr_matrix(text: String) -> Result<QrMatrix, FfiError> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|_| invalid("text too long for a QR code"))?;
    let size = code.width() as u32;
    let modules = code
        .to_colors()
        .into_iter()
        .map(|c| u8::from(c == qrcode::Color::Dark))
        .collect();
    Ok(QrMatrix { size, modules })
}

#[derive(uniffi::Record)]
pub struct PreparedUploadItem {
    pub transfer_id: String,
    pub url: String,
    pub authorization: String,
    pub content_length: u64,
    pub envelope_path: String,
    /// Hand this to the app (shared container) for `KitedropNode::adopt_upload`.
    pub json: String,
}

/// iOS share extension: seals the item for the target into `envelope_path` (streaming, one chunk
/// of memory) and signs the upload. `kind` is file | text | link; files come from `source_path`,
/// text and links from `text`.
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn prepare_upload(
    identity: Vec<u8>,
    target_sig_pk: Vec<u8>,
    target_kem_pk: Vec<u8>,
    address: String,
    kind: String,
    source_path: Option<String>,
    text: Option<String>,
    name: String,
    mime: String,
    envelope_path: String,
) -> Result<PreparedUploadItem, FfiError> {
    let keys = keys_from(&identity)?;
    let target = PublicIdentity {
        sig_pk: target_sig_pk.try_into().map_err(|_| invalid("sig_pk"))?,
        kem_pk: target_kem_pk.try_into().map_err(|_| invalid("kem_pk"))?,
    };
    let kind = match kind.as_str() {
        "file" => ContentKind::File,
        "text" => ContentKind::Text,
        "link" => ContentKind::Link,
        _ => return Err(invalid("kind")),
    };
    let (reader, size): (Box<dyn std::io::Read>, u64) = match (&source_path, &text) {
        (Some(p), _) if kind == ContentKind::File => {
            let f = std::fs::File::open(p)?;
            let size = f.metadata()?.len();
            (Box::new(std::io::BufReader::new(f)), size)
        }
        (_, Some(t)) if kind != ContentKind::File => (
            Box::new(std::io::Cursor::new(t.clone().into_bytes())),
            t.len() as u64,
        ),
        _ => return Err(invalid("file needs source_path, text and link need text")),
    };
    let header = ContentHeader {
        v: PROTOCOL_VERSION,
        kind,
        name: if kind == ContentKind::File {
            name
        } else {
            String::new()
        },
        mime,
        size,
        created_at: kitedrop_core::node::now(),
        chunk_size: DEFAULT_CHUNK_SIZE,
    };
    let up = kitedrop_core::node::prepare_upload(
        &keys,
        &target,
        &address,
        &header,
        reader,
        std::path::Path::new(&envelope_path),
    )?;
    Ok(PreparedUploadItem {
        transfer_id: b64_encode(&up.transfer_id),
        url: up.url.clone(),
        authorization: up.authorization.clone(),
        content_length: up.content_length,
        envelope_path: envelope_path.clone(),
        json: serde_json::to_string(&up).map_err(|_| invalid("json"))?,
    })
}

//! The listener: every endpoint of protocol.md §6 with the authentication rules of §7.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::auth::{CHALLENGE_TTL_SECS, ProbeRequest, ProbeResponse, RequestAuth, RequestTarget};
use crate::pairing::{AcceptDelivery, JoinMessage};
use crate::signed::{DOMAIN_RECEIPT, Receipt, SignedObject};
use crate::store::TransferState;
use crate::wire::{b64_array, b64_decode_array};
use crate::{DeviceId, Error, Result, TransferId};

use super::{
    ChallengeEntry, Event, Inner, NetworkIssue, PORT_RANGE, groupsync, lock, now, pair, read,
    receive,
};

const MAX_JSON: usize = 64 * 1024;
const MAX_CHALLENGES: usize = 4096;
/// Upload timestamps may be this far from the listener's clock (protocol.md §7.2).
const UPLOAD_SKEW_SECS: u64 = 24 * 3600;

type Shared = Arc<Inner>;

pub(crate) struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_server_error() {
            tracing::warn!("request failed: {}", self.0);
        }
        let body = serde_json::json!({
            "error": { "code": self.0.code(), "message": self.0.to_string() }
        });
        (status, axum::Json(body)).into_response()
    }
}

type ApiResult = std::result::Result<Response, ApiError>;

fn json_ok(v: &impl Serialize) -> Response {
    axum::Json(serde_json::to_value(v).expect("serializable")).into_response()
}

async fn read_json<T: serde::de::DeserializeOwned>(body: Body) -> Result<T> {
    let bytes = axum::body::to_bytes(body, MAX_JSON)
        .await
        .map_err(|_| Error::Malformed("body too large or unreadable"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) async fn start(inner: &Shared) -> Result<()> {
    let ports: Vec<u16> = match inner.cfg.port {
        Some(p) => vec![p],
        None => PORT_RANGE.collect(),
    };
    let mut listener = None;
    for p in &ports {
        match tokio::net::TcpListener::bind((inner.cfg.bind, *p)).await {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(e) if inner.cfg.port.is_some() => return Err(e.into()),
            Err(_) => {}
        }
    }
    let Some(listener) = listener else {
        *lock(&inner.listen_error) = Some(NetworkIssue::PortInUse);
        return Ok(());
    };
    inner
        .port
        .store(listener.local_addr()?.port(), Ordering::Relaxed);
    let app = router(inner.clone());
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("listener stopped: {e}");
        }
    });
    Ok(())
}

fn router(inner: Shared) -> Router {
    Router::new()
        .route("/v1/probe", post(probe))
        .route("/v1/challenge", post(challenge))
        .route("/v1/pair/join", post(pair_join))
        .route("/v1/pair/offer", post(pair_offer))
        .route("/v1/pair/accept", post(pair_accept))
        .route("/v1/inbox", post(inbox))
        .route("/v1/receipts", post(post_receipt))
        .route("/v1/receipts/{tid}", get(get_receipt))
        .route("/v1/outbox", get(outbox_list))
        .route("/v1/outbox/{tid}", get(outbox_item))
        .route("/v1/group", get(get_group).post(post_group))
        .with_state(inner)
}

// ---- authentication (protocol.md §7.2) ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// Current member, listener-issued challenge.
    Challenge,
    /// Current or former member (so a removed device can learn it was removed), challenge.
    ChallengeFormer,
    /// Upload: nonce = transfer id, seq = 0, ts within ±24 h.
    Upload,
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// Checks the `Authorization` header and returns the client's device id and the nonce.
fn authorize(
    inner: &Inner,
    req: &Request,
    content_length: u64,
    rule: Rule,
) -> Result<(DeviceId, Vec<u8>)> {
    let value = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Malformed("missing authorization"))?;
    let auth = RequestAuth::from_header(value)?;
    let identity = {
        let g = read(&inner.group);
        match g.member(&auth.device) {
            Some(m) => Some(m.identity()),
            None if rule == Rule::ChallengeFormer => g
                .chain()
                .iter()
                .rev()
                .find_map(|v| v.list.member(&auth.device).map(|m| m.identity())),
            None => None,
        }
    }
    .ok_or(Error::NotMember)?;
    let path = req
        .uri()
        .path_and_query()
        .map_or(req.uri().path(), |p| p.as_str());
    let target = RequestTarget {
        server: inner.me(),
        method: req.method().as_str(),
        path,
        content_length,
    };
    auth.verify(&identity, &target)?;
    match rule {
        Rule::Upload => {
            if auth.nonce.len() != 16 || auth.seq != 0 {
                return Err(Error::Malformed("upload nonce"));
            }
            if now().abs_diff(auth.ts) > UPLOAD_SKEW_SECS {
                return Err(Error::StaleRequest);
            }
        }
        Rule::Challenge | Rule::ChallengeFormer => {
            let nonce: [u8; 32] = auth
                .nonce
                .as_slice()
                .try_into()
                .map_err(|_| Error::ChallengeInvalid)?;
            let mut map = lock(&inner.challenges);
            let entry = map.get_mut(&nonce).ok_or(Error::ChallengeInvalid)?;
            if now().saturating_sub(entry.issued) > CHALLENGE_TTL_SECS
                || entry.device.is_some_and(|d| d != auth.device)
                || auth.seq <= entry.last_seq
            {
                return Err(Error::ChallengeInvalid);
            }
            entry.device = Some(auth.device);
            entry.last_seq = auth.seq;
        }
    }
    Ok((auth.device, auth.nonce))
}

fn transfer_id_param(s: &str) -> Result<TransferId> {
    b64_decode_array(s).ok_or(Error::Malformed("transfer id"))
}

// ---- handlers ----

async fn probe(State(inner): State<Shared>, body: Body) -> ApiResult {
    let req: ProbeRequest = read_json(body).await?;
    let (gid, gv) = {
        let g = read(&inner.group);
        (g.group_id(), g.version())
    };
    let seq = read(&inner.group).record_seq(&inner.me());
    Ok(json_ok(&ProbeResponse::sign(
        &inner.keys,
        &req.nonce,
        gid,
        gv,
        seq,
        now(),
    )))
}

#[derive(Serialize)]
struct ChallengeAnswer {
    #[serde(with = "b64_array")]
    nonce: [u8; 32],
    expires_in: u64,
}

async fn challenge(State(inner): State<Shared>) -> ApiResult {
    let nonce = crate::random::<32>();
    let mut map = lock(&inner.challenges);
    let t = now();
    if map.len() >= MAX_CHALLENGES {
        map.retain(|_, e| t.saturating_sub(e.issued) <= CHALLENGE_TTL_SECS);
    }
    if map.len() >= MAX_CHALLENGES {
        // Anyone may ask for challenges: evict the oldest one nobody has used yet, so a flood
        // cannot lock members out. Challenges bound to a member are kept.
        let oldest = map
            .iter()
            .filter(|(_, e)| e.device.is_none())
            .min_by_key(|(_, e)| e.issued)
            .map(|(k, _)| *k);
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => return Err(Error::RateLimited.into()),
        }
    }
    map.insert(
        nonce,
        ChallengeEntry {
            issued: t,
            device: None,
            last_seq: 0,
        },
    );
    Ok(json_ok(&ChallengeAnswer {
        nonce,
        expires_in: CHALLENGE_TTL_SECS,
    }))
}

async fn pair_join(State(inner): State<Shared>, body: Body) -> ApiResult {
    let msg: JoinMessage = read_json(body).await?;
    let accept = pair::handle_join(&inner, &msg)?;
    Ok(json_ok(&accept))
}

async fn pair_offer(State(inner): State<Shared>, body: Body) -> ApiResult {
    let msg: JoinMessage = read_json(body).await?;
    let join = pair::handle_offer(&inner, &msg)?;
    Ok(json_ok(&join))
}

async fn pair_accept(State(inner): State<Shared>, body: Body) -> ApiResult {
    let msg: AcceptDelivery = read_json(body).await?;
    pair::handle_accept(&inner, &msg)?;
    Ok(json_ok(&serde_json::json!({})))
}

async fn inbox(State(inner): State<Shared>, req: Request) -> ApiResult {
    let len = content_length(req.headers()).ok_or(Error::Malformed("content-length required"))?;
    let (client, nonce) = authorize(&inner, &req, len, Rule::Upload)?;
    let tid: TransferId = nonce.try_into().expect("checked length");
    let stream = req.into_body().into_data_stream();
    let receipt = receive::accept_upload(&inner, client, tid, len, stream).await?;
    Ok(json_ok(&serde_json::json!({ "receipt": receipt })))
}

async fn get_receipt(
    State(inner): State<Shared>,
    UrlPath(tid): UrlPath<String>,
    req: Request,
) -> ApiResult {
    let (client, _) = authorize(&inner, &req, 0, Rule::Challenge)?;
    let tid = transfer_id_param(&tid)?;
    match inner.store.seen(&client, &tid)? {
        Some(Some(receipt)) => Ok(json_ok(&serde_json::json!({ "receipt": receipt }))),
        _ => Err(Error::NotFound.into()),
    }
}

#[derive(Serialize)]
struct OutboxItem {
    #[serde(with = "b64_array")]
    transfer_id: TransferId,
    size: u64,
    created_at: u64,
}

async fn outbox_list(State(inner): State<Shared>, req: Request) -> ApiResult {
    let (client, _) = authorize(&inner, &req, 0, Rule::Challenge)?;
    let items: Vec<OutboxItem> = inner
        .store
        .outbox_for(&client)?
        .into_iter()
        .map(|t| OutboxItem {
            transfer_id: t.transfer_id,
            size: t.envelope_len,
            created_at: t.created_at,
        })
        .collect();
    let gv = read(&inner.group).version();
    Ok(json_ok(
        &serde_json::json!({ "items": items, "group_version": gv }),
    ))
}

/// `Range: bytes=N-` (resume) → offset N.
fn range_start(headers: &HeaderMap) -> Option<u64> {
    let v = headers.get(header::RANGE)?.to_str().ok()?;
    let rest = v.strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    end.is_empty().then_some(())?;
    start.parse().ok()
}

async fn outbox_item(
    State(inner): State<Shared>,
    UrlPath(tid): UrlPath<String>,
    req: Request,
) -> ApiResult {
    let (client, _) = authorize(&inner, &req, 0, Rule::Challenge)?;
    let tid = transfer_id_param(&tid)?;
    let t = inner
        .store
        .transfer(&tid)?
        .filter(|t| t.target == client && t.via_outbox && t.state == TransferState::Waiting)
        .ok_or(Error::NotFound)?;
    let mut file = tokio::fs::File::open(&t.envelope)
        .await
        .map_err(|_| Error::NotFound)?;
    let len = file.metadata().await?.len();
    let start = range_start(req.headers()).unwrap_or(0);
    if start > len || (start == len && start > 0) {
        return Ok(StatusCode::RANGE_NOT_SATISFIABLE.into_response());
    }
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        file.take(len - start),
        64 * 1024,
    ));
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len - start));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if start > 0 {
        *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
        let range = format!("bytes {start}-{}/{len}", len.saturating_sub(1));
        resp.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&range).expect("ascii"),
        );
    }
    Ok(resp)
}

async fn post_receipt(State(inner): State<Shared>, req: Request) -> ApiResult {
    let len = content_length(req.headers()).ok_or(Error::Malformed("content-length required"))?;
    let (client, _) = authorize(&inner, &req, len, Rule::Challenge)?;
    let signed: SignedObject = read_json(req.into_body()).await?;
    let identity = inner.member_identity(&client).ok_or(Error::NotMember)?;
    let receipt: Receipt = signed.verify(DOMAIN_RECEIPT, &identity)?;
    if receipt.sender != inner.me() || receipt.recipient != client {
        return Err(Error::BadSignature.into());
    }
    super::sender::complete(&inner, &receipt.transfer_id, &client, &signed)?;
    Ok(json_ok(&serde_json::json!({})))
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct GroupMessage {
    #[serde(default)]
    pub versions: Vec<SignedObject>,
    #[serde(default)]
    pub records: Vec<SignedObject>,
}

async fn get_group(State(inner): State<Shared>, req: Request) -> ApiResult {
    let (client, _) = authorize(&inner, &req, 0, Rule::ChallengeFormer)?;
    let g = read(&inner.group);
    // Former members get the versions only (to learn they were removed), not the members'
    // addresses and push tokens (D22).
    let records = if g.is_member(&client) {
        g.records_signed()
    } else {
        Vec::new()
    };
    Ok(json_ok(&GroupMessage {
        versions: g.chain_signed(),
        records,
    }))
}

/// No request authentication: every version is verified through the chain signatures and every
/// record through its member's signature, so the content authenticates itself. This lets a new
/// member introduce itself to a member that has not seen the version adding it yet (D21).
async fn post_group(State(inner): State<Shared>, req: Request) -> ApiResult {
    content_length(req.headers()).ok_or(Error::Malformed("content-length required"))?;
    let msg: GroupMessage = read_json(req.into_body()).await?;
    groupsync::apply(&inner, &msg)?;
    let gv = read(&inner.group).version();
    inner.emit(Event::DevicesChanged);
    Ok(json_ok(&serde_json::json!({ "group_version": gv })))
}

//! HTTP client side of the endpoints (protocol.md, "Transport and endpoints").

use std::path::Path;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::auth::{ProbeRequest, ProbeResponse, RequestAuth, RequestTarget};
use crate::signed::SignedObject;
use crate::wire::{b64_array, b64_encode};
use crate::{DeviceId, DeviceKeys, Error, Result, TransferId};

use super::now;

pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        // Also bounds outbox downloads and upload responses from a stalled peer.
        .read_timeout(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .expect("http client")
}

fn url(addr: &str, path: &str) -> String {
    format!("http://{addr}{path}")
}

fn net(e: reqwest::Error) -> Error {
    Error::Network(e.to_string())
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorInner,
}

#[derive(Deserialize)]
struct ErrorInner {
    code: String,
    #[serde(default)]
    message: String,
}

/// Turns a non-success response into [`Error::Remote`] with the peer's error code.
async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = limited_bytes(resp).await.unwrap_or_default();
    Err(match serde_json::from_slice::<ErrorBody>(&body) {
        Ok(b) => Error::from_remote(&b.error.code, &b.error.message),
        Err(_) if status == StatusCode::TOO_MANY_REQUESTS => Error::RateLimited,
        Err(_) => Error::Network(format!("HTTP {status}")),
    })
}

/// Peers' JSON answers are small; anything on the LAN may answer a scan probe, so cap the size.
const MAX_JSON_RESPONSE: usize = 256 * 1024;

async fn limited_bytes(mut resp: reqwest::Response) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(net)? {
        if out.len() + chunk.len() > MAX_JSON_RESPONSE {
            return Err(Error::Malformed("response too large"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

async fn json<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    let bytes = limited_bytes(check(resp).await?).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// `POST /v1/probe`; verifies the listener's signature over a fresh nonce.
pub(crate) async fn probe(
    http: &reqwest::Client,
    addr: &str,
    timeout: Duration,
) -> Result<ProbeResponse> {
    let nonce = crate::random::<32>();
    let resp = http
        .post(url(addr, "/v1/probe"))
        .timeout(timeout)
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&ProbeRequest { nonce })?)
        .send()
        .await
        .map_err(net)?;
    let r: ProbeResponse = json(resp).await?;
    if r.v != crate::wire::PROTOCOL_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    r.verify(&nonce)?;
    Ok(r)
}

#[derive(Deserialize)]
struct ChallengeResponse {
    #[serde(with = "b64_array")]
    nonce: [u8; 32],
}

/// An authenticated conversation with one listener: one challenge, increasing `seq`.
pub(crate) struct Session<'a> {
    http: &'a reqwest::Client,
    keys: &'a DeviceKeys,
    pub addr: String,
    server: DeviceId,
    nonce: [u8; 32],
    seq: u64,
}

impl<'a> Session<'a> {
    pub async fn open(
        http: &'a reqwest::Client,
        keys: &'a DeviceKeys,
        addr: &str,
        server: DeviceId,
    ) -> Result<Self> {
        let resp = http
            .post(url(addr, "/v1/challenge"))
            .timeout(CONTROL_TIMEOUT)
            .header(CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .await
            .map_err(net)?;
        let c: ChallengeResponse = json(resp).await?;
        Ok(Self {
            http,
            keys,
            addr: addr.to_owned(),
            server,
            nonce: c.nonce,
            seq: 0,
        })
    }

    fn request(&mut self, method: Method, path: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
        self.seq += 1;
        let target = RequestTarget {
            server: self.server,
            method: method.as_str(),
            path,
            content_length: body.len() as u64,
        };
        let auth = RequestAuth::sign(self.keys, &target, now(), &self.nonce, self.seq);
        let mut req = self
            .http
            .request(method, url(&self.addr, path))
            .header(AUTHORIZATION, auth.to_header());
        if !body.is_empty() {
            req = req
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_LENGTH, body.len())
                .body(body);
        }
        req
    }

    pub async fn get_json<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        let resp = self
            .request(Method::GET, path, Vec::new())
            .timeout(CONTROL_TIMEOUT)
            .send()
            .await
            .map_err(net)?;
        json(resp).await
    }

    pub async fn post_json<T: DeserializeOwned>(
        &mut self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<T> {
        let body = serde_json::to_vec(body)?;
        let resp = self
            .request(Method::POST, path, body)
            .timeout(CONTROL_TIMEOUT)
            .send()
            .await
            .map_err(net)?;
        json(resp).await
    }

    /// Streaming GET (outbox download). The caller reads the body.
    pub async fn get_stream(&mut self, path: &str) -> Result<reqwest::Response> {
        let resp = self
            .request(Method::GET, path, Vec::new())
            .send()
            .await
            .map_err(net)?;
        check(resp).await
    }
}

#[derive(Deserialize)]
pub(crate) struct ReceiptResponse {
    pub receipt: SignedObject,
}

/// `POST /v1/inbox`: streams the envelope file with a signature bound to the listener, the
/// transfer id and the exact length (protocol.md §7.2, upload rule).
pub(crate) async fn upload(
    http: &reqwest::Client,
    keys: &DeviceKeys,
    addr: &str,
    server: DeviceId,
    transfer_id: &TransferId,
    envelope: &Path,
    mut on_sent: impl FnMut(u64) + Send + 'static,
) -> Result<SignedObject> {
    use futures_util::StreamExt;
    let file = tokio::fs::File::open(envelope).await?;
    let len = file.metadata().await?.len();
    let auth = upload_authorization(keys, server, transfer_id, len, now());
    // `on_sent` sees each chunk as it is handed to the connection (upload progress).
    let stream = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024).map(move |chunk| {
        if let Ok(b) = &chunk {
            on_sent(b.len() as u64);
        }
        chunk
    });
    let body = reqwest::Body::wrap_stream(stream);
    let resp = http
        .post(url(addr, "/v1/inbox"))
        .header(AUTHORIZATION, auth)
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(CONTENT_LENGTH, len)
        .body(body)
        .send()
        .await
        .map_err(net)?;
    let r: ReceiptResponse = json(resp).await?;
    Ok(r.receipt)
}

/// `Authorization` header value for an upload. Also used by the iOS share extension, whose
/// background session sends the request itself.
pub fn upload_authorization(
    keys: &DeviceKeys,
    server: DeviceId,
    transfer_id: &TransferId,
    content_length: u64,
    ts: u64,
) -> String {
    let target = RequestTarget {
        server,
        method: "POST",
        path: "/v1/inbox",
        content_length,
    };
    RequestAuth::sign(keys, &target, ts, transfer_id, 0).to_header()
}

pub(crate) fn receipt_path(tid: &TransferId) -> String {
    format!("/v1/receipts/{}", b64_encode(tid))
}

pub(crate) fn outbox_item_path(tid: &TransferId) -> String {
    format!("/v1/outbox/{}", b64_encode(tid))
}

/// Unauthenticated JSON POST (pairing endpoints, which authenticate through the PSK).
pub(crate) async fn post_plain<T: DeserializeOwned>(
    http: &reqwest::Client,
    addr: &str,
    path: &str,
    body: &impl serde::Serialize,
) -> Result<T> {
    let resp = http
        .post(url(addr, path))
        .timeout(CONTROL_TIMEOUT)
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(body)?)
        .send()
        .await
        .map_err(net)?;
    json(resp).await
}

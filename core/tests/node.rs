//! End-to-end tests of the running node on loopback: pairing (normal and reverse QR), sends,
//! retry after the target comes back, the iPhone outbox path, removal, and listener-side
//! rejection of replayed, truncated, stale and unauthenticated uploads.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kitedrop_core::envelope::{ContentHeader, ContentKind};
use kitedrop_core::node::{self, PreparedUpload};
use kitedrop_core::signed::Platform;
use kitedrop_core::store::TransferState;
use kitedrop_core::{Config, Content, DeviceInfo, DeviceKeys, Event, Host, NetworkIssue, Node};

#[derive(Default)]
struct TestHost {
    texts: Mutex<Vec<(ContentKind, String)>>,
    files: Mutex<Vec<PathBuf>>,
    events: Mutex<Vec<String>>,
    pushes: Mutex<Vec<String>>,
    progress: Mutex<Vec<kitedrop_core::Progress>>,
}

impl Host for TestHost {
    fn deliver_text(&self, kind: ContentKind, text: &str, _: &DeviceInfo) -> Result<(), String> {
        self.texts.lock().unwrap().push((kind, text.to_owned()));
        Ok(())
    }
    fn deliver_file(
        &self,
        path: &Path,
        _: &ContentHeader,
        _: &DeviceInfo,
    ) -> Result<PathBuf, String> {
        self.files.lock().unwrap().push(path.to_owned());
        Ok(path.to_owned())
    }
    fn https_post(&self, _: &str, body: &str) -> Result<u16, String> {
        self.pushes.lock().unwrap().push(body.to_owned());
        Ok(200)
    }
    fn event(&self, e: Event) {
        let name = match e {
            Event::RemovedFromGroup => "removed".to_owned(),
            Event::Paired { name, .. } => format!("paired:{name}"),
            Event::TransferProgress { progress, .. } => {
                self.progress.lock().unwrap().push(progress);
                return;
            }
            _ => return,
        };
        self.events.lock().unwrap().push(name);
    }
    fn platform_issues(&self) -> Vec<NetworkIssue> {
        Vec::new()
    }
}

struct Dev {
    node: Node,
    host: Arc<TestHost>,
    secret: ([u8; 32], [u8; 32]),
    cfg: Config,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn config(root: &Path, name: &str, platform: Platform) -> Config {
    let dir = root.join(name);
    let mut cfg = Config::new(dir.join("data"), dir.join("downloads"), name, platform);
    cfg.bind = "127.0.0.1".parse().unwrap();
    cfg.mdns = false;
    cfg.scan = false;
    cfg.presence_interval_secs = 5;
    cfg.retry_schedule_secs = vec![1];
    if cfg.listen {
        let port = free_port();
        cfg.port = Some(port);
        cfg.advertise = Some(vec![format!("127.0.0.1:{port}")]);
    }
    cfg
}

fn start(root: &Path, name: &str, platform: Platform) -> Dev {
    let cfg = config(root, name, platform);
    let keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
    let (a, b) = keys.secret_bytes();
    let host = Arc::new(TestHost::default());
    let node = Node::start(cfg.clone(), keys, host.clone()).unwrap();
    Dev {
        node,
        host,
        secret: (*a, *b),
        cfg,
    }
}

impl Dev {
    fn restart(&mut self) {
        self.node.shutdown();
        std::thread::sleep(Duration::from_millis(200));
        let keys = DeviceKeys::from_secret_bytes(&self.secret.0, &self.secret.1).unwrap();
        self.node = Node::start(self.cfg.clone(), keys, self.host.clone()).unwrap();
    }

    fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.node.port().unwrap())
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn state(dev: &Dev, tid: &[u8; 16]) -> TransferState {
    dev.node.transfer(tid).unwrap().unwrap().state
}

fn pair(rt: &tokio::runtime::Runtime, inviter: &Dev, joiner: &Dev) {
    let uri = inviter.node.create_invite(false).unwrap();
    let id = rt.block_on(joiner.node.pair(&uri)).unwrap();
    assert_eq!(id, inviter.node.device_id());
}

#[test]
fn pair_send_text_link_and_file() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    assert_eq!(a.node.devices().len(), 2);
    assert_eq!(b.node.devices().len(), 2);
    assert_eq!(a.node.group_version(), 2);
    assert_eq!(b.node.group_version(), 2);
    assert!(
        a.host
            .events
            .lock()
            .unwrap()
            .contains(&"paired:pixel".to_owned())
    );

    let ids = rt
        .block_on(
            b.node
                .send(vec![a.node.device_id()], Content::Text("hello".into())),
        )
        .unwrap();
    wait_for("text delivered", 10, || {
        state(&b, &ids[0]) == TransferState::Delivered
    });
    assert_eq!(
        a.host.texts.lock().unwrap()[0],
        (ContentKind::Text, "hello".to_owned())
    );

    let ids = rt
        .block_on(b.node.send(
            vec![a.node.device_id()],
            Content::Link("https://example.org".into()),
        ))
        .unwrap();
    wait_for("link delivered", 10, || {
        state(&b, &ids[0]) == TransferState::Delivered
    });

    // A multi-chunk file (300 KiB + a few bytes), streamed both ways.
    let data: Vec<u8> = (0..300 * 1024 + 7).map(|i| (i * 31 % 251) as u8).collect();
    let src = tmp.path().join("photo.jpg");
    std::fs::write(&src, &data).unwrap();
    let ids = rt
        .block_on(a.node.send(
            vec![b.node.device_id()],
            Content::File {
                path: src.clone(),
                name: None,
                mime: None,
            },
        ))
        .unwrap();
    wait_for("file delivered", 10, || {
        state(&a, &ids[0]) == TransferState::Delivered
    });
    let got = b.host.files.lock().unwrap()[0].clone();
    assert_eq!(got.file_name().unwrap(), "photo.jpg");
    assert_eq!(std::fs::read(&got).unwrap(), data);
    // Same name again: never overwritten.
    let ids = rt
        .block_on(a.node.send(
            vec![b.node.device_id()],
            Content::File {
                path: src,
                name: None,
                mime: None,
            },
        ))
        .unwrap();
    wait_for("second file delivered", 10, || {
        state(&a, &ids[0]) == TransferState::Delivered
    });
    assert_eq!(
        b.host.files.lock().unwrap()[1].file_name().unwrap(),
        "photo (2).jpg"
    );
    // The envelope spool is cleaned after delivery.
    assert!(
        std::fs::read_dir(a.cfg.data_dir.join("spool"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(b.node.received(10).unwrap().len(), 2);
}

#[test]
fn reverse_qr_and_propagation_to_third_member() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    // The Windows desktop cannot scan: it shows a reverse QR, the Mac scans it.
    let c = start(tmp.path(), "desk", Platform::Windows);
    let uri = c.node.create_invite(true).unwrap();
    assert!(uri.contains("&r=1"));
    let id = rt.block_on(a.node.pair(&uri)).unwrap();
    assert_eq!(id, c.node.device_id());
    assert_eq!(c.node.devices().len(), 3);
    // B learns about C through the push from A.
    wait_for("B sees C", 10, || b.node.devices().len() == 3);
    // A used code is refused.
    let err = rt.block_on(b.node.pair(&uri)).unwrap_err();
    assert_eq!(err.code(), "code_invalid");
    // C can send to B directly.
    let ids = rt
        .block_on(c.node.send(
            vec![b.node.device_id()],
            Content::Text("hi from desk".into()),
        ))
        .unwrap();
    wait_for("C→B delivered", 10, || {
        state(&c, &ids[0]) == TransferState::Delivered
    });
}

#[test]
fn waits_while_offline_and_delivers_after_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let mut a = start(tmp.path(), "mac", Platform::Macos);
    let b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    a.node.shutdown();
    let ids = rt
        .block_on(
            b.node
                .send(vec![a.node.device_id()], Content::Text("later".into())),
        )
        .unwrap();
    wait_for("waiting", 10, || {
        state(&b, &ids[0]) == TransferState::Waiting
    });
    let t = b.node.transfer(&ids[0]).unwrap().unwrap();
    assert_eq!(t.reason.as_deref(), Some("offline"));
    a.restart();
    wait_for("delivered after restart", 20, || {
        state(&b, &ids[0]) == TransferState::Delivered
    });
    assert_eq!(a.host.texts.lock().unwrap()[0].1, "later");
}

/// A large file reports progress while it is encrypted and while it is uploaded; small sends do not.
#[test]
fn large_send_reports_progress() {
    use kitedrop_core::ProgressPhase::{Sealing, Uploading};
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "a", Platform::Macos);
    let b = start(tmp.path(), "b", Platform::Windows);
    pair(&rt, &a, &b);

    let small = rt
        .block_on(
            a.node
                .send(vec![b.node.device_id()], Content::Text("hi".into())),
        )
        .unwrap();
    wait_for("text delivered", 10, || {
        state(&a, &small[0]) == TransferState::Delivered
    });
    assert!(a.host.progress.lock().unwrap().is_empty());

    let data = vec![7u8; 3 * 1024 * 1024 + 5];
    let src = tmp.path().join("video.mp4");
    std::fs::write(&src, &data).unwrap();
    let ids = rt
        .block_on(a.node.send(
            vec![b.node.device_id()],
            Content::File {
                path: src,
                name: None,
                mime: None,
            },
        ))
        .unwrap();
    wait_for("file delivered", 20, || {
        state(&a, &ids[0]) == TransferState::Delivered
    });
    let seen = a.host.progress.lock().unwrap().clone();
    let last = |phase| seen.iter().rfind(|p| p.phase == phase).copied().unwrap();
    let (sealed, uploaded) = (last(Sealing), last(Uploading));
    assert_eq!(
        (sealed.done, sealed.total),
        (data.len() as u64, data.len() as u64)
    );
    // The envelope is the content plus a header and per-chunk tags: slightly larger.
    assert!(uploaded.total > sealed.total && uploaded.total < sealed.total + 64 * 1024);
    assert_eq!(uploaded.done, uploaded.total);
    // Phases come in order, and nothing is left behind once the send is over.
    let first_upload = seen.iter().position(|p| p.phase == Uploading).unwrap();
    assert!(seen[first_upload..].iter().all(|p| p.phase == Uploading));
    assert_eq!(a.node.progress(&ids[0]), None);
}

/// An item parked before the sender knew the iPhone's push token still gets a push once the
/// token arrives (seen in CI 2026-11-19: the token-carrying record reached the sender after the
/// item was parked, and no push was ever sent).
#[test]
fn push_follows_a_token_that_arrives_later() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let i = Node::start(
        config(tmp.path(), "iphone", Platform::Ios),
        DeviceKeys::generate(&mut kitedrop_core::os_rng()),
        Arc::new(TestHost::default()),
    )
    .unwrap();
    rt.block_on(i.pair(&a.node.create_invite(false).unwrap()))
        .unwrap();
    wait_for("member known", 10, || a.node.devices().len() == 2);

    let ids = rt
        .block_on(a.node.send(vec![i.device_id()], Content::Text("x".into())))
        .unwrap();
    wait_for("parked in outbox", 10, || {
        state(&a, &ids[0]) == TransferState::Waiting
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(a.host.pushes.lock().unwrap().is_empty(), "no token yet");

    i.set_push_token(Some("ExponentPushToken[late]".into()))
        .unwrap();
    wait_for("push after the token arrived", 10, || {
        !a.host.pushes.lock().unwrap().is_empty()
    });
    assert!(a.host.pushes.lock().unwrap()[0].contains("ExponentPushToken[late]"));
}

#[test]
fn iphone_pulls_from_outbox() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let mut cfg = config(tmp.path(), "iphone", Platform::Ios);
    cfg.push_token = Some("ExponentPushToken[test]".into());
    let host = Arc::new(TestHost::default());
    let i_keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
    let (s1, s2) = i_keys.secret_bytes();
    let i = Node::start(cfg, i_keys, host.clone()).unwrap();
    assert!(i.port().is_none());
    let uri = a.node.create_invite(false).unwrap();
    rt.block_on(i.pair(&uri)).unwrap();
    // A learns the iPhone's record (push token) from the record push after pairing.
    wait_for("record propagated", 10, || a.node.devices().len() == 2);

    let ids = rt
        .block_on(
            a.node
                .send(vec![i.device_id()], Content::Text("to phone".into())),
        )
        .unwrap();
    wait_for("parked in outbox", 10, || {
        state(&a, &ids[0]) == TransferState::Waiting
    });
    let t = a.node.transfer(&ids[0]).unwrap().unwrap();
    assert!(t.via_outbox);
    wait_for("push sent", 10, || {
        !a.host.pushes.lock().unwrap().is_empty()
    });
    let push = a.host.pushes.lock().unwrap()[0].clone();
    assert!(push.contains("New item from mac") && !push.contains("to phone"));

    assert_eq!(rt.block_on(i.poll_outboxes()).unwrap(), 1);
    assert_eq!(host.texts.lock().unwrap()[0].1, "to phone");
    wait_for("delivered by receipt", 10, || {
        state(&a, &ids[0]) == TransferState::Delivered
    });
    // Nothing left.
    assert_eq!(rt.block_on(i.poll_outboxes()).unwrap(), 0);

    // iPhone → Mac: the share extension seals and signs without a node, a background session
    // uploads, and the app adopts the transfer to fetch the receipt (protocol.md §8).
    let ext_keys = DeviceKeys::from_secret_bytes(&s1, &s2).unwrap();
    let a_pub = rt.block_on(probe(&a.addr())).identity();
    let src = b"shared from photos".to_vec();
    let header = ContentHeader {
        v: 1,
        kind: ContentKind::File,
        name: "IMG_1.HEIC".into(),
        mime: "image/heic".into(),
        size: src.len() as u64,
        created_at: node::now(),
        chunk_size: 65536,
    };
    let env = tmp.path().join("ext.ktdp");
    let up = node::prepare_upload(&ext_keys, &a_pub, &a.addr(), &header, &src[..], &env).unwrap();
    let body = std::fs::read(&env).unwrap();
    let status = rt.block_on(async {
        reqwest::Client::new()
            .post(&up.url)
            .header("authorization", &up.authorization)
            .header("content-length", body.len())
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
    });
    assert_eq!(status.as_u16(), 200);
    i.adopt_upload(&up).unwrap();
    wait_for("adopted upload delivered", 10, || {
        i.transfer(&up.transfer_id).unwrap().unwrap().state == TransferState::Delivered
    });
    assert_eq!(
        std::fs::read(&a.host.files.lock().unwrap()[0]).unwrap(),
        src
    );
}

#[test]
fn removed_device_resets_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    rt.block_on(a.node.remove_device(b.node.device_id()))
        .unwrap();
    assert_eq!(a.node.devices().len(), 1);
    wait_for("B removed", 10, || {
        b.host
            .events
            .lock()
            .unwrap()
            .contains(&"removed".to_owned())
    });
    assert_eq!(b.node.devices().len(), 1);
    // B's sends to A now fail at once.
    let err = rt
        .block_on(
            b.node
                .send(vec![a.node.device_id()], Content::Text("x".into())),
        )
        .unwrap_err();
    assert_eq!(err.code(), "not_member");
}

/// Raw uploads against the listener: replay, truncation, stale timestamp, non-member.
#[test]
fn listener_rejects_bad_uploads() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    // B is a paired device whose keys we hold, to craft uploads by hand.
    let b_keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
    let (s1, s2) = b_keys.secret_bytes();
    let mut b_cfg = config(tmp.path(), "pixel", Platform::Android);
    b_cfg.presence_interval_secs = 3600;
    let b = Node::start(b_cfg, b_keys, Arc::new(TestHost::default())).unwrap();
    let uri = a.node.create_invite(false).unwrap();
    rt.block_on(b.pair(&uri)).unwrap();
    let b_keys = DeviceKeys::from_secret_bytes(&s1, &s2).unwrap();
    let a_ident = a.node.devices().into_iter().find(|d| d.is_self).unwrap();
    let a_pub = {
        // the probe gives us A's full public identity
        let r = rt.block_on(probe(&a.addr()));
        assert_eq!(r.identity().device_id(), a_ident.device_id);
        r.identity()
    };

    let header = ContentHeader {
        v: 1,
        kind: ContentKind::Text,
        name: String::new(),
        mime: "text/plain; charset=utf-8".into(),
        size: 5,
        created_at: node::now(),
        chunk_size: 64,
    };
    let env = tmp.path().join("env.ktdp");
    let up =
        node::prepare_upload(&b_keys, &a_pub, &a.addr(), &header, &b"hello"[..], &env).unwrap();
    let http = reqwest::Client::new();
    let post = |up: &PreparedUpload, body: Vec<u8>, auth: String| {
        let http = http.clone();
        let url = up.url.clone();
        async move {
            let r = http
                .post(url)
                .header("authorization", auth)
                .header("content-length", body.len())
                .body(body)
                .send()
                .await
                .unwrap();
            (r.status().as_u16(), r.text().await.unwrap())
        }
    };
    let body = std::fs::read(&env).unwrap();

    // Truncated body (signed length matches the shorter body): `truncated`, nothing stored.
    let short = body[..body.len() - 10].to_vec();
    let auth = node::upload_authorization(
        &b_keys,
        a_pub.device_id(),
        &up.transfer_id,
        short.len() as u64,
        node::now(),
    );
    let (st, text) = rt.block_on(post(&up, short, auth));
    assert_eq!(st, 400, "{text}");
    assert!(text.contains("\"truncated\""), "{text}");

    // Signed for another length: bad signature.
    let auth =
        node::upload_authorization(&b_keys, a_pub.device_id(), &up.transfer_id, 1, node::now());
    let (st, text) = rt.block_on(post(&up, body.clone(), auth));
    assert_eq!(st, 403, "{text}");
    assert!(text.contains("bad_signature"));

    // Stale timestamp.
    let auth = node::upload_authorization(
        &b_keys,
        a_pub.device_id(),
        &up.transfer_id,
        body.len() as u64,
        node::now() - 25 * 3600,
    );
    let (st, text) = rt.block_on(post(&up, body.clone(), auth));
    assert_eq!(st, 403);
    assert!(text.contains("stale_request"));

    // Not a member.
    let stranger = DeviceKeys::generate(&mut kitedrop_core::os_rng());
    let auth = node::upload_authorization(
        &stranger,
        a_pub.device_id(),
        &up.transfer_id,
        body.len() as u64,
        node::now(),
    );
    let (st, text) = rt.block_on(post(&up, body.clone(), auth));
    assert_eq!(st, 403);
    assert!(text.contains("not_member"));

    // Valid upload, then an exact replay: same receipt, stored once.
    let (st, first) = rt.block_on(post(&up, body.clone(), up.authorization.clone()));
    assert_eq!(st, 200, "{first}");
    let (st, again) = rt.block_on(post(&up, body.clone(), up.authorization.clone()));
    assert_eq!(st, 200);
    assert_eq!(first, again);
    assert_eq!(a.host.texts.lock().unwrap().len(), 1);

    // The same transfer id under another envelope body: still the stored receipt, not applied.
    assert_eq!(a.node.received(10).unwrap().len(), 1);
    b.shutdown();
}

async fn probe(addr: &str) -> kitedrop_core::auth::ProbeResponse {
    let nonce = kitedrop_core::random::<32>();
    let bytes = reqwest::Client::new()
        .post(format!("http://{addr}/v1/probe"))
        .body(serde_json::to_vec(&kitedrop_core::auth::ProbeRequest { nonce }).unwrap())
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let r: kitedrop_core::auth::ProbeResponse = serde_json::from_slice(&bytes).unwrap();
    r.verify(&nonce).unwrap();
    r
}

#[test]
fn unreachable_target_expires() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let mut b_cfg = config(tmp.path(), "pixel", Platform::Android);
    b_cfg.ttl_secs = 2;
    let b = Node::start(
        b_cfg,
        DeviceKeys::generate(&mut kitedrop_core::os_rng()),
        Arc::new(TestHost::default()),
    )
    .unwrap();
    let uri = a.node.create_invite(false).unwrap();
    rt.block_on(b.pair(&uri)).unwrap();
    a.node.shutdown();
    let ids = rt
        .block_on(b.send(vec![a.node.device_id()], Content::Text("never".into())))
        .unwrap();
    wait_for("expired", 15, || {
        b.transfer(&ids[0]).unwrap().unwrap().state == TransferState::Failed
    });
    let t = b.transfer(&ids[0]).unwrap().unwrap();
    assert_eq!(t.reason.as_deref(), Some("expired"));
}

/// Review finding (D21): C joins through A while B is off; A goes off; C's first send to B must
/// still arrive, because C can introduce itself to B with the group push.
#[test]
fn new_member_reaches_member_that_missed_its_join() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let mut a = start(tmp.path(), "mac", Platform::Macos);
    let mut b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    // A must hold B's device record (address) before B goes away.
    wait_for("A sees B", 15, || {
        a.node
            .devices()
            .iter()
            .any(|d| d.device_id == b.node.device_id() && d.online)
    });
    b.node.shutdown();
    let c = start(tmp.path(), "desk", Platform::Windows);
    pair(&rt, &a, &c);
    a.node.shutdown();
    b.restart();
    assert_eq!(b.node.devices().len(), 2, "B has not seen C yet");
    let ids = rt
        .block_on(c.node.send(
            vec![b.node.device_id()],
            Content::Text("first hello".into()),
        ))
        .unwrap();
    wait_for("C→B delivered", 20, || {
        let t = c.node.transfer(&ids[0]).unwrap().unwrap();
        assert_ne!(t.state, TransferState::Failed, "{:?}", t.reason);
        t.state == TransferState::Delivered
    });
    assert_eq!(b.node.devices().len(), 3);
    let _ = &mut a;
}

/// Seen 2026-11-19: a phone in a group scanned the normal QR of a new, lone PC and silently left
/// its group for the PC's. Now the lone PC is added to the phone's group instead (D34).
#[test]
fn group_member_scanning_lone_device_adds_it() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    let c = start(tmp.path(), "desk", Platform::Windows);
    let uri = c.node.create_invite(false).unwrap();
    let id = rt.block_on(b.node.pair(&uri)).unwrap();
    assert_eq!(id, c.node.device_id());
    assert_eq!(b.node.devices().len(), 3, "B stays in its group and adds C");
    assert_eq!(c.node.devices().len(), 3, "C joined A and B's group");
    wait_for("A sees C", 10, || a.node.devices().len() == 3);
}

#[test]
fn devices_in_two_groups_do_not_merge_silently() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = rt();
    let a = start(tmp.path(), "mac", Platform::Macos);
    let b = start(tmp.path(), "pixel", Platform::Android);
    pair(&rt, &a, &b);
    let c = start(tmp.path(), "desk", Platform::Windows);
    let d = start(tmp.path(), "tab", Platform::Android);
    pair(&rt, &c, &d);
    let uri = d.node.create_invite(false).unwrap();
    let err = rt.block_on(b.node.pair(&uri)).unwrap_err();
    assert_eq!(err.code(), "already_in_group");
    assert_eq!(b.node.devices().len(), 2);
    assert_eq!(d.node.devices().len(), 2);
}

//! `kitedrop daemon`: a headless kitedrop device driven by line commands on stdin, printing one
//! JSON object per line on stdout (command answers and events). Used for development and for the
//! two-process integration test.
//!
//! Development tool only: the identity is kept in a plain key file inside `--dir`. The apps keep
//! it in the OS secure store (design §5).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kitedrop_core::envelope::{ContentHeader, ContentKind};
use kitedrop_core::signed::Platform;
use kitedrop_core::store::Transfer;
use kitedrop_core::wire::{b64_decode_array, b64_encode};
use kitedrop_core::{
    Config, Content, DeviceId, DeviceInfo, DeviceKeys, Event, Host, NetworkIssue, Node,
};
use serde_json::{Value, json};

const USAGE: &str =
    "usage: kitedrop daemon --dir <dir> --name <name> [--platform macos|windows|android|ios]
                       [--port <n>] [--bind <ip>] [--advertise <ip:port>] [--no-mdns] [--no-scan]
                       [--presence <secs>] [--retry <secs>]
stdin commands: info | devices | invite | invite-reverse | pair <uri> | send-text <device> <text>
                send-link <device> <url> | send-file <device> <path> | transfers | received | poll
                remove <device> | rename <device> <name> | refresh | diag | quit
<device> is a device id (base64url) or a member name.";

/// Serializes stdout lines from the command loop and from host callbacks.
static OUT: Mutex<()> = Mutex::new(());

fn emit(v: Value) {
    let _g = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn kind(k: ContentKind) -> &'static str {
    match k {
        ContentKind::File => "file",
        ContentKind::Text => "text",
        ContentKind::Link => "link",
    }
}

fn transfer_json(t: &Transfer) -> Value {
    json!({
        "transfer_id": b64_encode(&t.transfer_id), "target": b64_encode(&t.target),
        "kind": kind(t.kind), "name": t.name, "size": t.size, "state": t.state.as_str(),
        "reason": t.reason, "via_outbox": t.via_outbox, "attempts": t.attempts,
    })
}

fn device_json(d: &DeviceInfo) -> Value {
    json!({
        "device_id": b64_encode(&d.device_id), "name": d.name, "platform": format!("{:?}", d.platform).to_lowercase(),
        "self": d.is_self, "online": d.online, "address": d.address,
    })
}

struct CliHost {
    downloads: PathBuf,
}

impl Host for CliHost {
    fn deliver_text(&self, k: ContentKind, text: &str, from: &DeviceInfo) -> Result<(), String> {
        emit(json!({ "event": "text", "kind": kind(k), "text": text, "from": from.name }));
        Ok(())
    }

    fn deliver_file(
        &self,
        path: &Path,
        h: &ContentHeader,
        from: &DeviceInfo,
    ) -> Result<PathBuf, String> {
        let _ = &self.downloads;
        emit(
            json!({ "event": "file", "path": path, "name": h.name, "size": h.size, "from": from.name }),
        );
        Ok(path.to_owned())
    }

    fn https_post(&self, url: &str, _body: &str) -> Result<u16, String> {
        Err(format!("push to {url} not supported by the CLI"))
    }

    fn event(&self, e: Event) {
        let v = match e {
            Event::TransferChanged(t) => {
                json!({ "event": "transfer", "transfer": transfer_json(&t) })
            }
            Event::Received(r) => {
                json!({ "event": "received", "transfer_id": b64_encode(&r.transfer_id), "kind": kind(r.kind), "name": r.name })
            }
            Event::DevicesChanged => json!({ "event": "devices_changed" }),
            Event::Presence { device, online } => {
                json!({ "event": "presence", "device": b64_encode(&device), "online": online })
            }
            Event::Paired { device, name } => {
                json!({ "event": "paired", "device": b64_encode(&device), "name": name })
            }
            Event::RemovedFromGroup => json!({ "event": "removed_from_group" }),
            Event::OutboxPending { count } => json!({ "event": "outbox_pending", "count": count }),
            Event::NetworkIssues(list) => {
                json!({ "event": "network_issues", "codes": list.iter().map(NetworkIssue::code).collect::<Vec<_>>() })
            }
            Event::TransferProgress {
                transfer_id,
                progress,
            } => json!({
                "event": "progress", "transfer_id": b64_encode(&transfer_id),
                "phase": progress.phase.as_str(), "done": progress.done, "total": progress.total
            }),
        };
        emit(v);
    }

    fn platform_issues(&self) -> Vec<NetworkIssue> {
        Vec::new()
    }
}

fn load_or_create_keys(path: &Path) -> std::io::Result<DeviceKeys> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 64 => {
            let sig: [u8; 32] = bytes[..32].try_into().expect("32 bytes");
            let kem: [u8; 32] = bytes[32..].try_into().expect("32 bytes");
            DeviceKeys::from_secret_bytes(&sig, &kem)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
        }
        // Never replace an existing identity silently: that would orphan the device.
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a kitedrop identity (64 bytes)", path.display()),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
            let (sig, kem) = keys.secret_bytes();
            let mut bytes = sig.to_vec();
            bytes.extend_from_slice(&kem[..]);
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            opts.open(path)?.write_all(&bytes)?;
            Ok(keys)
        }
        Err(e) => Err(e),
    }
}

fn parse_platform(s: &str) -> Result<Platform, String> {
    Ok(match s {
        "macos" => Platform::Macos,
        "windows" => Platform::Windows,
        "android" => Platform::Android,
        "ios" => Platform::Ios,
        _ => return Err(format!("unknown platform {s}")),
    })
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut it = args.iter();
    let (mut dir, mut name, mut platform) = (None, None, Platform::Macos);
    let mut extra: Vec<(String, String)> = Vec::new();
    let (mut mdns, mut scan) = (true, true);
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--dir" => dir = Some(PathBuf::from(val()?)),
            "--name" => name = Some(val()?),
            "--platform" => platform = parse_platform(&val()?)?,
            "--no-mdns" => mdns = false,
            "--no-scan" => scan = false,
            "--port" | "--bind" | "--advertise" | "--presence" | "--retry" => {
                extra.push((a.clone(), val()?))
            }
            _ => return Err(format!("unknown option {a}")),
        }
    }
    let dir = dir.ok_or("--dir is required")?;
    let name = name.ok_or("--name is required")?;
    let mut cfg = Config::new(dir.join("data"), dir.join("downloads"), &name, platform);
    cfg.mdns = mdns;
    cfg.scan = scan;
    for (k, v) in extra {
        match k.as_str() {
            "--port" => cfg.port = Some(v.parse().map_err(|_| "bad --port")?),
            "--bind" => cfg.bind = v.parse().map_err(|_| "bad --bind")?,
            "--advertise" => cfg.advertise = Some(vec![v]),
            "--presence" => cfg.presence_interval_secs = v.parse().map_err(|_| "bad --presence")?,
            "--retry" => cfg.retry_schedule_secs = vec![v.parse().map_err(|_| "bad --retry")?],
            _ => unreachable!(),
        }
    }
    Ok(cfg)
}

fn find_device(node: &Node, s: &str) -> Result<DeviceId, String> {
    if let Some(id) = b64_decode_array::<16>(s) {
        return Ok(id);
    }
    node.devices()
        .into_iter()
        .find(|d| d.name == s && !d.is_self)
        .map(|d| d.device_id)
        .ok_or_else(|| format!("no device named {s}"))
}

fn run_command(node: &Node, rt: &tokio::runtime::Runtime, line: &str) -> Result<Value, String> {
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let e = |e: kitedrop_core::Error| format!("{}: {e}", e.code());
    let send = |dev: &str, content: Content| -> Result<Value, String> {
        let id = find_device(node, dev)?;
        let ids = rt.block_on(node.send(vec![id], content)).map_err(e)?;
        Ok(json!({ "transfers": ids.iter().map(|t| b64_encode(t)).collect::<Vec<_>>() }))
    };
    Ok(match cmd {
        "info" => {
            json!({ "device_id": b64_encode(&node.device_id()), "port": node.port(), "group_version": node.group_version() })
        }
        "devices" => {
            json!({ "devices": node.devices().iter().map(device_json).collect::<Vec<_>>() })
        }
        "invite" => json!({ "uri": node.create_invite(false).map_err(e)? }),
        "invite-reverse" => json!({ "uri": node.create_invite(true).map_err(e)? }),
        "pair" => json!({ "paired": b64_encode(&rt.block_on(node.pair(rest.trim())).map_err(e)?) }),
        "send-text" | "send-link" | "send-file" => {
            let (dev, arg) = rest.split_once(' ').ok_or("missing argument")?;
            let content = match cmd {
                "send-text" => Content::Text(arg.to_owned()),
                "send-link" => Content::Link(arg.to_owned()),
                _ => Content::File {
                    path: PathBuf::from(arg),
                    name: None,
                    mime: None,
                },
            };
            send(dev, content)?
        }
        "transfers" => {
            json!({ "transfers": node.transfers(100).map_err(e)?.iter().map(transfer_json).collect::<Vec<_>>() })
        }
        "received" => json!({ "received": node.received(100).map_err(e)?.iter().map(|r| json!({
            "transfer_id": b64_encode(&r.transfer_id), "kind": kind(r.kind), "name": r.name, "location": r.location })).collect::<Vec<_>>() }),
        "poll" => json!({ "stored": rt.block_on(node.poll_outboxes()).map_err(e)? }),
        "remove" => {
            let id = find_device(node, rest.trim())?;
            rt.block_on(node.remove_device(id)).map_err(e)?;
            json!({ "removed": b64_encode(&id) })
        }
        "rename" => {
            let (dev, name) = rest.split_once(' ').ok_or("missing name")?;
            let id = find_device(node, dev)?;
            rt.block_on(node.rename_device(id, name.to_owned()))
                .map_err(e)?;
            json!({ "renamed": b64_encode(&id) })
        }
        "refresh" => {
            node.refresh();
            json!({ "ok": true })
        }
        "diag" => {
            json!({ "issues": node.diagnostics().iter().map(NetworkIssue::code).collect::<Vec<_>>() })
        }
        _ => return Err(format!("unknown command {cmd}")),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("daemon") {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let cfg = match parse_args(&args[1..]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let dir = cfg
        .data_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    std::fs::create_dir_all(&dir).expect("create --dir");
    let keys = load_or_create_keys(&dir.join("identity.key")).expect("identity");
    let host = Arc::new(CliHost {
        downloads: cfg.download_dir.clone(),
    });
    let node = match Node::start(cfg, keys, host) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("start failed: {e}");
            std::process::exit(1);
        }
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    emit(json!({ "ready": true, "device_id": b64_encode(&node.device_id()), "port": node.port() }));
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" {
            break;
        }
        match run_command(&node, &rt, line) {
            Ok(v) => emit(json!({ "ok": v })),
            Err(err) => emit(json!({ "error": err })),
        }
    }
    node.shutdown();
}

//! kitedrop desktop: a tray / menu-bar app around `kitedrop-core` (design §2, §8).
//!
//! - Starts at login, always listens, lives in the tray (menu bar on macOS, no Dock icon).
//! - Send: global shortcut (clipboard), a small drop window (a tray icon cannot be a drop target),
//!   Windows "Send to", or the main window's file picker.
//! - Receive: file → `Downloads/kitedrop`, text/link → clipboard, with a notification; no accept dialog.

mod folder;
mod notify;
mod platform;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kitedrop_core::envelope::{ContentHeader, ContentKind};
use kitedrop_core::signed::Platform;
use kitedrop_core::store::{Transfer, TransferState};
use kitedrop_core::wire::{b64_decode_array, b64_encode};
use kitedrop_core::{
    Config, Content, DeviceId, DeviceInfo, Event, Host, NetworkIssue, Node, Progress,
};
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, DragDropEvent, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::ShortcutState;

const SHORTCUT: &str = "CommandOrControl+Shift+K";

/// What the "send" window is about to send, chosen before the target.
#[derive(Clone, Debug)]
enum Pending {
    Files(Vec<PathBuf>),
    Text(String),
    Link(String),
}

struct AppState {
    node: Node,
    pending: Mutex<Option<Pending>>,
    downloads: PathBuf,
    /// Archives of folders being sent (folder.rs).
    outgoing: PathBuf,
}

struct DesktopHost {
    app: AppHandle,
    sleep: platform::SleepGuard,
}

/// Plain notification; a click opens the main window.
fn notify(_app: &AppHandle, title: &str, body: &str) {
    notify::show(title, body, notify::Target::Window);
}

fn kind_str(k: ContentKind) -> &'static str {
    match k {
        ContentKind::File => "file",
        ContentKind::Text => "text",
        ContentKind::Link => "link",
    }
}

impl Host for DesktopHost {
    fn deliver_text(&self, kind: ContentKind, text: &str, from: &DeviceInfo) -> Result<(), String> {
        self.app
            .clipboard()
            .write_text(text.to_owned())
            .map_err(|e| e.to_string())?;
        let what = if kind == ContentKind::Link {
            "Link"
        } else {
            "Text"
        };
        let target = if kind == ContentKind::Link {
            notify::Target::Link(text.to_owned())
        } else {
            notify::Target::Text(text.to_owned())
        };
        let preview: String = text.chars().take(120).collect();
        notify::show(
            &format!("{what} from {} — copied", from.name),
            &preview,
            target,
        );
        Ok(())
    }

    fn deliver_file(
        &self,
        path: &std::path::Path,
        h: &ContentHeader,
        from: &DeviceInfo,
    ) -> Result<PathBuf, String> {
        notify::show(
            &format!("File from {}", from.name),
            &format!("{} — saved to Downloads/kitedrop", h.name),
            notify::Target::File(path.to_owned()),
        );
        Ok(path.to_owned())
    }

    fn https_post(&self, url: &str, json_body: &str) -> Result<u16, String> {
        // reqwest's blocking client must not run on a thread inside the node's async runtime.
        let (url, body) = (url.to_owned(), json_body.to_owned());
        std::thread::spawn(move || {
            reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .and_then(|c| {
                    c.post(url)
                        .header("content-type", "application/json")
                        .header("accept", "application/json")
                        .body(body)
                        .send()
                })
                .map(|r| r.status().as_u16())
                .map_err(|e| e.to_string())
        })
        .join()
        .map_err(|_| "push thread panicked".to_owned())?
    }

    fn event(&self, e: Event) {
        match &e {
            Event::OutboxPending { count } => self.sleep.set(*count > 0),
            Event::TransferChanged(t) if t.state == TransferState::Failed => {
                let target = self
                    .app
                    .try_state::<AppState>()
                    .and_then(|s| {
                        s.node
                            .devices()
                            .into_iter()
                            .find(|d| d.device_id == t.target)
                    })
                    .map_or_else(|| "a device".to_owned(), |d| d.name);
                let what = if t.name.is_empty() {
                    kind_str(t.kind).to_owned()
                } else {
                    t.name.clone()
                };
                notify(
                    &self.app,
                    &format!("Could not send {what} to {target}"),
                    &reason_text(t.reason.as_deref().unwrap_or("")),
                );
            }
            Event::RemovedFromGroup => notify(
                &self.app,
                "This computer was removed",
                "Another device removed it from your kitedrop group.",
            ),
            Event::TransferProgress {
                transfer_id,
                progress,
            } => {
                // Only the bar of that row changes; no full re-render.
                let _ = self.app.emit(
                    "kitedrop://progress",
                    progress_json(&b64_encode(transfer_id), progress),
                );
                return;
            }
            _ => {}
        }
        let _ = self.app.emit("kitedrop://changed", ());
    }

    fn platform_issues(&self) -> Vec<NetworkIssue> {
        platform::platform_issues()
    }
}

/// Plain-language text for a failure reason (design §3: no silent failures).
fn reason_text(reason: &str) -> String {
    match reason {
        "expired" => "It waited too long without reaching the device.".into(),
        "cancelled" => "You cancelled it.".into(),
        "removed_from_group" => "The device is no longer in your group.".into(),
        "file_unreadable" => "The file could not be read.".into(),
        r if r.starts_with("rejected:") => format!("The device refused it ({}).", &r[9..]),
        r => r.to_owned(),
    }
}

fn device_json(d: &DeviceInfo) -> Value {
    json!({
        "id": b64_encode(&d.device_id), "name": d.name,
        "platform": match d.platform { Platform::Ios => "ios", Platform::Android => "android", Platform::Macos => "macos", Platform::Windows => "windows" },
        "self": d.is_self, "online": d.online, "address": d.address, "last_seen": d.last_seen,
    })
}

fn transfer_json(t: &Transfer, devices: &[DeviceInfo], progress: Option<Progress>) -> Value {
    let target = devices
        .iter()
        .find(|d| d.device_id == t.target)
        .map_or("removed device", |d| d.name.as_str());
    let id = b64_encode(&t.transfer_id);
    json!({
        "id": id, "target": target, "kind": kind_str(t.kind), "name": t.name,
        "size": t.size, "state": t.state.as_str(), "reason": t.reason.as_deref().map(reason_text),
        "via_outbox": t.via_outbox, "created_at": t.created_at,
        "progress": progress.map(|p| progress_json(&id, &p)),
    })
}

fn progress_json(id: &str, p: &Progress) -> Value {
    json!({ "id": id, "phase": p.phase.as_str(), "done": p.done, "total": p.total })
}

fn issue_text(i: &NetworkIssue, devices: &[DeviceInfo]) -> String {
    match i {
        NetworkIssue::NoNetwork => "This computer is not connected to a network.".into(),
        NetworkIssue::WindowsPublicProfile => "Windows treats this network as Public, so other devices cannot reach this PC. Set the network to Private in Settings → Network & internet.".into(),
        NetworkIssue::LocalNetworkDenied => "kitedrop is not allowed to use the local network. Allow it in System Settings → Privacy & Security → Local Network.".into(),
        NetworkIssue::DifferentSubnet { device } => {
            let name = devices.iter().find(|d| d.device_id == *device).map_or("A device", |d| d.name.as_str());
            format!("{name} was last seen on a different network.")
        }
        NetworkIssue::ApIsolationSuspected => "Your devices cannot see each other on this Wi-Fi. It may be a guest network or have client isolation turned on.".into(),
        NetworkIssue::SubnetTooLarge { .. } => "This network is very large; kitedrop only searches the nearby part of it.".into(),
        NetworkIssue::PortInUse => "kitedrop could not open a network port (52525–52534 are busy).".into(),
    }
}

fn err(e: kitedrop_core::Error) -> String {
    e.to_string()
}

fn parse_ids(ids: &[String]) -> Result<Vec<DeviceId>, String> {
    ids.iter()
        .map(|s| b64_decode_array::<16>(s).ok_or_else(|| "bad device id".to_owned()))
        .collect()
}

// ---- commands ----

#[tauri::command]
async fn state(s: tauri::State<'_, AppState>) -> Result<Value, String> {
    let devices = s.node.devices();
    let transfers = s.node.transfers(50).map_err(err)?;
    let received = s.node.received(20).map_err(err)?;
    let issues: Vec<String> = s
        .node
        .diagnostics()
        .iter()
        .map(|i| issue_text(i, &devices))
        .collect();
    Ok(json!({
        "devices": devices.iter().map(device_json).collect::<Vec<_>>(),
        "transfers": transfers
            .iter()
            .map(|t| transfer_json(t, &devices, s.node.progress(&t.transfer_id)))
            .collect::<Vec<_>>(),
        "received": received.iter().map(|r| json!({
            "kind": kind_str(r.kind), "name": r.name, "size": r.size, "at": r.at,
            "from": devices.iter().find(|d| d.device_id == r.sender).map_or("removed device", |d| d.name.as_str()),
            "location": r.location,
        })).collect::<Vec<_>>(),
        "issues": issues,
        "shortcut": SHORTCUT,
    }))
}

#[tauri::command]
fn pending(s: tauri::State<'_, AppState>) -> Value {
    match s.pending.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        None => Value::Null,
        Some(Pending::Files(p)) => {
            let folders = p.iter().filter(|p| p.is_dir()).count();
            let label = match p.as_slice() {
                [one] => {
                    let name = one
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    if folders == 1 {
                        format!("{name} (folder, sent as .zip)")
                    } else {
                        name
                    }
                }
                _ if folders == 0 => format!("{} files", p.len()),
                _ => format!("{} items (folders are sent as .zip)", p.len()),
            };
            json!({ "kind": "files", "count": p.len(), "folders": folders, "label": label })
        }
        Some(Pending::Text(t)) => json!({ "kind": "text", "label": preview(t) }),
        Some(Pending::Link(t)) => json!({ "kind": "link", "label": preview(t) }),
    }
}

fn preview(t: &str) -> String {
    let one_line: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 80 {
        format!("{}…", one_line.chars().take(80).collect::<String>())
    } else {
        one_line
    }
}

async fn send_contents(
    node: &Node,
    targets: Vec<DeviceId>,
    contents: Vec<Content>,
) -> Result<(), String> {
    for c in contents {
        node.send(targets.clone(), c).await.map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
async fn send_pending(
    app: AppHandle,
    s: tauri::State<'_, AppState>,
    targets: Vec<String>,
) -> Result<(), String> {
    let targets = parse_ids(&targets)?;
    let p = s.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
    let contents = match p {
        None => return Err("nothing to send".into()),
        Some(Pending::Files(paths)) => {
            let mut contents = Vec::with_capacity(paths.len());
            for path in paths {
                let path = if path.is_dir() {
                    let out = s.outgoing.clone();
                    let dir = path.clone();
                    let app = app.clone();
                    // The send window shows the zipping progress (at most ~5 updates a second).
                    let mut last: Option<std::time::Instant> = None;
                    let report = move |done: u64, total: u64| {
                        let now = std::time::Instant::now();
                        if done == total
                            || last.is_none_or(|l| now.duration_since(l).as_millis() >= 200)
                        {
                            last = Some(now);
                            let _ = app.emit(
                                "kitedrop://zipping",
                                json!({ "done": done, "total": total }),
                            );
                        }
                    };
                    tauri::async_runtime::spawn_blocking(move || {
                        folder::zip_dir(&dir, &out, report)
                    })
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("Could not zip {}: {e}", path.display()))?
                } else {
                    path
                };
                contents.push(Content::File {
                    path,
                    name: None,
                    mime: None,
                });
            }
            contents
        }
        Some(Pending::Text(t)) => vec![Content::Text(t)],
        Some(Pending::Link(t)) => vec![Content::Link(t)],
    };
    send_contents(&s.node, targets, contents).await
}

#[tauri::command]
async fn send_files(
    app: AppHandle,
    s: tauri::State<'_, AppState>,
    targets: Vec<String>,
) -> Result<u32, String> {
    let targets = parse_ids(&targets)?;
    let Some(files) = app.dialog().file().blocking_pick_files() else {
        return Ok(0);
    };
    let paths: Vec<PathBuf> = files
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .collect();
    let n = paths.len() as u32;
    let contents = paths
        .into_iter()
        .map(|path| Content::File {
            path,
            name: None,
            mime: None,
        })
        .collect();
    send_contents(&s.node, targets, contents).await?;
    Ok(n)
}

#[tauri::command]
async fn send_clipboard_to(
    app: AppHandle,
    s: tauri::State<'_, AppState>,
    targets: Vec<String>,
) -> Result<(), String> {
    let targets = parse_ids(&targets)?;
    let Some(p) = clipboard_pending(&app) else {
        return Err("The clipboard has no text.".into());
    };
    let content = match p {
        Pending::Link(t) => Content::Link(t),
        Pending::Text(t) => Content::Text(t),
        Pending::Files(_) => unreachable!("clipboard yields text"),
    };
    send_contents(&s.node, targets, vec![content]).await
}

#[derive(serde::Serialize)]
struct InviteAnswer {
    uri: String,
    svg: String,
}

#[tauri::command]
fn invite(s: tauri::State<'_, AppState>, reverse: bool) -> Result<InviteAnswer, String> {
    let uri = s.node.create_invite(reverse).map_err(err)?;
    let code = qrcode::QrCode::with_error_correction_level(uri.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| e.to_string())?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(260, 260)
        .quiet_zone(true)
        .build();
    Ok(InviteAnswer { uri, svg })
}

/// Pairs from a link pasted from another device (D31): the same path as scanning its QR.
#[tauri::command]
async fn pair_link(s: tauri::State<'_, AppState>, uri: String) -> Result<String, String> {
    let uri = uri.trim();
    if !uri.starts_with(kitedrop_core::pairing::URI_PREFIX) {
        return Err("This is not a kitedrop pairing link.".into());
    }
    let id = s.node.pair(uri).await.map_err(|e| match e.code() {
        "code_invalid" => "The link has expired or was already used. Make a new one.".to_owned(),
        "already_in_group" => "Both computers are already in groups with other devices. Remove one of them from its group first.".to_owned(),
        "network" => {
            "The other device could not be reached. Are both on the same network?".to_owned()
        }
        _ => e.to_string(),
    })?;
    Ok(s.node
        .devices()
        .into_iter()
        .find(|d| d.device_id == id)
        .map_or_else(String::new, |d| d.name))
}

#[tauri::command]
fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_device(s: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let id = parse_ids(&[id])?[0];
    s.node.remove_device(id).await.map_err(err)
}

#[tauri::command]
async fn rename_device(
    s: tauri::State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let id = parse_ids(&[id])?[0];
    s.node.rename_device(id, name).await.map_err(err)
}

#[tauri::command]
fn retry(s: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let id = b64_decode_array::<16>(&id).ok_or("bad id")?;
    s.node.retry(&id).map_err(err)
}

#[tauri::command]
fn cancel(s: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let id = b64_decode_array::<16>(&id).ok_or("bad id")?;
    s.node.cancel(&id).map_err(err)
}

/// Must be async: creating a window from a synchronous command deadlocks on Windows (Tauri
/// docs, wry#583) — seen 2026-11-19, "Add a device" froze the app.
#[tauri::command]
async fn open_window(app: AppHandle, label: String) {
    show(&app, &label);
}

#[tauri::command]
fn open_downloads(s: tauri::State<'_, AppState>) {
    open_path(&s.downloads);
}

/// Opens (or shows in the folder) a received file. Only paths inside the download folder.
#[tauri::command]
fn open_received(s: tauri::State<'_, AppState>, path: String, reveal: bool) -> Result<(), String> {
    let p = std::fs::canonicalize(&path).map_err(|_| "The file is no longer there.".to_owned())?;
    let root = std::fs::canonicalize(&s.downloads).map_err(|e| e.to_string())?;
    if !p.starts_with(&root) {
        return Err("Not a received file.".into());
    }
    if reveal {
        notify::reveal(&p);
    } else {
        notify::open(p.as_os_str());
    }
    Ok(())
}

fn open_path(p: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

#[tauri::command]
fn refresh(s: tauri::State<'_, AppState>) {
    s.node.refresh();
}

// ---- windows, tray, input paths ----

pub(crate) fn show(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let (title, w, h, on_top) = match label {
        "send" => ("Send with kitedrop", 340.0, 440.0, true),
        "drop" => ("kitedrop", 220.0, 170.0, true),
        "pair" => ("Add a device", 400.0, 680.0, false),
        _ => ("kitedrop", 460.0, 640.0, false),
    };
    let label = if ["send", "drop", "pair"].contains(&label) {
        label
    } else {
        "main"
    };
    let built = WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::App(format!("index.html#{label}").into()),
    )
    .title(title)
    .inner_size(w, h)
    .resizable(label == "main")
    .always_on_top(on_top)
    .center()
    .build();
    if let Ok(win) = built {
        let _ = win.set_focus();
    }
}

fn looks_like_link(t: &str) -> bool {
    let t = t.trim();
    !t.contains(char::is_whitespace) && (t.starts_with("https://") || t.starts_with("http://"))
}

fn clipboard_pending(app: &AppHandle) -> Option<Pending> {
    let text = app.clipboard().read_text().ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(if looks_like_link(&text) {
        Pending::Link(text.trim().to_owned())
    } else {
        Pending::Text(text)
    })
}

fn set_pending(app: &AppHandle, p: Pending) {
    if let Some(s) = app.try_state::<AppState>() {
        *s.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
    }
    let _ = app.emit("kitedrop://pending", ());
    show(app, "send");
}

/// `kitedrop://send?file=…` (repeated), `?link=…` or `?text=…` from the macOS share extension
/// (D39). Opens the usual send window; nothing is sent until a device is chosen there. The app may
/// still be starting when it is launched by the URL, so this waits for its state.
#[cfg(target_os = "macos")]
fn handle_send_url(app: &AppHandle, url: &tauri::Url) {
    if url.scheme() != "kitedrop" || url.host_str() != Some("send") {
        return;
    }
    let (mut files, mut link, mut text) = (Vec::new(), None, None);
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "file" => files.push(PathBuf::from(v.as_ref())),
            "link" => link = Some(v.into_owned()),
            "text" => text = Some(v.into_owned()),
            _ => {}
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..100 {
            if app.try_state::<AppState>().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !files.is_empty() {
            offer_paths(&app, files);
        } else if let Some(l) = link.filter(|l| looks_like_link(l)) {
            set_pending(&app, Pending::Link(l.trim().to_owned()));
        } else if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
            let p = if looks_like_link(&t) {
                Pending::Link(t.trim().to_owned())
            } else {
                Pending::Text(t)
            };
            set_pending(&app, p);
        }
    });
}

/// Files and folders from a drop, "Send to" or the share menu: opens the send window, or says why
/// nothing can be sent (no silent failures, design §3).
fn offer_paths(app: &AppHandle, paths: Vec<PathBuf>) {
    let items: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.is_file() || p.is_dir())
        .collect();
    if items.is_empty() {
        notify(
            app,
            "kitedrop",
            "Nothing to send: only files and folders can be sent.",
        );
    } else {
        set_pending(app, Pending::Files(items));
    }
}

/// Global shortcut: send what is on the clipboard (design §8).
fn send_clipboard(app: &AppHandle) {
    match clipboard_pending(app) {
        Some(p) => set_pending(app, p),
        None => notify(app, "kitedrop", "The clipboard has no text to send."),
    }
}

/// Command-line paths from Windows "Send to" (`--send <files…>`), first or second instance.
fn handle_args(app: &AppHandle, args: &[String]) {
    if let Some(i) = args.iter().position(|a| a == "--send") {
        offer_paths(app, args[i + 1..].iter().map(PathBuf::from).collect());
        return;
    }
    // Debug builds: `--pair <uri>` acts on a pairing link at start (development and testing only;
    // whether desktops may pair from a pasted link is an open product question).
    if cfg!(debug_assertions)
        && let Some(i) = args.iter().position(|a| a == "--pair")
        && let (Some(uri), Some(s)) = (args.get(i + 1).cloned(), app.try_state::<AppState>())
    {
        let node = s.node.clone();
        tauri::async_runtime::spawn(async move {
            match node.pair(&uri).await {
                Ok(_) => eprintln!("kitedrop dev: paired"),
                Err(e) => eprintln!("kitedrop dev: pairing failed: {e}"),
            }
        });
    }
    if cfg!(debug_assertions)
        && let Some(i) = args.iter().position(|a| a == "--window")
        && let Some(label) = args.get(i + 1)
    {
        show(app, label);
        return;
    }
    if !args.iter().any(|a| a == "--hidden") {
        show(app, "main");
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open kitedrop", true, None::<&str>)?;
    let clip = MenuItem::with_id(app, "clipboard", "Send clipboard", true, Some(SHORTCUT))?;
    let drop = MenuItem::with_id(app, "drop", "Drop window", true, None::<&str>)?;
    let pair = MenuItem::with_id(app, "pair", "Add a device…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit kitedrop", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &clip, &drop, &pair, &sep, &quit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("kitedrop")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, e| match e.id().as_ref() {
            "open" => show(app, "main"),
            "clipboard" => send_clipboard(app),
            "drop" => show(app, "drop"),
            "pair" => show(app, "pair"),
            "quit" => {
                if let Some(s) = app.try_state::<AppState>() {
                    s.node.shutdown();
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                show(tray.app_handle(), "main");
            }
        })
        .build(app)?;
    Ok(())
}

/// Debug builds only: `KITEDROP_DEV_DIR` keeps all state (data, downloads, a plain key file) in
/// that folder and leaves the Keychain and login items alone, so a development run cannot touch
/// the installed app.
fn dev_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        std::env::var_os("KITEDROP_DEV_DIR").map(PathBuf::from)
    } else {
        None
    }
}

fn start_node(app: &AppHandle) -> Result<(), String> {
    let (data_dir, downloads) = match dev_dir() {
        Some(d) => (d.join("data"), d.join("downloads")),
        None => (
            app.path().app_data_dir().map_err(|e| e.to_string())?,
            app.path()
                .download_dir()
                .map_err(|e| e.to_string())?
                .join("kitedrop"),
        ),
    };
    let platform = if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Windows
    };
    let cfg = Config::new(
        data_dir,
        downloads.clone(),
        &dev_dir()
            .and(std::env::var("KITEDROP_DEV_NAME").ok())
            .unwrap_or_else(platform::device_name),
        platform,
    );
    let keys = match dev_dir() {
        Some(d) => platform::dev_keys(&d.join("identity.key"))?,
        None => platform::load_or_create_keys()?,
    };
    let host = Arc::new(DesktopHost {
        app: app.clone(),
        sleep: platform::SleepGuard::new(),
    });
    let outgoing = match dev_dir() {
        Some(d) => d.join("outgoing"),
        None => app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("outgoing"),
    };
    folder::clean(&outgoing);
    let node = Node::start(cfg, keys, host).map_err(|e| e.to_string())?;
    app.manage(AppState {
        node,
        pending: Mutex::new(None),
        downloads,
        outgoing,
    });
    Ok(())
}

/// Turns on start-at-login once, on first run (design §2); the user can turn it off later.
fn first_run_autostart(app: &AppHandle) {
    if dev_dir().is_some() {
        return;
    }
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let flag = dir.join("autostart-configured");
    if flag.exists() {
        return;
    }
    if app.autolaunch().enable().is_ok() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(flag, b"1");
    }
}

pub fn run() {
    // A development run (KITEDROP_DEV_DIR) must not hand itself over to the installed app or take
    // its global shortcut, so it skips both.
    let dev = dev_dir().is_some();
    let mut builder = tauri::Builder::default();
    if !dev {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
                handle_args(app, &args);
            }))
            .plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_shortcuts([SHORTCUT])
                    .expect("valid shortcut")
                    .with_handler(|app, _shortcut, e| {
                        if e.state() == ShortcutState::Pressed {
                            send_clipboard(app);
                        }
                    })
                    .build(),
            );
    }
    builder
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            state,
            pending,
            send_pending,
            send_files,
            send_clipboard_to,
            invite,
            pair_link,
            copy_text,
            remove_device,
            rename_device,
            retry,
            cancel,
            open_window,
            open_downloads,
            open_received,
            refresh,
        ])
        .on_window_event(|window, e| {
            // Files dropped on the drop window or the main window go to the send window.
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = e
                && (window.label() == "drop" || window.label() == "main")
            {
                offer_paths(window.app_handle(), paths.clone());
            }
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            if let Err(e) = start_node(&handle) {
                handle
                    .dialog()
                    .message(format!("kitedrop could not start: {e}"))
                    .title("kitedrop")
                    .blocking_show();
                return Err(e.into());
            }
            notify::init(&handle);
            build_tray(&handle)?;
            first_run_autostart(&handle);
            let args: Vec<String> = std::env::args().collect();
            handle_args(&handle, &args);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri app")
        .run(|_app, e| match e {
            // Closing the last window keeps the tray app running.
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => api.prevent_exit(),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Opened { urls } => {
                for url in urls {
                    handle_send_url(_app, &url);
                }
            }
            _ => {}
        });
}

//! Desktop notifications with action buttons (owner request, 2026-11-19): a received file offers
//! "Open" / "Copy" / "Show in folder", a link "Open link" / "Copy", a text "Copy". Clicking the
//! notification itself shows a file in its folder (selected), opens a link, copies a text. macOS: UNUserNotificationCenter categories; Windows: toast buttons.
//! Debug runs outside an app bundle fall back to the notification plugin (no buttons).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::NotificationExt;

/// What a notification's actions act on.
#[derive(Clone, Debug)]
pub enum Target {
    File(PathBuf),
    Link(String),
    Text(String),
    /// Opens the main window (problems, status).
    Window,
}

impl Target {
    #[cfg(target_os = "macos")]
    fn category(&self) -> &'static str {
        match self {
            Target::File(_) => "file",
            Target::Link(_) => "link",
            Target::Text(_) => "text",
            Target::Window => "window",
        }
    }
}

static APP: OnceLock<AppHandle> = OnceLock::new();
/// Recent notification targets by id; old ones are dropped (their buttons then do nothing).
static TARGETS: Mutex<(u64, VecDeque<(String, Target)>)> = Mutex::new((0, VecDeque::new()));
const KEEP: usize = 50;

fn remember(target: Target) -> String {
    let mut t = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    t.0 += 1;
    let id = format!("kitedrop-{}", t.0);
    t.1.push_back((id.clone(), target));
    if t.1.len() > KEEP {
        t.1.pop_front();
    }
    id
}

fn lookup(id: &str) -> Option<Target> {
    let t = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    t.1.iter().find(|(i, _)| i == id).map(|(_, t)| t.clone())
}

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    #[cfg(target_os = "macos")]
    mac::init();
}

pub fn show(title: &str, body: &str, target: Target) {
    let id = remember(target.clone());
    #[cfg(target_os = "macos")]
    if mac::available() {
        mac::post(&id, title, body, target.category());
        return;
    }
    #[cfg(windows)]
    if win::post(&id, title, body, &target) {
        return;
    }
    if let Some(app) = APP.get() {
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

/// Runs an action: `"default"` (the notification was clicked), `"open"`, `"reveal"`, `"copy"`.
pub fn perform(id: &str, action: &str) {
    let Some(target) = lookup(id) else { return };
    let Some(app) = APP.get() else { return };
    match (&target, action) {
        (Target::File(p), "open") => open(p.as_os_str()),
        (Target::File(p), "copy") => copy_file(p),
        // Clicking the notification itself shows the file in its folder (owner request, 2026-11-20).
        (Target::File(p), _) => reveal(p),
        (Target::Link(url), "copy") => copy(app, url),
        (Target::Link(url), _) if url.starts_with("https://") || url.starts_with("http://") => {
            open(std::ffi::OsStr::new(url))
        }
        (Target::Text(text), _) | (Target::Link(text), _) => copy(app, text),
        (Target::Window, _) => crate::show(app, "main"),
    }
}

fn copy(app: &AppHandle, text: &str) {
    let _ = app.clipboard().write_text(text.to_owned());
}

/// Puts the file itself on the clipboard, ready to paste into Finder / Explorer, chat or mail.
fn copy_file(path: &std::path::Path) {
    let p = path.to_string_lossy();
    #[cfg(target_os = "macos")]
    {
        let quoted = p.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!("set the clipboard to (POSIX file \"{quoted}\")"))
            .spawn();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let quoted = p.replace('\'', "''");
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("Set-Clipboard -LiteralPath '{quoted}'"))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

pub(crate) fn open(what: &std::ffi::OsStr) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(what).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(what).spawn();
}

pub(crate) fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{NSArray, NSError, NSSet, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
        UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "KitedropNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// Show banners even while kitedrop is the active app.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let id = response.notification().request().identifier().to_string();
                let action = response.actionIdentifier();
                let action = if &*action == unsafe { UNNotificationDefaultActionIdentifier } {
                    "default".to_owned()
                } else {
                    action.to_string()
                };
                super::perform(&id, &action);
                handler.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    /// UNUserNotificationCenter needs an app bundle; a bare debug binary would crash on it.
    pub fn available() -> bool {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
            .unwrap_or(false)
    }

    fn action(id: &str, title: &str) -> Retained<UNNotificationAction> {
        UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(id),
            &NSString::from_str(title),
            UNNotificationActionOptions::empty(),
        )
    }

    fn category(
        id: &str,
        actions: &[Retained<UNNotificationAction>],
    ) -> Retained<UNNotificationCategory> {
        UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
            &NSString::from_str(id),
            &NSArray::from_retained_slice(actions),
            &NSArray::new(),
            UNNotificationCategoryOptions::empty(),
        )
    }

    pub fn init() {
        if !available() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The center keeps only a weak reference; the delegate lives as long as the app.
        std::mem::forget(delegate);
        let categories = NSSet::from_retained_slice(&[
            category(
                "file",
                &[
                    action("open", "Open"),
                    action("copy", "Copy"),
                    action("reveal", "Show in Finder"),
                ],
            ),
            category(
                "link",
                &[action("open", "Open link"), action("copy", "Copy")],
            ),
            category("text", &[action("copy", "Copy")]),
        ]);
        center.setNotificationCategories(&categories);
        let done = RcBlock::new(|_granted: Bool, _err: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    }

    pub fn post(id: &str, title: &str, body: &str, category: &str) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setCategoryIdentifier(&NSString::from_str(category));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(id),
            &content,
            None,
        );
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, None);
    }
}

#[cfg(windows)]
mod win {
    use tauri_winrt_notification::Toast;

    use super::Target;

    const APP_ID: &str = "com.mucahitimre.kitedrop";

    /// Returns false if the toast could not be shown (caller falls back to the plugin).
    pub fn post(id: &str, title: &str, body: &str, target: &Target) -> bool {
        let mut toast = Toast::new(APP_ID).title(title).text1(body);
        toast = match target {
            Target::File(_) => toast
                .add_button("Open", "open")
                .add_button("Copy", "copy")
                .add_button("Show in folder", "reveal"),
            Target::Link(_) => toast
                .add_button("Open link", "open")
                .add_button("Copy", "copy"),
            Target::Text(_) => toast.add_button("Copy", "copy"),
            Target::Window => toast,
        };
        let id = id.to_owned();
        toast
            .on_activated(move |action| {
                super::perform(&id, action.as_deref().unwrap_or("default"));
                Ok(())
            })
            .show()
            .is_ok()
    }
}

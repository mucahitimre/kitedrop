//! Desktop-specific services: device identity in the OS secure store, the device name, a
//! sleep assertion while the outbox holds items, and the Windows network profile check.

use std::sync::mpsc::{Sender, channel};

use kitedrop_core::{DeviceKeys, NetworkIssue};
use zeroize::Zeroizing;

const KEYRING_SERVICE: &str = "com.mucahitimre.kitedrop";
const KEYRING_USER: &str = "device-identity";

/// Loads the identity from Keychain / Credential Manager, creating it on first run
/// (design §5: private keys live in the platform secure store).
pub fn load_or_create_keys() -> Result<DeviceKeys, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| e.to_string())?;
    match entry.get_secret() {
        Ok(bytes) if bytes.len() == 64 => {
            let bytes = Zeroizing::new(bytes);
            let sig: [u8; 32] = bytes[..32].try_into().expect("32 bytes");
            let kem: [u8; 32] = bytes[32..].try_into().expect("32 bytes");
            DeviceKeys::from_secret_bytes(&sig, &kem).map_err(|e| e.to_string())
        }
        Ok(_) => Err("stored identity has the wrong length".into()),
        Err(keyring::Error::NoEntry) => {
            let keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
            let (sig, kem) = keys.secret_bytes();
            let mut bytes = Zeroizing::new(Vec::with_capacity(64));
            bytes.extend_from_slice(&sig[..]);
            bytes.extend_from_slice(&kem[..]);
            entry.set_secret(&bytes).map_err(|e| e.to_string())?;
            Ok(keys)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Debug-build development identity in a plain file (never used by release builds).
pub fn dev_keys(path: &std::path::Path) -> Result<DeviceKeys, String> {
    if let Ok(bytes) = std::fs::read(path) {
        let sig: [u8; 32] = bytes
            .get(..32)
            .and_then(|b| b.try_into().ok())
            .ok_or("bad dev key")?;
        let kem: [u8; 32] = bytes
            .get(32..64)
            .and_then(|b| b.try_into().ok())
            .ok_or("bad dev key")?;
        return DeviceKeys::from_secret_bytes(&sig, &kem).map_err(|e| e.to_string());
    }
    let keys = DeviceKeys::generate(&mut kitedrop_core::os_rng());
    let (sig, kem) = keys.secret_bytes();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
    std::fs::write(path, [&sig[..], &kem[..]].concat()).map_err(|e| e.to_string())?;
    Ok(keys)
}

/// The name other members see for this computer.
pub fn device_name() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !name.is_empty() {
                return name;
            }
        }
        "Mac".into()
    }
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        "Computer".into()
    }
}

/// Holds a "keep the system awake" assertion while asked to, for at most `MAX_HOLD`
/// (design §9: bounded). Runs on its own thread because the Windows call is per-thread.
pub struct SleepGuard {
    tx: Sender<bool>,
}

const MAX_HOLD: std::time::Duration = std::time::Duration::from_secs(2 * 3600);

impl SleepGuard {
    pub fn new() -> Self {
        let (tx, rx) = channel::<bool>();
        std::thread::Builder::new()
            .name("kitedrop-power".into())
            .spawn(move || {
                let mut held: Option<(imp::Assertion, std::time::Instant)> = None;
                loop {
                    let timeout = held.as_ref().map_or(MAX_HOLD, |(_, since)| {
                        MAX_HOLD.saturating_sub(since.elapsed())
                    });
                    match rx.recv_timeout(timeout) {
                        Ok(true) if held.is_none() => {
                            held = imp::Assertion::take().map(|a| (a, std::time::Instant::now()));
                        }
                        Ok(true) => {}
                        Ok(false) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => held = None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("power thread");
        Self { tx }
    }

    pub fn set(&self, keep_awake: bool) {
        let _ = self.tx.send(keep_awake);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    //! `IOPMAssertionCreateWithName(kIOPMAssertionTypePreventSystemSleep)` (protocol.md §9).
    use std::ffi::{c_char, c_void};

    type CFStringRef = *const c_void;
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            enc: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    const ON: u32 = 255;

    pub struct Assertion(u32);

    fn cfstr(s: &std::ffi::CStr) -> CFStringRef {
        unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) }
    }

    impl Assertion {
        pub fn take() -> Option<Self> {
            let ty = cfstr(c"PreventSystemSleep");
            let name = cfstr(c"kitedrop: items waiting for your iPhone");
            let mut id = 0u32;
            let rc = unsafe { IOPMAssertionCreateWithName(ty, ON, name, &mut id) };
            unsafe {
                CFRelease(ty);
                CFRelease(name);
            }
            (rc == 0).then_some(Self(id))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

#[cfg(windows)]
mod imp {
    //! `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` on the power thread.
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    pub struct Assertion;

    impl Assertion {
        pub fn take() -> Option<Self> {
            let prev = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            (prev.0 != 0).then_some(Self)
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub struct Assertion;
    impl Assertion {
        pub fn take() -> Option<Self> {
            None
        }
    }
}

/// Windows: is the network of one of kitedrop's LAN adapters Public? (protocol.md §13). Only the
/// adapters kitedrop publishes count: VPN and Hyper-V / WSL networks are often Public and do not
/// matter (seen 2026-11-19 on a PC with a VPN and WSL).
#[cfg(windows)]
pub fn platform_issues() -> Vec<NetworkIssue> {
    use windows::Win32::Networking::NetworkListManager::{
        INetworkListManager, NLM_NETWORK_CATEGORY_PUBLIC, NetworkListManager,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };
    let norm = |s: &str| {
        s.trim_matches(|c| c == '{' || c == '}')
            .to_ascii_uppercase()
    };
    let lan: Vec<String> = kitedrop_core::node::lan_interfaces()
        .iter()
        .map(|i| norm(&i.id))
        .collect();
    let public = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(nlm) =
            CoCreateInstance::<_, INetworkListManager>(&NetworkListManager, None, CLSCTX_ALL)
        else {
            return Vec::new();
        };
        let Ok(conns) = nlm.GetNetworkConnections() else {
            return Vec::new();
        };
        let mut public = false;
        loop {
            let mut item = [None];
            let mut fetched = 0u32;
            if conns.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(conn) = item[0].take() else { break };
            let Ok(adapter) = conn.GetAdapterId() else {
                continue;
            };
            if !lan.contains(&norm(&format!("{adapter:?}"))) {
                continue;
            }
            if conn
                .GetNetwork()
                .and_then(|n| n.GetCategory())
                .is_ok_and(|c| c == NLM_NETWORK_CATEGORY_PUBLIC)
            {
                public = true;
            }
        }
        public
    };
    if public {
        vec![NetworkIssue::WindowsPublicProfile]
    } else {
        Vec::new()
    }
}

/// macOS: the Local Network permission cannot be queried; a denied permission shows up as
/// failed connections (not verified how to tell it apart yet — Phase 4).
#[cfg(not(windows))]
pub fn platform_issues() -> Vec<NetworkIssue> {
    Vec::new()
}

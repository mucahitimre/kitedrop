# Decision log

Newest last. Each entry: decision, reason, alternatives, status. Entries marked **needs owner decision** touch
[design.md](design.md) and wait for approval (stop point 3); the protocol draft follows the proposed option
until then.

Sources checked on 2026-11-17 are linked inline; "not verified" means no official source was found.

---

## D1 — Repository layout and toolchain (Phase 0a)

Cargo workspace at the root with `core/` as the only member for now; `desktop/` (Tauri) and `mobile/`
(Expo + native targets) are added in Phases 0b–3. Rust edition 2024; MSRV 1.87 (required by `hpke` 0.15).
Local checks in `scripts/ci.sh` (fmt, clippy `-D warnings`, tests).

## D2 — Cryptography crates

| Need | Crate | Status of the crate |
|---|---|---|
| HPKE (RFC 9180) | `hpke` 0.15 (rozbb/rust-hpke), pure Rust | Maintained. **Not audited**: README says "nobody has performed a paid audit of this crate"; Cloudflare did an internal, non-formal review of v0.8. [README](https://github.com/rozbb/rust-hpke) |
| Ed25519 | `ed25519-dalek` 3.0 | Maintained (3.0.0 released 2026-07-06). The dalek libraries had one external audit (Quarkslab, 2019, mainly curve25519-dalek). [Quarkslab](https://blog.quarkslab.com/security-audit-of-dalek-libraries.html) |
| X25519 | `x25519-dalek` 3.0 (through `hpke`) | as above; RUSTSEC-2024-0344 (timing) is fixed in the curve25519-dalek version in use |
| SHA-256, ChaCha20-Poly1305 | RustCrypto `sha2`, `chacha20poly1305` (through `hpke`) | maintained, widely used |

Mitigation for the unaudited HPKE crate: the official RFC 9180 test vectors for our exact suite and modes
(auth, auth_psk) run in CI ([core/tests/rfc9180.rs](../core/tests/rfc9180.rs)), seal and open, byte for byte.

Alternative: `hpke-rs` 0.8 (Cryspen, part of libcrux). Its libcrux backend is formally verified but has limited
suite support; there is no audit statement either, and its deterministic-test feature is not in the 0.8 release.
[hpke-rs](https://github.com/cryspen/hpke-rs).

**Decided 2026-11-19 (owner):** keep `hpke` with the RFC 9180 vectors in CI.

## D3 — Separate Ed25519 and X25519 keys

Two independent key pairs per device instead of converting one Ed25519 key to X25519. No key reuse across
algorithms, and each library uses its keys as intended. Cost: 32 more public-key bytes per member.

## D4 — Pairing uses HPKE mode_auth_psk, not a PAKE

The QR carries a 256-bit code, which meets RFC 9180's "at least 32 bytes of entropy" for a PSK (§5.1.2). A
PAKE only adds value for low-entropy secrets. mode_auth_psk gives mutual authentication with the library we
already use and test against RFC vectors. Full reasoning: [protocol.md §4.3](protocol.md#43-why-psk-and-not-pake).

## D5 — Envelope streaming through the HPKE context sequence

Records are sealed with consecutive uses of one HPKE context (nonce = base_nonce XOR seq, RFC 9180 §5.2), and the
AAD carries transfer id, record index and record type (design §6). This is the standard HPKE multi-message
use, not a custom nonce scheme. Default chunk 64 KiB; receivers accept 64 B–1 MiB so test vectors stay small.
The final record has its own type, so truncation and extension are detected. Memory use is two chunks.

## D6 — Group state = signed membership list + self-signed device records

Design §5 lists addresses and the Expo token as part of the group list. Addresses change with DHCP and the token
can rotate; putting them in the versioned list would create a new version (and potential fork) for every IP
change. They therefore live in per-device records, signed by the device itself with their own sequence number,
and travel with the list. Membership (keys, name, platform) stays in the versioned, member-signed list. The
group state as a whole is still versioned and signed — technical detail inside the design decision.

## D7 — Upload authentication without a round trip — **approved 2026-11-19**

Design §6 says every endpoint, including upload, authenticates with challenge-response. An iOS background upload
is created by the share extension, which exits right away; the system may start or restart it later, and the
request headers are fixed when the task is created. Apple's guidance for background sessions is to obtain a
token in the foreground and send it as a header, and to avoid authentication challenges in the background
(Apple DTS, [forum thread 28713](https://developer.apple.com/forums/thread/28713) — a forum answer, not API
documentation).

Proposal: interactive endpoints keep a listener-issued challenge; `/v1/inbox` signs the request with the
envelope's `transfer_id` as nonce, plus timestamp, target listener and length ([protocol.md §7.2](protocol.md#72-signed-requests-caller-proves-identity)).
Replays are harmless because the recipient's seen-cache answers with the stored receipt and applies nothing twice.
A bare device id is still never accepted.

Alternative kept open: the extension fetches a challenge just before creating the task (works only when the
target is online at share time; retries created later by the app would fetch again). Phase 0b measures how
late background uploads actually start.

## D8 — Do not rely on the background upload response body

Apple documents nothing about delivering the response body of a background upload task, and developers report not
receiving it ([forum thread 756283](https://developer.apple.com/forums/thread/756283)). The iPhone uses only the
HTTP status and fetches the receipt with `GET /v1/receipts/{transfer_id}`.

## D9 — Signed objects carry their exact bytes

Signatures cover the exact JSON body bytes, carried as base64url next to the signature. No canonical JSON, so no
canonicalization bugs between implementations.

## D10 — Default port 52525

Inside the IANA dynamic range, not LocalSend's 53317, so both apps can run side by side. Fallback 52525–52534.

## D11 — iOS Info.plist keys go into the containing app

`NSLocalNetworkUsageDescription` and `NSBonjourServices` belong in the app's Info.plist, not the share
extension's ([TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)).
Outgoing TCP to a LAN address triggers the Local Network prompt; the multicast entitlement is not needed for
Bonjour through the Network framework (same source).

## D12 — Android 17 local network permission

Apps that target Android 17 (API 37) need the runtime permission `ACCESS_LOCAL_NETWORK` (nearby-devices group)
for any LAN TCP/UDP/mDNS, incoming included; apps targeting API 36 or lower keep implicit access
([docs](https://developer.android.com/privacy-and-security/local-network-permission)). Onboarding asks for it and
the diagnostics report it as `local_network_denied`. This fits design §3 ("Local Network permission not granted").

## D13 — Android platform facts confirmed for the service design

`specialUse` needs `FOREGROUND_SERVICE_SPECIAL_USE` and the `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` property; review
applies only to Play submissions ([FGS types](https://developer.android.com/develop/background-work/services/fgs/service-types)).
Android 15 forbids starting `dataSync` (and camera, mediaPlayback, phoneCall, mediaProjection, microphone) FGS from
`BOOT_COMPLETED`; `specialUse` is not on the list ([Android 15 changes](https://developer.android.com/about/versions/15/behavior-changes-15)).
Battery-optimization exemption also allows starting the FGS from the background. **Background clipboard write:
not documented** (docs are silent); worked on a Samsung S22 / Android 16 on 2026-11-19 (Phase 3 test) — Phase 0b/3 tests it; the notification "Copy" action is the fallback (design §8).

## D14 — iPhone can act as pairing inviter while in the foreground

A foreground app may run an `NWListener`; accepting incoming TCP does not need Local Network permission (TN3179),
and the listener must close when the app goes to the background
([TN2277](https://developer.apple.com/library/archive/technotes/tn2277/_index.html)). Not verified on a device yet.

## D15 — How a desktop without a camera joins — **approved 2026-11-19: reverse QR (option 2)**

Design §5: "any device in the group shows a QR". A Windows PC (or a Mac whose webcam is awkward to point at a
screen) cannot scan. Options:

1. Desktops scan with the webcam (Mac and laptops only).
2. **Reverse QR (proposed):** the new desktop shows a QR, and a member (Android or iPhone) scans it and runs the
   same handshake towards the desktop, which listens anyway. The cryptography is unchanged; only who shows the QR
   changes.
3. Typed short code — needs a PAKE (D4), adds a second protocol.

Implemented in Phase 1 (core: roles of the handshake do not depend on who shows the QR) and Phase 2/3 (UI).

## D16 — Mobile project structure (Phase 0b)

- Expo SDK 58 with Continuous Native Generation: `ios/` and `android/` are generated by `expo prebuild` and not
  committed. Native behaviour lives in our own sources:
  - `mobile/modules/kitedrop-native/`: local Expo module (UI bridge). Its Android part also holds the
    foreground service and boot receiver; their manifest entries merge into the app.
  - `mobile/native/ios/KitedropCore/`: a pure-native CocoaPod = Rust core (static xcframework) + UniFFI Swift
    bindings + shared Swift (app group, upload observer). No React Native, so the share extension links it
    without a JS runtime (design §4).
  - `mobile/targets/share/`: the share extension, added by `@bacons/apple-targets` (maintained Expo-community
    plugin, v5); `pods.rb` links `KitedropCore` into it.
- Android package segment `native` is a Java keyword, so the module package is `com.mucahitimre.kitedrop.bridge`.
- Toolchain: Node ≥ 22.13 (Expo 58), JDK 21 for Gradle, Android SDK 37 / NDK 27.1 (pinned by React Native 0.88);
  `scripts/mobile-env.sh` sets them on macOS. Rust → mobile: `scripts/build-mobile-core.sh` (cargo-ndk, xcframework).
- React Native 0.88 targets SDK 36, so the Android 17 local-network runtime permission (D12) is not yet required;
  it becomes required when the target moves to 37.

## D17 — Spike-only shortcuts (removed in Phase 1/3)

`spike/` (tiny_http server, desktop prober) and the `kitedrop-ffi` functions that use it exist only for Phase 0b.
Keys are plain files (app files dir / app group container) instead of Keystore/Keychain; the iPhone pins the test
server with a spike-only `/spike/register` endpoint instead of real pairing. The HTTP stack for the product is
chosen in Phase 1.

## D18 — Runtime stack of the core (Phase 1)

| Need | Crate | Why |
|---|---|---|
| async runtime | `tokio` 1 | de-facto standard; required by the HTTP crates below |
| HTTP listener | `axum` 0.8 (hyper 1) | maintained by the tokio project; streaming request bodies, small feature set (`http1`, `json`, `tokio`) |
| HTTP client | `reqwest` 0.13, **no TLS, no default features** | plain HTTP on the LAN only (design §6); streaming upload bodies with an explicit `Content-Length` |
| persistence | `rusqlite` 0.43 (bundled SQLite) | one transactional file for queue, outbox, seen-cache and group state; WAL + `synchronous=FULL` so a state written before a network call survives a crash (design §3) |
| mDNS | `mdns-sd` 0.22, optional feature `mdns` | pure Rust, maintained; off for iOS builds (no multicast entitlement — the app uses Bonjour through the Network framework and passes results in with `Node::add_discovered`) |
| interfaces / netmask | `if-addrs` 0.15 | real prefix length per interface for the scan size (design §7) |

The node owns a small tokio runtime (2 workers). Network methods of `Node` are `async` and run on that runtime, so
they can be awaited from any executor (Tauri, UniFFI async) or none.

## D19 — Platform services through one `Host` callback interface

Clipboard writes, saving to Photos, HTTPS POST for Expo push, events and platform-only network checks are calls
from the core into the app (`Host` trait; UniFFI callback interface on mobile). Consequence: the core contains **no
TLS stack** — the only internet call (Expo push, design §9) goes through the platform's HTTP client, which already
handles system trust stores and proxies.

## D20 — A joiner's group chain starts at the parent of the version it was given (anchor)

The accept message carries the new group version and its parent, not the full history. The joiner anchors on the
parent (authenticated by the pairing PSK) and verifies the new version as its child (signed by the inviter it
pinned through the QR, joiner is a member). History before the anchor is never needed: acceptance only looks at
the parent. Anchoring on the parent rather than on the new version matters when the new version loses a fork
(two pairings at once, or a pairing during a rename): the joiner can then switch to the winner like everyone
else (review finding, 2026-11-19).

## D21 — Upload answered `not_member`: push the group once, then retry; `POST /v1/group` needs no request auth

Right after pairing, a member may not have the version that added the sender yet. On `not_member` the sender pushes
its group state to that listener (`POST /v1/group`) and retries the upload once; a second `not_member` fails the
send as protocol.md §11 says. For this to work the push cannot require the listener to know the sender, so
`POST /v1/group` has no request authentication: each version is accepted only if it chains onto the listener's
own version and is signed by a member of its parent, and each record only if its signer is a member — the
content authenticates itself. (The first implementation required membership and the recovery could never work —
review finding, 2026-11-19.)

## D22 — Former members may read `GET /v1/group`

A device removed while offline must learn that it was removed (design §5). `GET /v1/group` therefore accepts a
signed request from any device that appears in a stored version of the chain, not only the current one. A former
member gets the versions only — no device records, so no addresses and no iPhone push token. Every other endpoint requires current membership.
The member who removes a device also pushes the new version to it directly.

## D23 — v1 core listens and publishes IPv4 only

Link-local IPv6 needs scope ids that differ per device and interface; home networks always have IPv4. The QR
format and address strings already allow `[v6]:port`, so adding IPv6 later is not a protocol change.

## D24 — Text and links are limited to 8 MiB

They go to the clipboard; larger content is a file. Senders refuse larger text up front; receivers answer
`record_too_large`.

## D25 — iPhone share extension: `prepare_upload` + `Node::adopt_upload`

The extension has no running node. `prepare_upload` (stateless) seals the envelope into the shared container and
returns the URL and signed `Authorization` for the background session plus a small JSON record; the app adopts that
record into its queue (`sending`) and fetches the receipt with `GET /v1/receipts/{id}` (D8). If the recipient has
no receipt yet, the same envelope is uploaded again; the seen-cache makes that idempotent.

## D26 — Fork losers' changes are re-applied by whoever sees them

Protocol §3.2 says the author of the losing version re-applies it. A change whose effect is already in the winner
produces no new version, so letting any device that observes the lost version re-apply it is equivalent and does
not need to know who authored it. Concurrent re-applications are themselves resolved by the same fork rule.

## D27 — Fork rule: a removed signer never wins

Review finding (2026-11-19): with "smaller hash wins" alone, a device that was just removed (for example a stolen
phone) could sign a competing version that removes the remover, change `updated_at` until its hash is smaller, and
take over. Rule now (protocol.md §3.2): a version signed by a device that the competing version removes never
wins; if each removes the other's signer, the version a device already holds stays; otherwise the smaller hash
wins. Known limit: in that mutual-removal race, devices that received the attacker's version first keep it, so
the group can split. The owner then removes the device again from a device that shows the wrong list, or pairs
the affected device again. Accepted: it needs a stolen, unlocked device racing the owner's removal, and the
remover itself can no longer be pushed out.

## D28 — Rejected receipts for outbox items

An iPhone that cannot accept an outbox item for good (expired, failed decryption, …) answers with a receipt of
status `rejected` and the error code, so the sender shows `failed(rejected:<code>)` at once instead of `waiting`
for 7 days (design §3, no silent failures). Passing failures (network, storage, Photos) are skipped and retried at
the next poll, and one item never blocks the others.

## D29 — Other hardening from the Phase 1 review

- Transfer rows are written conditionally: a final state is never overwritten by a stale copy (cancel or expiry
  during an attempt, presence kicks). Expiry skips sends with an attempt in flight.
- Request bodies idle for 60 s are abandoned; client reads time out after 60 s; JSON answers from peers are
  capped at 256 KiB (scans talk to any host on the LAN).
- A flood of `POST /v1/challenge` evicts the oldest unused challenge instead of locking members out.
- Discovered addresses are capped at 4 per device; coalesced pushes are delayed to the end of the 30 s window
  instead of dropped; leftover spool files are removed at start.
- Known and accepted for now: SQLite calls run on the node's async workers (4 threads). Fine at this scale;
  revisit if the listener stalls under load (Phase 4 measurements).

## D30 — Desktop app choices (Phase 2)

- **UI without a build step:** static HTML/JS in `desktop/ui/` with `withGlobalTauri`; no npm toolchain for the
  desktop. CSP `script-src 'self'`; names and file names from other devices are set with `textContent` only.
- **Plugins (official, 2.x):** single-instance (Send to / second launch forward their arguments), autostart
  (LaunchAgent on macOS; enabled once on first run), global-shortcut (`Cmd/Ctrl+Shift+K` = send clipboard),
  notification, clipboard-manager, dialog (file picker).
- **Identity:** `keyring` 3 (macOS Keychain, Windows Credential Manager), 64 bytes under service
  `com.mucahitimre.kitedrop` (design §5).
- **Expo push (D19):** `reqwest` blocking with `native-tls` (Security.framework / SChannel): system trust store,
  no C crypto build for Windows.
- **Sleep assertion (design §9):** `IOPMAssertionCreateWithName(PreventSystemSleep)` / `SetThreadExecutionState`
  on a dedicated thread while the outbox holds items, released after at most 2 h.
- **Windows installer:** NSIS `perMachine` (administrator) so the post-install hook can add the inbound firewall
  rule for the Private profile only, and a "Send to" shortcut (`kitedrop.exe --send <files>`) for the installing
  user. MSI is not built (WiX runs only on Windows). Built on the Mac with `cargo-xwin`
  (`scripts/build-desktop.sh windows`; owner approved accepting Microsoft's CRT/SDK license, 2026-11-19).
  The installer is unsigned: SmartScreen shows a warning until a code-signing certificate exists.
- **macOS:** `LSUIElement` (no Dock icon), Local Network usage string and `_kitedrop._tcp` in Info.plist.
  Release builds are ad-hoc signed until a Developer ID certificate is set up (stop point 2).
- **Debug-only dev mode:** `KITEDROP_DEV_DIR` and `--pair` (see desktop/README.md) for tests that must not touch
  the installed app; compiled out of release builds.

## D31 — Desktops pair by copying and pasting the pairing link — **approved 2026-11-19**

Two desktops without a phone cannot pair: neither can scan. Owner chose (of: paste link / Mac webcam scan / only
through a phone) the paste link. The pairing window shows "Copy link" under every QR (normal and reverse) and a
field to paste a link from another device; pasting runs exactly the scan path (`Node::pair`). The link carries the
same 256-bit code, valid 60 s and once, so the cryptography is unchanged (protocol §4.3 still holds: no PAKE is
needed for a high-entropy code). Accepted risk: a channel used to move the link (chat, e-mail, a synced
clipboard) sees the code for that minute and could pair first; the user then sees an unexpected device in the
list and removes it.

## D32 — Mobile apps (Phase 3)

- **FFI:** `kitedrop-ffi` exposes the frozen `Node` API as `KitedropNode` (UniFFI object; async methods become
  Kotlin `suspend` / Swift `async`), a `NodeHost` callback interface, and stateless helpers for the share
  extension (`generate_identity`, `prepare_upload`, `qr_matrix`). iOS builds leave out the `mdns` feature.
  Core additions (no changes): `Node::member_keys`, `Node::addresses`.
- **Android:** one node per process, started by the `specialUse` foreground service (boot, update, app open).
  Identity: 64 bytes encrypted with an AES-GCM key held in the Android Keystore. Received files land in the
  app's private folder and the host copies them into `Downloads/kitedrop` through MediaStore (Android 10+).
  Text/links: clipboard write is attempted and the notification always has a "Copy" action (design §8, D13).
  Share target: a native `ShareActivity` dialog (no React Native) that copies shared content into the cache and
  sends it. mDNS runs without a multicast lock; record addresses and the scan are the fallback (not verified
  whether Android delivers mDNS replies without the lock).
- **iPhone:** node runs only while the app is in use or relaunched for the share extension's background
  session (listen off, mDNS off). Identity in the Keychain, access group = the app group, so the extension
  uses the same keys (not verified on device yet). Bonjour through `NWBrowser` feeds `add_discovered`; a
  `PolicyDenied` browser state reports `local_network_denied`. Photos/videos go to Photos when add-only
  access is granted, other files to `Documents/kitedrop` (Files app), text to the pasteboard. The app writes
  the device list to the app group for the extension; the extension writes one upload record per send into
  `pending/`, which the app adopts (D25).
- **UI:** Expo Router screens in `mobile/src/app/` (devices, activity, add device: scan / show code
  (Android) / paste link, pairing deep link `kitedrop://pair`, compose, device). QR codes are drawn from the
  core's module matrix, so no QR library is added. The phone's own camera app can scan a pairing QR too: the
  `kitedrop` URL scheme opens the pairing screen, which still asks for a tap.
- **Push to iPhone (design §9):** needs an Expo project id (EAS) and APNs credentials — external accounts, stop
  point 2. Until the owner sets them up, the iPhone gets items when its app opens or is refreshed.
- The Phase 0b spike crate and scripts are removed (D17); `scripts/mobile.sh` builds and installs the apps.

## D33 — macOS builds must be properly signed (Local Network permission)

Seen 2026-11-19: the Tauri release bundle was only linker-signed (ad-hoc, identifier
`kitedrop_desktop-<hash>`). With the Local Network toggle on, all outgoing LAN connections of the app failed;
re-signed with the owner's Developer ID (identifier `com.mucahitimre.kitedrop`, hardened runtime) they worked
immediately. `scripts/build-desktop.sh mac` takes the identity from `APPLE_SIGNING_IDENTITY` (never stored in
the repo). Notarization (Apple ID / API key) stays for Phase 5 (stop point 2). Follow-up: when outgoing probes
fail with "no route to host" on macOS while other members are known on this subnet, report
`local_network_denied` instead of `ap_isolation_suspected` (Phase 4 heuristics).

## D34 — A device in a group never leaves it silently when pairing

Seen 2026-11-19: the Android phone, a member of the Mac's group, scanned the *normal* QR of a newly installed
Windows PC (the default tab) and, following the protocol, joined the PC's single-member group — leaving the
Mac and the iPhone without any warning. The rule now (protocol.md §4.4): the device that is alone joins the
other's group whichever QR was shown (a lone shower of a normal QR accepts the offer flow); two devices that
both have other members refuse with `already_in_group`. No message format changes. Regression tests:
`group_member_scanning_lone_device_adds_it`, `devices_in_two_groups_do_not_merge_silently`.

Also seen in the same test: every send from Windows failed because the envelope was synced through a
read-only handle (`FlushFileBuffers` needs write access on Windows); fixed in the sender. CI does not run on
Windows yet — Phase 5 adds a Windows job so this class of bug is caught before a device test.

## D35 — Desktop notifications with actions; UI refresh (Phase 3, owner request)

- Received file → "Open" / "Copy" / "Show in folder" ("Copy" puts the file itself on the clipboard: macOS
  `osascript` with a POSIX file, Windows PowerShell `Set-Clipboard -LiteralPath`); link → "Open link" / "Copy"; text → "Copy" (the item is already on
  the clipboard; the button copies it again). Clicking the notification shows a received file in its folder
  (selected; owner request 2026-11-20, it used to open the file), opens a link, copies a text; problem
  notifications open the main window. macOS: `UNUserNotificationCenter` categories and a delegate (needs the app
  bundle; bare debug binaries fall back to the notification plugin). Windows: toast buttons through
  `tauri-winrt-notification` (the crate the plugin itself uses), AUMID = bundle identifier. Targets live in a
  small in-memory table (last 50), so buttons of very old notifications do nothing.
- Main window: device cards with platform icons and online state, inline rename, two-click remove, files can be
  dropped on the main window, received files open on click with "Show in folder", relative times.

- Mobile UI refresh: same palette as the desktop; icons from `expo-symbols` (Expo's own module: SF Symbols on
  iOS, Material Symbols on Android, so both look native; no third-party icon font). Installed with
  `npm install --force` because `npx expo install` stops on an existing peer conflict (react-dom 19.3.1 wants
  react 19.3.1; Expo SDK 58 pins 19.3.0); `--legacy-peer-deps` was tried and rejected because it dropped
  Babel/Metro packages from the lockfile.
- The isolation warning waits for two silent presence rounds; a freshly started app showed it for seconds.

## D36 — Open-source packaging (Phase 5)

- **CI** (`.github/workflows/ci.yml`): Linux (core, cli, ffi: fmt, clippy, tests incl. RFC 9180 vectors),
  Windows (whole workspace incl. the desktop crate) and a mobile typecheck. macOS is not in CI: the full
  `scripts/ci.sh` runs on a Mac before every commit (project rule), and macOS runner minutes cost ten times
  Linux minutes on a private repository.
- **Release** (`.github/workflows/release.yml`) on `v*` tags: Windows NSIS (unsigned, design §10), macOS universal
  `.dmg`/`.zip` signed and notarized by Tauri when the secrets exist, Android arm64 APK re-signed with
  `apksigner` when a release keystore secret exists; a **draft** release with SHA-256 sums, published by the
  owner. Every secret is optional so forks and early tags still build; missing ones are reported as warnings.
  Notarization uses an App Store Connect API key (Tauri's documented `APPLE_API_*` variables) rather than an
  Apple ID password, so it can be revoked separately.
- **Supply chain:** third-party actions are pinned to full commit SHAs (version in a comment); the Tauri CLI
  (`@tauri-apps/cli@2.12.6`) and `cargo-ndk 4.1.2` are pinned to the versions used locally.
- **TestFlight** (`scripts/testflight.sh`): `xcodebuild archive` + `-exportArchive` with `method
  app-store-connect`, `destination export|upload`, `testFlightInternalTestingOnly` (keys checked in
  `xcodebuild -help`). It uses the account signed in to Xcode, so no App Store Connect key is stored anywhere.
  Not run yet: it needs the App Store Connect app record (owner, stop point 2). Export compliance
  (`ITSAppUsesNonExemptEncryption`) is left to the owner.
- **Mobile icons** are drawn from the desktop mark by `scripts/make-mobile-icons.swift` (CoreGraphics, no extra
  tools): opaque full-bleed iOS icon, Android adaptive layers with the mark inside the safe zone.
- **License holder** in `LICENSE`: the owner's name, as in the owner's other repositories.
- **README** follows the layout of the owner's Shutterbar README (owner request): logo, tagline, badges, hero
  image, essentials, why / solution, usage, troubleshooting, architecture, limitations, roadmap. Images in
  `docs/images/` were taken with test devices (`kitedrop daemon`, a development desktop run, the iOS
  simulator) so no real device names appear.
- **First CI run (2026-11-19) found three real problems**, all fixed: (1) on Windows, Git checked the test
  vectors out with CRLF and the byte comparison failed → `.gitattributes` keeps `protocol/vectors/*.json` LF;
  (2) on Linux an item parked before the sender had the iPhone's push token never got a push → a record that
  brings a new or changed token now triggers a push when items wait (test `push_follows_a_token_that_arrives_later`,
  checked to fail without the fix); (3) `npm ci` failed on the react-dom 19.3.1 peer conflict → `react-dom`
  pinned to 19.3.0, the version Expo SDK 58 lists (`expo/bundledNativeModules.json`). That also unblocked
  `expo lint` (eslint-config-expo), which now runs in CI; its three findings (setState inside effects) are fixed.

## D37 — Progress of large sends (owner request, 2026-11-19)

- The owner asked for a progress bar for large files. The core counts bytes in both phases of a send: reading the
  source while sealing and handing envelope chunks to the HTTP connection while uploading. It reports them as
  `Event::TransferProgress` (files ≥ 1 MiB, at most every 200 ms, always at 100 %) and keeps the latest value in
  memory for `Node::progress`; nothing is persisted or sent on the wire. Upload bytes count when they are handed to
  the connection, so the bar can run slightly ahead of what the receiver has stored; *Delivered* still waits for the
  receipt.
- Desktop: a progress event updates only that row (bar, percent, "x of y", speed, time left); the full re-render
  stays on state changes. Mobile: progress is part of `transfers()`; rows show the same bar. Receiving devices show
  no progress yet.

## D38 — Folders are sent as a .zip (owner report, 2026-11-20)

- Dropping a folder on the desktop app did nothing (only files were accepted, silently). Folders are now offered
  in the send window and zipped when a target is chosen, as AirDrop does: entries under the folder's own name,
  symbolic links stored as links (app bundles need them) and never followed, modification times kept,
  `.DS_Store` skipped. Archives go to the app's cache (`outgoing/`) and are removed after a day; the core seals its
  own copy at once. Nothing sendable now produces a notification.
- Crate: `zip` 9.8 (zip-rs, maintained), write side only, `deflate-flate2-zlib-rs` (pure Rust) and `chrono` for
  local modification times. The archive was checked with three independent readers (`unzip -t`, Python
  `zipfile`, `ditto`).
- Compression level: default deflate. Measured on a 1.9 GB app bundle (ChatGPT.app, release build, external SSD):
  default 19.2 s → 836 MB, level 1 10.7 s → 1036 MB; at the 25 MB/s seen on Wi-Fi both total about 52 s, and the
  default leaves a smaller file on the receiver.
- The send window shows zipping progress (bytes of content read, ~5 updates a second), marks the chosen device and
  disables the others until the send is queued (owner feedback: "Zipping the folder…" looked idle and the devices
  still looked clickable).

## D39 — macOS share extension — **approved 2026-11-20 (option a)**

- "Share → kitedrop" in Finder, Safari and other apps. Design v2.6 moves this from "out of v1" into v1.
- The extension (`desktop/macos-share`, Swift/AppKit, `com.apple.share-services`, sandboxed as every app extension
  must be; keys checked against Xcode's own Share Extension template) sends nothing itself: it hands file paths, a
  web link or text to the app as `kitedrop://send?file=…|link=…|text=…` and closes. The app (URL scheme in its
  Info.plist, `RunEvent::Opened`) opens its usual send window, the same flow as Windows "Send to". This keeps all
  sending, keys and network access in one place and needs no app group: the app is not sandboxed and reads the
  paths directly.
- Activation rule `TRUEPREDICATE` (as Xcode's template): kitedrop appears for everything; not accepted by the App
  Store, fine for Developer ID distribution.
- Build: `scripts/build-mac-share.sh` compiles with `swiftc` (`-application-extension`, entry `_NSExtensionMain`,
  universal in CI), signs the extension with its sandbox entitlement; `tauri.conf.json` copies it into
  `Contents/PlugIns`, and Tauri signs the app around it without `--deep`, so the extension's signature stays
  (checked with `codesign --verify --deep --strict`). CI imports the Developer ID certificate into a temporary
  keychain before both are signed.
- Any app or web page can open a `kitedrop://send` URL. It only fills the send window; nothing is sent until the
  user picks a device, and only to the user's own devices (SECURITY.md).
- Not verified yet: whether macOS lists the extension without enabling it in System Settings → Extensions →
  Sharing, and that a sandboxed share extension may open the URL (needs the installed app).

## D40 — iPhone share extension: app look, every shared item (owner feedback, 2026-11-20)

- The extension's picker was plain filled buttons ("looks very old"). It now uses the app's palette (light and
  dark), the app mark, a summary of what is shared ("3 photos", file name, "Link") and device cards with platform
  icon, online dot and status; the chosen card is marked with a spinner, the others are dimmed, and the result is
  shown with a check or warning before the sheet closes.
- It used to send only the first attachment (`.first`) when several items were shared. Now every item gets its own
  envelope and background upload, one after the other.
- The card no longer says an offline target receives the item "when it is back": whether the background upload is
  retried after it fails is not verified.

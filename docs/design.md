# kitedrop — Design v2.6

v2.6 (2026-11-20, approved by the owner): macOS "Share → kitedrop" through a share extension (moved into v1
from §11); folders are sent as a .zip (D38, D39).
v2.5 (2026-11-19, approved by the owner): desktops can pair by copying and pasting the pairing link (D31).
v2.4 (2026-11-19, approved by the owner): reverse QR for joining devices that cannot scan (D15);
non-interactive upload authentication (D7). See [decisions.md](decisions.md).

Status: binding. Product/design changes require the owner's approval (see [plan.md](plan.md#stop-points)).
Technical detail choices inside a decision are free but must be logged in [decisions.md](decisions.md).

## 1. Product

- kitedrop sends files, text and links between the owner's own devices (iPhone, Android, Windows, Mac)
  on the same home network, with an AirDrop-like "two taps" flow. MIT licensed, open source on GitHub.
- iPhone → Mac keeps using real AirDrop; kitedrop covers every other direction.
- Origin: in LocalSend, devices appear and disappear from the list, and "send" sometimes delivers nothing.
  kitedrop exists so that neither of those two problems happens.

## 2. Device roles

- **Mac and Windows:** Tauri v2 tray/menubar app. Starts at login, always listens (local HTTP server).
- **Android:** always listens through a foreground service of type `specialUse`
  (with `PROPERTY_SPECIAL_USE_FGS_SUBTYPE`). Starts at boot. Asks for battery-optimization exemption.
  The `dataSync` type is NOT used (Android 15 time limit, cannot be started at boot).
- **iPhone:** does not listen. Sends through a native Swift share extension. Pulls items addressed to it
  from the other devices' outboxes.

## 3. Reliability principles (the LocalSend lesson — highest priority)

- The device list comes from the paired group, not from discovery. A paired device never disappears from
  the list; only its online/offline indicator changes.
- No silent failures. Every send has a persistent state (survives app restart):
  `queued → sending → delivered | waiting | failed(reason)`. `delivered` is written only after the
  recipient's signed delivery receipt.
- A send to an unreachable target stays queued and is retried automatically with backoff.
- Network problems are detected and stated plainly: Windows network profile is Public, guest network /
  AP isolation suspected, Local Network permission not granted, different subnet.

## 4. Shared core

- One Rust crate (`core/`): protocol, identity/group, envelope encryption, HTTP client/server, outbox,
  queue/retry, discovery.
- Desktop: Tauri uses the crate directly. Mobile: Swift and Kotlin bindings via UniFFI. Expo / React Native
  is UI only and calls the core through native modules.
- The iOS share extension and the Android service do NOT depend on the JS runtime; they call the core
  directly through the native bindings.

## 5. Identity, pairing, group

- Every device has a persistent key pair (X25519 for encryption; Ed25519 where signatures are needed).
  Private keys live in the platform secure store (Keychain / Keystore / OS credential store).
- Group = list of trusted members (public key, device name, platform, last known addresses, Expo push token
  for iPhones). The list is versioned and signed; changes propagate to all members.
- Joining: any device in the group shows a QR code. QR content: a one-time pairing code valid for 60 seconds
  + the inviter's key fingerprint + address(es). A long-lived key NEVER goes into the QR. The code is used to
  mutually authenticate the pairing channel (PSK or PAKE; the choice is justified in
  [protocol.md](protocol.md)).
- Reverse QR: a joining device that cannot scan (e.g. a desktop without a usable camera) shows the QR itself,
  and a group member scans it and runs the same handshake towards it. Only who shows the QR changes (D15).
- Pairing link: desktops can also copy the QR's content as a link and paste a link from another device, so two
  desktops pair without a phone. Same content, same 60-second one-use code; whatever channel carries the link
  also sees the code during that minute (D31).
- Removing a device: "remove device" produces a new list version; the others reject that key.

## 6. Envelope format (one format for every transfer)

- No Noise channel: iOS background `URLSession` opens the HTTP connection itself, so a custom handshake
  cannot be inserted.
- Every send is an envelope sealed to the recipient's public key: HPKE (RFC 9180) in auth mode, which also
  authenticates the sender.
- Large content is encrypted in chunks (streaming); a file is never loaded into memory as a whole.
  Counter-based nonce; AAD carries `transfer_id`, `chunk_index`, `is_final`. Truncation and reordering must
  be detected.
- Metadata (file name, MIME, size, content type: file/text/link) is in the encrypted header.
- Replay protection: cache of seen `transfer_id`s (especially against replaying a clipboard overwrite).
- Transport: plain HTTP + envelope. Endpoints (probe, outbox pull, receipt) authenticate with
  challenge-response; uploads carry a signature bound to the listener, the transfer id and the length, because an
  iOS background upload cannot do a round trip (D7). A bare device id is never enough.
- Machine-readable test vectors (envelope, pairing, receipt) live under `protocol/`; core tests run
  against them.

## 7. Discovery and reachability

- Order: last known address → mDNS (`_kitedrop._tcp`) → parallel unicast scan sized by the network's REAL
  netmask. The probe response proves identity through challenge-response.
- No dependency on multicast. On iOS, do nothing that needs the multicast entitlement. Info.plist:
  `NSLocalNetworkUsageDescription`, `NSBonjourServices` if Bonjour is used, `NSAllowsLocalNetworking`
  for ATS.
- Light periodic check for the online indicator (between desktops/Android), at battery-friendly intervals.

## 8. Platform flows

- **iPhone send:** Swift share extension (app group) → reads the device list from the shared container →
  stream-encrypts the envelope into a file in the shared container through the core → hands it to a
  background `URLSession` upload → exits. Mind the memory limit (stream from the file URL).
- **iPhone receive:** when the app opens and when a notification is tapped, polls every member's outbox,
  downloads envelopes, sends receipts. Photo/video → Photos (with permission), file → a kitedrop folder in
  Files, text/link → clipboard.
- **Android:** sends through a share target; receives through the service. File → `Downloads/kitedrop`,
  text/link → clipboard. Verify background clipboard-write restrictions; if writing does not work, add a
  "Copy" action to the notification.
- **Desktop:** global shortcut (send what is on the clipboard), small drop window (a tray icon cannot be a
  drop target), Windows classic "Send to" menu (the new Windows 11 context menu is out of v1), macOS share
  menu ("Share → kitedrop" in Finder, Safari and other apps) through a share extension that hands the item to
  the app's send window. Folders are sent as one .zip. Receive:
  notification, file → `Downloads/kitedrop`, text/link → clipboard. No accept dialog (within the group).
- **Windows install:** inbound firewall rule for the Private profile only; if the profile is Public, warn and
  guide the user.
- **macOS:** Local Network permission is needed for outgoing LAN connections and mDNS; onboarding guides the
  user. Dev builds may also need an Apple-signed identity.

## 9. Delivery to iPhone

- The sender keeps the item for the iPhone in its own persistent outbox (default TTL 7 days, configurable).
- At the same time it sends a content-free notification through the Expo push API (`exp.host`) with the
  iPhone's token: only the sender device name ("New item from Windows"); no file name, no content. Expo
  "enhanced push security" stays OFF (conscious trade-off: the token is shared only during pairing; the worst
  case is empty-notification spam). Without internet the push is skipped; the item still waits in the outbox.
- While the outbox is non-empty, a desktop prevents sleep for a bounded time (Windows/macOS power APIs).
  Closing the lid defeats this; replicating pending items to another device is v1.1.

## 10. Distribution

- GitHub Actions → Releases: Windows (unsigned), macOS (signed + notarized), Android APK.
  iOS: TestFlight (owner only).

## 11. Out of v1

- Mac↔Windows clipboard sync, Windows 11 new context menu, App Store / Play Store
  release, replicating pending items to another device, content transfer over the internet.

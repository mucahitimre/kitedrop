# Progress log

## 2026-11-17 — Phase 0a: protocol and skeleton — done

- Repo skeleton: Cargo workspace, `core/`, `desktop/`, `mobile/`, `protocol/`, `docs/`, `scripts/`.
- Docs: design.md (design v2.3), plan.md (plan v1.1), protocol.md (v1 draft), decisions.md (D1–D15).
- Core primitives that the vectors come from: identity (fingerprint, device id), streaming envelope seal/open,
  signed objects (group list, device record, receipt), QR pairing (mode_auth_psk), probe and request signatures.
- Test vectors: 5 generated files + official RFC 9180 subset. Tests: 9 (all vectors, 11 envelope attack cases,
  signature and pairing negatives, RFC 9180 seal/open conformance).
- `scripts/ci.sh` (fmt, clippy -D warnings, test): green.
- Platform and crypto assumptions checked against official sources; results in decisions.md.

Not done in 0a (later phases): HTTP server/client, outbox, queue, discovery, replay cache, listener policy
errors (`not_member`, `challenge_invalid`, …) — Phase 1. Tauri and Expo scaffolds — Phases 0b/2/3.

Environment note: the Android SDK was installed in Phase 0b.

## 2026-11-19 — Phase 0b: spikes — done; stop point 1

- Owner decisions recorded (D2 keep `hpke`, D7 approved, D15 reverse QR); design v2.4.
- Toolchain installed: JDK 21, Android SDK 37 / NDK 27.1, emulator, Node 22, Rust iOS/Android targets, cargo-ndk.
- `ffi/` UniFFI bindings, `spike/` test server + prober, Expo SDK 58 app with local module, Android
  `specialUse` service + boot receiver, iOS share extension via `@bacons/apple-targets`.
- Verified automatically: builds on both platforms; the share extension links no React/Expo; Android emulator:
  JS→Kotlin→Rust call, signed probes, start at boot, restart after update. Details: docs/spikes/phase-0b.md.
- Fixed on the way: UniFFI error field `message` clashes with Kotlin `Throwable.message`; JNA needs R8 keep
  rules; xcframework needed an x86_64 simulator slice; Expo 58 async functions must be `@Sendable`.
- iPhone spike done on the device (2026-11-19): extension peak memory 9.9 MB, uploads of 348 MB and 679 MB complete
  with the extension closed and the phone locked. Details: docs/spikes/phase-0b.md.
- Android overnight test done on the device (2026-11-19): 92/92 probes answered over 7 h 43 min, 85 % of the
  time in deep Doze, started by itself after a restart; kitedrop's battery share ~0.18 mAh. Details:
  docs/spikes/phase-0b.md.
- Phase 0b complete. Stop point 1: waiting for the owner's product decision before Phase 1.

## 2026-11-19 — Phase 1: Rust core — done

Owner decisions at stop point 1 (2026-11-19): design unchanged, go to Phase 1; Samsung-only Android result
accepted, other vendors in Phase 4; iPhone test videos deleted.

- `core/src/node/`: the running device behind one `Node` handle — HTTP listener (every endpoint of protocol §6
  with the §7 authentication rules), HTTP client, persistent send queue with backoff and TTL, outbox + content-free
  Expo push for iPhones, outbox polling for the iPhone, receipts, group sync (push/pull, forks, removal),
  pairing over HTTP (normal and reverse QR), discovery (last address → mDNS → netmask-sized scan), presence,
  network diagnostics.
- `core/src/group.rs` (chain rules, forks, records), `core/src/store.rs` (SQLite), reverse-QR messages in
  `pairing.rs` with test vectors.
- `cli/`: `kitedrop daemon`, a headless device driven over stdin/stdout (development and process-level tests).
- Public API frozen and documented: docs/api.md. Technical choices: decisions.md D18–D26. protocol.md is now v1.
- Tests (all green in `scripts/ci.sh`): vectors (8), group rules (6), node end-to-end on loopback (7: pairing,
  reverse QR + propagation, text/link/file, offline → retry after restart, iPhone outbox + share-extension upload,
  removal, expiry, listener rejection of truncated / wrong-length / stale / non-member / replayed uploads),
  two-process test (1). A mutation check (replay protection disabled) makes the replay test fail as expected.
- Manual LAN check on the Mac: two daemons on the real interface with mDNS; after one restarted on another port
  the other found it again and delivered.
- Cross-compiles for `aarch64-apple-ios` and Android `arm64-v8a`.

Not verified yet: mDNS and the scan between two different machines, Windows (`windows_public_profile` comes from
the app, Phase 2), the AP-isolation heuristic (Phase 4).

Independent review (separate agent, unanchored) found 15 issues, 3 confirmed with tests. All fixed, with
regression tests: removed device could win a group fork by grinding its hash (D27); D21 recovery could never work
(D21); a joiner anchored on a losing fork stayed stuck (D20); stalled uploads, lost state updates, one bad outbox
item blocking the iPhone's pulls (D28), challenge flood, unbounded peer answers and address lists, former members
seeing records, dropped pushes, CLI key file permissions (D29). Tests now: vectors 8, group 9, node 8,
two-process 1, unit 1 — green in `scripts/ci.sh`.

## 2026-11-19 — Phase 2: desktop — built on macOS; stopped for owner decisions

- Tauri v2 app (`desktop/`): tray / menu bar, start at login, global shortcut for the clipboard, drop window,
  send window (target picker), main window (devices with online state, send status with retry/cancel, received
  items, plain-language network problems), pairing window (normal QR and reverse QR), Windows "Send to" and
  firewall rule in the installer, Windows network-profile check, sleep assertion while the outbox is non-empty,
  identity in Keychain / Credential Manager. Choices: decisions.md D30.
- Verified on the Mac (debug build, isolated dev folder): paired with a `kitedrop daemon` device; text from it
  landed in the macOS clipboard; a 150 KB file landed in the download folder byte-identical; UI screenshot
  checked. Release `.app` bundles (19 MB, ad-hoc signed, LSUIElement, Local Network string present).
- Not verified: anything on Windows (the Windows-only code is not even compiled yet — see the open question on
  how to build it), desktop-to-desktop pairing (open product question), the drop window and the global shortcut
  by hand, macOS Local Network permission prompt with an ad-hoc signed app.

## 2026-11-19 — Phase 3: mobile — done with three devices; Windows and iPhone push pending

Owner decisions (2026-11-19): desktops pair by copy/paste link (D31, design v2.5); Windows built on the Mac
with cargo-xwin (Microsoft license accepted by the owner); Windows test later.

- Windows installer cross-built on the Mac (`scripts/build-desktop.sh windows`): first compile of the
  Windows-only code, clippy clean for `x86_64-pc-windows-msvc`; NSIS hooks included, per-machine install.
- FFI on the real node API; Android service/share target/Keystore identity/MediaStore delivery; iOS
  Keychain identity, Bonjour, Photos/Files delivery, share extension with device picker and adopted uploads;
  Expo Router UI. Choices: decisions.md D32.
- Verified: Android release APK installed on the phone, the new node answers signed probes over Wi-Fi as a
  v1 group; iOS app + share extension compile (Release, generic device). CI green.
- Not verified yet: everything that needs hands on the phones (pairing, sends, share sheets, clipboard,
  Photos), iPhone push (stop point 2), lint (`expo lint` setup fails on a peer-dependency conflict).
- Disk: the Mac had 165 MB free at one point; build outputs were cleaned (all regenerable).
- Device tests 2026-11-19 (docs/testing/phase-3-devices.md): every flow between the Mac, the Samsung S22 and
  the iPhone 15 Pro works — pairing by QR, text both ways, photos through both share sheets, the iPhone's
  outbox pull and its share-extension upload with a fetched receipt. Fixed on the way: Mac builds must be
  Developer ID signed for Local Network (D33); iOS extension picker; Photos permission.
- Open: the fourth device (Windows), iPhone push (needs Expo/APNs accounts — stop point 2), lint setup.

## 2026-11-19 — Phase 3 continued: Windows, notification actions, UI refresh

- Windows (prereleases v0.1.0-dev.1 … dev.7, private repo): installed on the owner's PC; four-device group
  works (docs/testing/phase-3-devices.md). Fixed on the way: virtual/VPN adapters caused false network
  warnings; window creation from a sync command froze the app; sealed spool files could not be synced on
  Windows (read-only handle); a phone silently joined the PC's group from a normal QR (D34).
- Desktop notifications with action buttons (owner request): received file → Open / Copy / Show in
  Finder|folder; link → Open link / Copy; text → Copy. macOS UNUserNotificationCenter categories, Windows toast
  buttons. "Copy" puts the file itself on the clipboard (verified on macOS with a known file: clipboard holds a
  file URL; Windows not verified yet).
- Desktop UI refresh: device cards with platform icons and online dot, compact icon actions, relative times,
  clickable received files, segmented pairing window, drag overlay.
- Isolation warning now waits for two silent presence rounds: a freshly started app showed it for a few
  seconds before the first probes answered.
- Mobile UI refresh: same palette as the desktop, native icons (expo-symbols: SF Symbols on iOS, Material
  Symbols on Android), device cards with one-tap Clipboard / Text / Files, grouped lists with state badges,
  Sent/Received tabs, camera frame for scanning, expiring QR card, success/error notices.
- Open: iPhone push (stop point 2), Windows autostart after restart and a ~1 GB file (owner), Android/iOS
  rebuild on the phones with the latest core, lint setup.

## 2026-11-19 — Phase 5: open-source packaging — done except making the repository public

- README (install, pairing, network troubleshooting), MIT LICENSE, SECURITY.md (what is protected, threat model,
  conscious trade-offs incl. Expo push), CONTRIBUTING.md, docs/release.md (release and TestFlight runbook).
- GitHub Actions: CI (Linux, Windows, mobile typecheck) and the tag-driven release pipeline with optional
  signing / notarization secrets and a draft release (D36). Both files pass `actionlint` 1.7.12. CI is green on
  GitHub (Linux, Windows, mobile typecheck + lint) after fixing what its first run found (decisions D36).
- `scripts/testflight.sh` for iPhone TestFlight builds (not run: needs the App Store Connect app record).
- Mobile icons now use the kitedrop mark (`scripts/make-mobile-icons.swift`). README rewritten in the layout
  of the owner's Shutterbar README with a logo and a hero image taken with test devices.
- Left for the owner: making the repository public; release secrets; App Store Connect app record and export
  compliance answer; iPhone push (Expo / APNs).

## 2026-11-20 — Progress bar; first release-pipeline run

- Large sends show progress (D37): desktop bar with percent, size, speed and time left; phones with their next
  install.
- `v0.1.0-dev.8`: the release workflow built the Windows installer, the macOS app and the Android APK on GitHub
  and drafted the release on its first run. Without signing secrets the CI macOS build is ad-hoc signed, so the
  locally Developer ID-signed macOS zip replaced it in the published pre-release.

## 2026-11-20 — macOS share menu, folders, phone reinstall

- macOS "Share → kitedrop" (share extension, D39, design v2.6): installed on the owner's Mac, registered by
  macOS (`pluginkit`), the `kitedrop://send` hand-over opens the send window (checked); the owner sent a 1.9 GB app
  bundle through it.
- Folders are sent as a .zip with progress in the send window and a clear chosen-device state (D38).
- Clicking a received-file notification shows the file in its folder (owner request; confirmed on the Mac).
- iPhone reinstalled with the new UI, icon and progress; its share sheet redesigned and now sends every shared
  item (D40; three files confirmed by the owner). Android APK built, waiting for the phone on USB.
- Releases `v0.1.0-dev.9` (draft, superseded) and `v0.1.0-dev.10` built by the release workflow.

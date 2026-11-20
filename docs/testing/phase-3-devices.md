# Phase 3 device tests (owner, 2026-11-19)

Exit criterion (plan Phase 3): basic flows work with four devices.

## Android (Samsung Galaxy S22, Android 16) ↔ Mac (MacBook Air, macOS 27)

| # | Flow | Result |
|---|---|---|
| 1 | Mac app installed (Developer ID signed), listening on 52525 | works |
| 2 | Android: Add a device → Scan the Mac's QR | paired |
| 3 | Mac → Android text with ⌘⇧K | delivered; notification on the phone; text in the phone clipboard (service wrote it) |
| 4 | Android: Gallery → Share → kitedrop → Mac | photo in `~/Downloads/kitedrop`, notification on the Mac |
| 5 | Android: Clipboard button → Mac | notification, text in the Mac clipboard |

Finding: with the first, linker-signed Mac build, every outgoing connection from the Mac app to the phone
failed although the Local Network toggle was on, while Terminal reached the phone. The app's code signature
identifier was not even the bundle id. Signed with the owner's Developer ID it worked at once (decisions D33).
The diagnostics wrongly said "AP isolation suspected" in that state; see D33.

## iPhone (iPhone 15 Pro, iOS 27) ↔ Mac ↔ Android

| # | Flow | Result |
|---|---|---|
| 6 | iPhone: Add a device → Scan the Mac's QR | paired |
| 7 | Mac → iPhone text | Mac shows "Waiting for iPhone"; after pull-to-refresh on the iPhone the text is in its clipboard and the Mac shows Delivered |
| 8 | iPhone: Photos → Share → kitedrop → Mac (share extension, background upload) | file on the Mac; iPhone Activity shows Delivered (receipt fetched after adopting the extension's record) — confirms the Keychain identity is shared with the extension |
| 9 | iPhone → Android (share extension) | photo in Downloads/kitedrop on the phone |
| 10 | Android → iPhone (outbox) | after refresh, photo in the iPhone's Photos; Android shows Delivered |

Findings fixed during the test: the share extension's device picker (an action sheet presented from the
extension) never appeared on iOS 27 — replaced by buttons in the extension's own view; the app never asked
for Photos add-only access, so received photos would always have gone to Files — it now asks once.

## Windows (Windows 11 PC on the same Wi-Fi; also has WSL/Hyper-V adapters and a VPN)

| # | Flow | Result |
|---|---|---|
| 11 | Install (NSIS, per machine) | works; tray icon |
| 12 | Diagnostics | first build warned "network too large" and "Public" because of the WSL/Hyper-V and VPN adapters — fixed (dev.2); the remaining Public warning was real (home Wi-Fi was Public), gone after setting it to Private |
| 13 | Pairing | "Add a device" froze the app (window created from a sync command) — fixed (dev.3); a group member scanning the PC's normal QR silently left its group — fixed (D34, dev.4); re-paired: all four devices listed |
| 14 | Mac → Windows text | works |
| 15 | Windows → Mac link (Ctrl+Shift+K), Send to → kitedrop → Mac | failed in dev.3 ("file could not be read": sync through a read-only handle), works in dev.4 |
| 16 | Android → Windows photo | works (Downloads\kitedrop, notification) |
| 17 | Windows offline → Mac send waits → Windows back → delivered | works |
| 18 | Start at login after a restart | pending (owner, next morning) |
| 19 | ~1 GB file | pending |

Owner request from this test: buttons on the desktop notifications (open, show in folder, copy).

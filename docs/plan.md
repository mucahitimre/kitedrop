# kitedrop — Plan v1.1

Every phase ends with a short report. Work continues into the next phase unless the phase end is a stop point.

## Phase 0a — Protocol and skeleton (S)

- Repo: `core/` (Rust), `desktop/` (Tauri), `mobile/` (Expo + native iOS/Android targets),
  `protocol/` (spec + test vectors), `docs/`.
- `docs/`: [design.md](design.md), plan.md, [protocol.md](protocol.md), [decisions.md](decisions.md)
  (decision log), [progress.md](progress.md) (progress log).
- protocol.md: identity, pairing, group list and versioning, envelope format, endpoints and
  challenge-response, receipt, outbox, push, error codes.
- Exit: protocol.md complete; test vectors generated; local CI scripts (fmt, lint, test) green.

## Phase 0b — Spikes, in parallel (S+S) — results shape the final protocol

- **Android spike:** `specialUse` service, screen off for at least 8 hours; authenticated probe from a desktop
  every 5 minutes; auto-start at boot; battery drain. Suggested bar: probe success ≥ 98%, after a phone
  reboot the service is up without opening the app. The owner installs and runs the overnight test on a
  physical device with step-by-step instructions; measurements are logged automatically.
- **iPhone spike:** Swift share extension + background `URLSession` uploads a ~1 GB video to a test server
  on the LAN. The extension exits immediately, the screen locks, the upload must complete. Measure the
  extension's peak memory and the throughput.
- **Build spike:** a simple function from the Rust core is callable through UniFFI from iOS (share extension
  + Expo native module) and Android (service + Expo native module).
- Exit: measured report of all three spikes. → **STOP POINT 1.**

## Phase 1 — Rust core (M)

- Identity/group, pairing, envelope, HTTP client/server, outbox, persistent queue/retry, receipt,
  discovery, network diagnostics.
- Exit: unit tests + test vectors pass; truncation/reorder/replay tests for chunked encryption; integration
  test between two processes; the core public API is frozen and documented.

## Phase 2 — Desktop (M)

- Tray, autostart, global-shortcut, notification, clipboard (official Tauri plugins), drop window, QR pairing
  screen, send-status list, Send to, firewall/profile check, sleep prevention.
- Exit: all flows work between two desktops on Mac and Windows.

## Phase 3 — Mobile (M–L)

- Android: service, share target, receive, pairing (QR scan), status list.
- iPhone: share extension, outbox pull, push registration and notification, pairing, status list,
  Local Network onboarding.
- UI in Expo; core through native modules.
- Exit: basic flows work with four devices.

## Phase 4 — End-to-end test (S)

- Matrix: 4 devices × all directions × content types (text, link, photo, 2 GB file, 100 small files).
- LocalSend scenarios: Wi-Fi off/on, IP change (DHCP renew), sender goes to sleep, app/service killed,
  phone reboots.
- Reuse the owner's existing adb automation on Android.
- Measure and report as a table: delivery success rate, delivery time p50/p95, time back to "online" after a
  network change, silent-failure count. Suggested targets: success ≥ 99%, small item p95 < 3 s while the
  target is online, network recovery < 10 s, silent failures = 0.

## Phase 5 — Open-source packaging (S)

- README (install, pairing, network troubleshooting), MIT LICENSE, SECURITY.md (threat model and conscious
  trade-offs, including Expo push), Actions release pipeline, TestFlight preparation.

## Stop points

Stop, report, wait for approval:

1. End of Phase 0b: spike results and, if needed, plan B (e.g. if Android listening does not hold up:
   push-to-wake + pull model) — product decision.
2. Before any external action: creating the GitHub repo, first push, making the repo public, TestFlight
   upload, any step that needs Apple/Expo/APNs/notarization credentials or secrets. Local git commits are free.
3. Any finding that requires changing a product/design decision in [design.md](design.md). Technical detail
   choices inside a decision are free; log them with reasons in [decisions.md](decisions.md).
4. Physical device steps: give the owner clear instructions and wait for the result.

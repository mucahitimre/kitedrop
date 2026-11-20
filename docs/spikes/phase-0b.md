# Phase 0b spikes — runbook and results

Goal and exit criteria: [plan.md](../plan.md#phase-0b--spikes-in-parallel-ss--results-shape-the-final-protocol).
Raw results go to `spike-results/` (gitignored: contains local addresses); summaries are copied here.

## What is built

| Piece | Where | Notes |
|---|---|---|
| Rust → mobile bindings | `ffi/` (UniFFI 0.32), `scripts/build-mobile-core.sh` | xcframework (device + arm64/x86_64 simulator) and Android `.so` (arm64-v8a, x86_64) |
| Android listener | `mobile/modules/kitedrop-native/android/` | `specialUse` foreground service hosting the Rust probe server on 52525; boot receiver; battery/lifecycle log |
| iOS share extension | `mobile/targets/share/` + `mobile/native/ios/KitedropCore/` | seals the shared item into an envelope file, hands it to a background `URLSession`, exits |
| Desktop side | `spike/` (`kitedrop-spike serve` / `probe`) | test server (probe + upload + receipt) and overnight prober |

## Results so far (automated, no physical device)

| Check | Result |
|---|---|
| iOS app + share extension build (simulator Debug/Release, device Release unsigned) | succeeds |
| Share extension binary links React / Expo / Hermes | **no** — only system libraries; the Rust core (UniFFI symbols, envelope code) is linked statically |
| Android: JS → Expo module → Kotlin → Rust (`coreInfo`) on the emulator (Android 16, API 36) | works (core 0.1.0 + fingerprint shown) |
| Android: service type | `specialUse` (`types=0x40000000`), foreground, ongoing notification |
| Android: signed probe from the Mac (via `adb forward`) | 100 % (latency 2–6 ms), fingerprint pinned and stable |
| Android: start at boot without opening the app (emulator reboot) | works (`boot_receiver,boot` → `service_start,reason=boot`) |
| Android: restart after app update | works (`reason=update`) |
| Android: forced deep Doze on the emulator | probes still succeed — **weak evidence**: `adb forward` uses loopback, not Wi-Fi; only the overnight device test counts |
| Spike server end-to-end (localhost test) | probe, register, 3 MiB upload, signed receipt, wrong-length request rejected (403) |
| iOS runtime on the simulator | **not verified** — the iOS 27 simulator would not launch apps in this environment; verified on the device instead |

Device results: iPhone and Android sections below.

## iPhone results (2026-11-19, iPhone 15 Pro, iOS 27.3, installed over Wi-Fi)

Run 1 — 348 MB video from Photos:

| Measure | Value |
|---|---|
| Extension footprint at open | 5.9 MB |
| **Extension peak footprint while sealing** | **9.7 MB** |
| Seal 348 MB into the app-group container | 0.92 s (~378 MB/s) |
| Extension handoff → first request byte at the server | 47 ms |
| Upload (background `URLSession`, Wi-Fi) | 27.5 s, 12.7 MB/s, HTTP 200, envelope verified, signed receipt |
| App relaunched for `handleEventsForBackgroundURLSession` | yes, 68 ms after the server finished |
| Background upload response body delivered | yes (354 bytes) in this run — still undocumented by Apple, so D8 stands |
| JS → Swift → Rust at runtime (pairing: probe verify, register) | works |

Run 1 note: the phone was locked 5–10 s after the sheet closed, so part of the upload ran unlocked.

Run 2 — 679 MB video, phone locked immediately after the sheet closed:

| Measure | Value |
|---|---|
| Extension peak footprint | **9.9 MB** (vs 9.7 MB for 348 MB: memory does not grow with file size) |
| Seal 679 MB | 1.64 s (~414 MB/s) |
| Extension open → handoff (copy from Photos is before `ext_open`) | 1.7 s |
| Handoff → first request byte at the server | 74 ms |
| Upload, entirely with the phone locked | 43.9 s, 15.5 MB/s, HTTP 200, verified, signed receipt |
| App relaunched for background-session events | yes, 58 ms after the server finished |

Verdict: the iPhone send path of design §8 holds — tiny extension memory, the background upload starts at once,
survives the extension exiting and the screen locking, and the app is woken with the result. The ~1 GB target
was not hit exactly (679 MB); memory is flat in file size and the locked upload lasted 44 s, so the conclusion
holds. "Preparing…" seen by the user before the sheet is Photos exporting the asset, not kitedrop.

Spike UI bug found: in dark mode the text was black on black; fixed by forcing the light style for the spike.

## Android results (2026-11-19, Samsung Galaxy S22, Android 16 / API 36, Wi-Fi)

Overnight: phone on battery, screen off, untouched; Mac sent a signed probe over Wi-Fi every 5 minutes.
In the morning the phone was restarted and kitedrop was not opened.

| Measure | Value |
|---|---|
| **Probes answered** | **92 / 92 (100 %)** over 7 h 43 min, including 3 after the restart |
| Probe latency (p50 / p95 / max) | 430 / 747 / 1552 ms (radio wake-up from sleep; 2–6 ms on the emulator) |
| Device in deep Doze ("full idling") | 6 h 17 min of 7 h 23 min on battery (85 %); CPU awake only 34 min in total |
| Start after restart without opening the app | yes: `boot_receiver,boot` → `service_start,reason=boot` within 0.1 s, probes answered |
| Battery, whole phone, unplugged period | 100 % → 97 % in ~5.9 h (138 mAh discharged over the full stats window) |
| kitedrop share (batterystats, screen off) | ~0.18 mAh (cpu 0.07 + Wi-Fi 0.11) for 7.5 h of foreground service |
| Battery-optimization exemption | granted by the owner; not tested without it |

Observation: the service's own 5-minute battery timer fired only every 30–90 minutes while the phone was in
Doze (the CPU sleeps), yet every incoming probe was answered — incoming packets wake the device. Timers in the
listener must not be relied on for liveness; incoming traffic is.

Verdict: the Android receive path of design §8 holds on this device — a `specialUse` foreground service with the
battery exemption keeps listening through deep Doze at negligible cost, and comes back after a restart without
user action. Not verified: other vendors' battery managers (only Samsung One UI tested); behaviour without the
battery exemption; Android 17 `ACCESS_LOCAL_NETWORK` (targetSdk 36 for now).

## Android overnight test (owner)

1. Phone and Mac on the same Wi-Fi. Developer options → USB debugging on. Connect USB.
2. `scripts/spike-android.sh install` → on the phone tap **START LISTENER**, allow notifications, allow the
   battery-optimization exemption.
3. `scripts/spike-android.sh prepare` → note the address; unplug the phone (on battery, so Doze can happen).
4. `scripts/spike-android.sh probe <address>` → leave running ≥ 8 h, phone screen off, untouched.
5. Morning: restart the phone, unlock it, do **not** open kitedrop, wait 10 minutes (the prober keeps running).
6. Ctrl+C the prober, plug USB, `scripts/spike-android.sh collect`.

Bar: probe success ≥ 98 %; after the restart the probes succeed without opening the app.

## iPhone upload test (owner)

1. `scripts/spike-ios.sh server` (Mac; allow incoming connections if macOS asks). Note the printed address.
2. iPhone connected over USB, Developer Mode on. `scripts/spike-ios.sh install` (signs with team 4SFTG4T5YD;
   Xcode may ask to register the device and create the app group).
3. In the app: allow Local Network, enter the Mac address, **Pair with test server**.
4. Photos → a ~1 GB video → Share → **kitedrop**. The sheet closes by itself. Lock the phone immediately.
5. Wait for `upload_done` in the server window (it prints MB/s).
6. Open the app → **Send log to test server** (extension memory peak and timings).

Bar: upload completes with the extension closed and the phone locked; peak extension memory well under the limit
(Apple publishes no number; field reports around 120 MB).

## Known spike shortcuts

See [decisions.md D17](../decisions.md). Phase 3 also fixes: share-extension activation rule (`TRUEPREDICATE`
is for development only), `pods.rb` copied into the extension bundle, the debug-dylib split in Release builds.

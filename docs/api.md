# kitedrop core API (frozen at the end of Phase 1)

This is the interface the desktop app (Tauri, Rust directly) and the mobile apps (UniFFI bindings, Phase 3) build
on. It is **frozen**: Phases 2–3 may add to it, but changing or removing anything below needs an entry in
[decisions.md](decisions.md). Rust docs: `cargo doc -p kitedrop-core --open`.

Wire protocol: [protocol.md](protocol.md). Reasons for the design: [decisions.md](decisions.md) D18–D26.

## Starting a device

```rust
let cfg = Config::new(data_dir, download_dir, "Alice's Mac", Platform::Macos);
let keys = DeviceKeys::from_secret_bytes(&sig_sk, &kem_sk)?; // from the OS secure store
let node = Node::start(cfg, keys, host)?;                   // host: Arc<dyn Host>
```

- First start creates a single-member group. State lives in `data_dir/kitedrop.db` (SQLite) and
  `data_dir/spool/` (sealed envelopes waiting to be sent).
- Keys are **not** stored by the core. On first run the app calls `DeviceKeys::generate`, stores
  `secret_bytes()` in Keychain / Keystore / Credential Manager, and passes them on every start (design §5).
- `Node` is cheap to clone. `shutdown()` stops background work; the next `start` resumes from the database.

### `Config`

| Field | Default (`Config::new`) | Meaning |
|---|---|---|
| `data_dir`, `download_dir` | given | state; received files (`Downloads/kitedrop`) |
| `device_name`, `platform` | given | shown to the other members |
| `listen` | `true` except iOS | run the HTTP listener |
| `port` | `None` = first free in 52525–52534 | `Some(0)` = any (tests) |
| `bind` | `0.0.0.0` | |
| `advertise` | `None` | override published addresses (tests on loopback) |
| `push_token` | `None` | iPhone Expo push token (also `set_push_token`) |
| `ttl_secs` | 7 days (max 30) | how long a send retries / an outbox item waits |
| `presence_interval_secs` | 60 | online check; use 300 on battery and on Android |
| `mdns`, `scan` | `true` | iOS: `mdns = false` (use Bonjour, see `add_discovered`) |
| `retry_schedule_secs` | 2, 5, 15, 30, 60, 120, 300 | backoff, ±20 % jitter, last value repeats |

## `Node` methods

| Method | Purpose |
|---|---|
| `device_id()`, `fingerprint()`, `port()`, `group_version()` | identity and listener info |
| `devices() -> Vec<DeviceInfo>` | all members incl. this device; built from the group list, never from discovery |
| `member_keys() -> Vec<PublicIdentity>` | members' public keys (added in Phase 3 for the iOS share extension) |
| `addresses(id) -> Vec<String>` | addresses to try for a member, most likely first (added in Phase 3, same reason) |
| `async send(targets, Content) -> Vec<TransferId>` | one transfer per target; persisted as `queued` before returning |
| `transfers(limit)`, `transfer(id)` | send history, newest first, with state and reason |
| `retry(id)`, `cancel(id)` | retry a waiting send now; give up (`failed(cancelled)`) |
| `received(limit)` | items stored on this device |
| `create_invite(reverse) -> String` | pairing QR URI, 60 s, one use; `reverse = true` for a device that cannot scan |
| `async pair(uri) -> DeviceId` | act on a scanned QR: join (normal) or add the device that shows it (reverse) |
| `async remove_device(id)`, `async rename_device(id, name)` | new group version, pushed to every member |
| `async poll_outboxes() -> u32` | iPhone: pull, store, send receipts; returns items stored |
| `refresh()` | network changed / app foregrounded: republish addresses, probe all, push group |
| `diagnostics() -> Vec<NetworkIssue>` | current network problems (also sent as an event) |
| `set_push_token(Option<String>)` | iPhone: Expo push token changed |
| `add_discovered(id, "ip:port")` | iPhone: Bonjour result; probed before it is trusted |
| `adopt_upload(&PreparedUpload)` | iPhone: register an upload started by the share extension |

`async` methods run on the node's own runtime and may be awaited from any executor.

## Types

- `Content`: `File { path, name: Option, mime: Option }` · `Text(String)` · `Link(String)` (text ≤ 8 MiB).
- `Transfer`: `transfer_id, target, kind, name, mime, size, state, reason, via_outbox, attempts,
  next_attempt_at, created_at, updated_at, expires_at`.
  `state`: `queued → sending → delivered | waiting | failed`. `reason` examples: `offline`, `network`,
  `expired`, `cancelled`, `removed_from_group`, `file_unreadable`, `rejected:<error code>`.
- `Received`: `sender, transfer_id, kind, name, size, location (files), at`.
- `DeviceInfo`: `device_id, fingerprint, name, platform, is_self, online, address, last_seen`.
- `NetworkIssue` (`code()`): `no_network`, `windows_public_profile`, `local_network_denied`,
  `different_subnet { device }`, `ap_isolation_suspected`, `subnet_too_large { prefix }`, `port_in_use`.
- `Error` (`code()`, `http_status()`, `is_transient()`): the wire error codes of protocol.md §14 plus `network`,
  `io`, `internal`.

## `Host` (implemented by each app)

| Callback | When |
|---|---|
| `deliver_text(kind, text, from) -> Result<(), String>` | text/link received; write clipboard. `Err` = not stored, no receipt |
| `deliver_file(path, header, from) -> Result<PathBuf, String>` | file already in `download_dir`; may move it (Photos) and return the final path |
| `https_post(url, json) -> Result<u16, String>` | Expo push (content-free) |
| `event(Event)` | `TransferChanged`, `Received`, `DevicesChanged`, `Presence`, `Paired`, `RemovedFromGroup`, `OutboxPending { count }`, `NetworkIssues`, `TransferProgress { transfer_id, progress }` |
| `platform_issues() -> Vec<NetworkIssue>` | Windows network profile, Local Network permission |

`TransferProgress` is sent for files of 1 MiB and more while they are encrypted (`Sealing`, bytes of the
content) and uploaded (`Uploading`, bytes of the envelope), at most every 200 ms per transfer and once at 100 %;
`Node::progress(tid)` returns the latest value while the send is in that phase. Not persisted.

Callbacks may arrive on any thread. Desktops hold a sleep assertion while `OutboxPending.count > 0` (design §9).

## iPhone share extension (no running node)

```rust
let up = prepare_upload(&keys, &target_identity, "192.168.1.20:52525", &header, file, &envelope_path)?;
// background URLSession: POST up.url, header Authorization: up.authorization, body = file at up.envelope
// write serde_json::to_string(&up) into the app group; the app later calls node.adopt_upload(&up)
```

`upload_authorization(keys, server, transfer_id, len, ts)` signs an upload for other callers.

## Lower-level modules

`envelope`, `pairing`, `signed`, `group`, `auth`, `wire` are public for test vectors and tools. Apps should not
need them beyond `ContentHeader`/`ContentKind` (`envelope`), `Platform` (`signed`) and `DeviceKeys`.

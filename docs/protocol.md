# kitedrop protocol v1

Status: v1, implemented by [core/](../core/) in Phase 1. Incompatible changes need a new version number.
Reference implementation: [core/](../core/). Test vectors: [protocol/vectors/](../protocol/vectors/).
Design constraints come from [design.md](design.md); reasons for choices are in [decisions.md](decisions.md).

The key words MUST, SHOULD and MAY are used as in RFC 2119.

## 1. Conventions

- **Integers** inside signed or encrypted inputs are unsigned 64-bit big-endian (`u64`).
- **frame(f1, f2, …)** = for each field, `u32 BE length || bytes`. Every signature input, HPKE `info` and
  record AAD is a `frame` whose first field is an ASCII domain label (`kitedrop/v1/...`). Two different
  field lists never produce the same bytes, and labels keep different message types apart.
- **JSON** is used for every control message (UTF-8, `application/json`). Binary values inside JSON are
  base64url without padding. Unknown JSON fields MUST be ignored.
- **Time** is Unix seconds. Devices are assumed to have roughly correct clocks (NTP); every window below is
  generous enough for normal skew.
- **Version:** every message carries `v: 1` (JSON) or a version byte (binary). A receiver MUST reject versions
  it does not know with `unsupported_version`.

## 2. Identity

Each device creates, once, two key pairs and stores the private halves in the platform secure store
(Keychain with an access group shared by the iOS app and its share extension, Android Keystore-wrapped
storage, macOS Keychain, Windows Credential Manager / DPAPI):

| Key | Algorithm | Use |
|---|---|---|
| `sig` | Ed25519 | group list, device record, receipts, request and probe signatures |
| `kem` | X25519 (HPKE DHKEM) | envelopes and pairing messages |

```
fingerprint = SHA-256(frame("kitedrop/v1/fingerprint", sig_pk, kem_pk))   32 bytes
device_id   = fingerprint[0..16]                                             16 bytes
```

`device_id` is a public routing identifier. Possessing it proves nothing; every endpoint that matters requires
a signature or an HPKE authentication by the key behind it (§7).

## 3. Group state

The group state of a device is: the **group list** (signed, versioned membership) plus one **device record**
per member (self-signed reachability data). The UI's device list is built only from the group list
(design §3): members never disappear because discovery missed them.

### 3.1 Signed objects

```json
{ "signer": "<device_id>", "body": "<base64url bytes>", "sig": "<base64url 64 bytes>" }
```

`sig = Ed25519(sig_sk, frame(domain, body))`. Verifiers check the signature over the exact `body` bytes and
only then parse `body` as JSON, so no canonical serialization is needed. Domains:
`kitedrop/v1/group`, `kitedrop/v1/device-record`, `kitedrop/v1/receipt`.

### 3.2 Group list (domain `kitedrop/v1/group`)

```json
{
  "v": 1,
  "group_id": "<16 bytes>",
  "version": 2,
  "prev": "<SHA-256 of the previous version's body; absent in version 1>",
  "members": [
    { "sig_pk": "…", "kem_pk": "…", "name": "Alice's Mac", "platform": "macos", "added_at": 1790000000 }
  ],
  "updated_at": 1790000060
}
```

`platform` ∈ `ios`, `android`, `macos`, `windows`.

- A device that is not in any group creates version 1 with itself as the only member and a random `group_id`.
- **Accepting a new version.** A device holding version N accepts candidate list L iff: the signature is valid;
  the signer is a member of version N; `L.group_id` matches; `L.version == N + 1`; and
  `L.prev == SHA-256(body of N)`. A device that is several versions behind fetches the chain
  (`GET /v1/group`, which returns all versions it has) and applies it one step at a time. A list signed by a
  device that the previous version had already removed therefore never applies.
- **Fork** (two members published different N+1): a version signed by a device that the other version
  removes never wins — otherwise a removed (for example stolen) device could sign a competing version and grind
  its hash. If each removes the other's signer, the version a device already holds stays. In every other case
  the version whose body has the lexicographically smaller SHA-256 wins everywhere. The losing change is
  re-applied as N+2 on top of the winner (D26, D27).
- **Join / remove / rename** each produce a new version. A removed device's key is rejected everywhere as soon
  as the new version is applied (design §5). A device that receives a valid list without itself shows
  "removed from group", deletes the group state and outboxes, and starts a new single-member group.
- **Anchor:** a device that joined starts its chain at the parent of the version it received while pairing
  (`AcceptPayload.parent`, §4), so a fork at that version still resolves; older history is never needed,
  because acceptance only looks at the parent (D20).
- **Propagation:** the author pushes the new version to every reachable member (`POST /v1/group`), and to a
  device it just removed so that one resets itself. `GET /v1/group` also answers former members (D22). Every probe
  response carries `group_version` (§7.1), so members that were offline notice and pull. The iPhone pulls at
  every outbox poll.

### 3.3 Device record (domain `kitedrop/v1/device-record`)

```json
{
  "v": 1, "device_id": "…", "seq": 7,
  "addresses": ["192.168.1.20:52525"],
  "push_token": "ExponentPushToken[…]",
  "updated_at": 1790000000
}
```

Signed by the device itself. Accepted iff the signer is a current member, `signer == device_id` and `seq` is
greater than the stored one. A device publishes a new record when its address set, port or push token
changes. `addresses` is empty for iPhones (they do not listen); `push_token` is present only on iPhones.
Records spread the same way as the group list (`POST /v1/group`, probe `record_seq`, pull).

Addresses and push token live in device records instead of the group list so that DHCP changes do not create
group versions and version conflicts (decision D6).

## 4. Pairing

### 4.1 QR code

The inviter (a group member that listens) creates a fresh 32-byte random `code`, valid for **60 seconds** and
**one use**, and shows:

```
kitedrop://pair?v=1&c=<base64url code>&f=<base64url inviter fingerprint>&a=192.168.1.20:52525,[fd00::20]:52525
```

The QR never contains a long-lived key (design §5). `psk_id = SHA-256(frame("kitedrop/v1/psk-id", code))[0..16]`.

### 4.2 Flow

1. The joiner scans the QR and sends `POST /v1/probe` to the listed addresses (first one that answers). It
   checks the probe signature and that `fingerprint(sig_pk, kem_pk)` equals `f` from the QR. This pins the
   inviter's keys.
2. The joiner sends `POST /v1/pair/join` with a **JoinMessage**:
   ```json
   { "psk_id": "…", "joiner_kem_pk": "…", "enc": "…", "ct": "…" }
   ```
   `ct` is an HPKE single-shot seal in **mode_auth_psk** from the joiner's `kem` key to the inviter's `kem`
   key, `psk = code`, `psk_id` as above, `info = frame("kitedrop/v1/pair-join", inviter_fingerprint)`,
   empty AAD. Plaintext (JoinRequest):
   ```json
   { "v": 1, "sig_pk": "…", "kem_pk": "…", "name": "Pixel", "platform": "android",
     "nonce": "<16 random bytes>", "sig": "<Ed25519 over frame('kitedrop/v1/pair-join-sig', inviter_fingerprint, psk_id, kem_pk, nonce)>" }
   ```
3. The inviter looks up a live, unused code by `psk_id`, opens the message (this proves the joiner knows the
   code and holds `kem_sk`), checks `kem_pk == joiner_kem_pk` and the signature (proves `sig_sk`), then marks
   the code used. It signs group list version N+1 with the joiner added.
4. The inviter answers with an **AcceptMessage** `{ "enc", "ct" }`: mode_auth_psk from the inviter's `kem`
   key to the joiner's, same PSK, `info = frame("kitedrop/v1/pair-accept", inviter_fingerprint,
   joiner_fingerprint)`. Plaintext (AcceptPayload):
   ```json
   { "v": 1, "nonce": "<joiner nonce echoed>", "group": <SignedObject>, "records": [<SignedObject>, …],
     "parent": <SignedObject, the version before "group"; absent when "group" is version 1> }
   ```
5. The joiner opens it (proves the peer holds the pinned inviter key and knows the code), checks the nonce,
   verifies the group list chain and stores it. It then publishes its own device record. The inviter pushes
   version N+1 to the other members.

A code is invalidated after use, after 60 s, or after 5 failed `pair/join` attempts. Unknown, expired and used
codes all answer `code_invalid` without telling them apart.

### 4.3 Why PSK and not PAKE

A PAKE (SPAKE2, CPace) exists to protect a **low-entropy** password from offline guessing. The QR carries
**256 bits** of entropy through a visual out-of-band channel, so an attacker who records the exchange cannot
guess the code, and RFC 9180's PSK requirement ("at least 32 bytes of entropy", §5.1.2) is met.
mode_auth_psk is a standard construction from the HPKE library we already depend on, and gives mutual
authentication in one round trip. The Rust PAKE crates are unaudited and would add a second protocol to
verify. If a typed short code is ever added (see open question in [decisions.md](decisions.md), D15), that
path MUST use a PAKE.

### 4.4 Platform notes

- **iPhone as inviter:** the iPhone does not listen in the background, but a foreground app can run a
  temporary `NWListener` while the QR screen is open. Accepting incoming TCP does not need Local Network
  permission (Apple TN3179). The listener MUST close when the app leaves the foreground (Apple TN2277).
  *Not verified on device yet (Phase 3).*
- **Reverse QR (D15):** a joining device that cannot scan (a desktop without a usable camera) shows a QR with
  the same fields — its own fingerprint and addresses — plus `r=1`. The new desktop listens anyway, so no extra
  transport is needed. With J = joiner (shows the QR) and M = member (scans it):
  1. M probes J's addresses and pins J's keys against `f`.
  2. M sends `POST /v1/pair/offer` to J: an **OfferMessage**, the same shape as a JoinMessage (field
     `joiner_kem_pk` carries M's key), sealed mode_auth_psk from M to J with `psk = code`,
     `info = frame("kitedrop/v1/pair-offer", J_fingerprint)`; plaintext as JoinRequest with M's name, platform,
     nonce and `sig` over `frame('kitedrop/v1/pair-offer-sig', J_fingerprint, psk_id, M kem_pk, nonce)`.
  3. J opens it (proves M saw the QR and holds its keys) and answers with a JoinMessage sealed **to M** exactly
     as in §4.2 step 2 (`info` and signature bind M's fingerprint).
  4. M opens it, checks the joiner's keys equal the pinned ones, adds J (new group version) and sends
     `POST /v1/pair/accept` to J: `{ "psk_id", "enc", "ct" }`, the AcceptMessage of §4.2 step 4 plus `psk_id`.
  5. J opens it with the nonce it chose in step 3 and adopts the group as in §4.2 step 5.
  The code is single-use on J; offer and accept must both arrive within its 60 s lifetime.
- **Who joins whom (D34):** a device whose group has other members never leaves it to join another group.
  When it scans a *normal* QR shown by a device that is alone (group version 1), it runs the offer flow above
  instead, so the lone device joins its group; the shower accepts an offer on a normal QR only while it is
  alone. If both devices are in groups with other members, pairing stops with the local error
  `already_in_group`.

## 5. Envelope

Every transfer — file, text or link, direct upload or outbox pull — is one envelope.

### 5.1 HPKE parameters

| Parameter | Value |
|---|---|
| Mode | `mode_auth` (0x02) — authenticates the sender's `kem` key |
| KEM | DHKEM(X25519, HKDF-SHA256) (0x0020) |
| KDF | HKDF-SHA256 (0x0001) |
| AEAD | ChaCha20-Poly1305 (0x0003) |
| info | `frame("kitedrop/v1/envelope", prefix)` (prefix below, 56 bytes) |

ChaCha20-Poly1305 is chosen because phones without AES instructions run it fast in software and it has no
timing pitfalls.

### 5.2 Layout

```
prefix (56 bytes, clear, authenticated through HPKE info)
  0   4  magic        "KTDP"
  4   1  version      0x01
  5   1  suite        0x01  (table above)
  6   2  reserved     0x0000
  8  16  sender       device_id
 24  16  recipient    device_id
 40  16  transfer_id  random, chosen by the sender
enc (32 bytes)        HPKE encapsulated key
records, repeated:
  1  type             0x01 HEADER | 0x02 CHUNK | 0x03 FINAL_CHUNK
  4  length           u32 BE, ciphertext length
  n  ciphertext       HPKE context Seal(plaintext, aad)
```

Record `i` (0-based) is sealed with the i-th use of the single HPKE sender context, so its nonce is
`base_nonce XOR i` (RFC 9180 §5.2) and the receiver MUST open records in order (§9.7.1).

```
aad(i, type) = frame("kitedrop/v1/record", transfer_id, u64 i, type)
```

- Record 0 is `HEADER`; records 1… are `CHUNK`, the last one is `FINAL_CHUNK`. There is always exactly one
  `FINAL_CHUNK`, even for empty content (then its plaintext is empty).
- Every `CHUNK` plaintext is exactly `chunk_size` bytes; `FINAL_CHUNK` has 0…`chunk_size` bytes.
- Senders SHOULD use `chunk_size = 65536`. Receivers MUST accept any power of two from 64 to 1 MiB (small
  sizes keep test vectors readable).

What this detects: **reordering** (index in AAD and in the nonce), **truncation** (no `FINAL_CHUNK` →
`truncated`; an early chunk relabelled `FINAL_CHUNK` fails AEAD because the type is in the AAD),
**extension** (any byte after `FINAL_CHUNK` → `trailing_data`), **tampering** of any byte including the
prefix (AEAD / HPKE info), **sender spoofing** (mode_auth).

### 5.3 Content header (record 0 plaintext, JSON, ≤ 16 KiB)

```json
{ "v": 1, "kind": "file", "name": "photo.jpg", "mime": "image/jpeg",
  "size": 200, "created_at": 1790000000, "chunk_size": 65536 }
```

- `kind` ∈ `file`, `text`, `link`. Text and links carry their UTF-8 content in the chunks like a file, with an
  empty `name` (`mime` `text/plain; charset=utf-8` or `text/uri-list`).
- `name`: ≤ 255 bytes, no `/`, `\`, NUL, not `.` or `..`; non-empty iff `kind == file`. Receivers still
  sanitize it for the target file system and never overwrite (add ` (2)` etc.).
- `size` MUST equal the total chunk plaintext length (`size_mismatch` otherwise).
- Text and links are at most 8 MiB (`record_too_large`); larger content is sent as a file (D24).
- All metadata is encrypted (design §6); only the routing fields of the prefix are visible on the network.

### 5.4 Receiver rules

1. Read the prefix. `recipient` MUST be this device (`wrong_recipient`). `sender` MUST be a current member
   (`unknown_sender`).
2. **Replay:** if `(sender, transfer_id)` is in the seen-cache, do not process the envelope again. For an
   upload, answer with the stored receipt (§8) so retries are idempotent. The seen-cache is persistent and
   keeps entries for 31 days.
3. Open records in order, streaming plaintext into a temporary file in the destination folder. Nothing is
   shown, copied to the clipboard or moved into place before the whole envelope verifies.
4. Reject (`expired`) if `created_at` is older than 30 days or more than 1 day in the future. Together with
   the 31-day cache this means an old envelope (for example a clipboard overwrite) can never be replayed.
5. Store durably (rename the file into place / write the clipboard / save to Photos), add
   `(sender, transfer_id)` to the seen-cache, then sign the receipt (§8).

## 6. Transport and endpoints

Plain HTTP/1.1 between LAN addresses (design §6: iOS background `URLSession` owns the connection, so no
custom handshake). Confidentiality and integrity come from the envelope; endpoint access from §7.

- IPv4 only in v1 (D23).
- Default TCP port **52525**. If busy, the next free port in 52525–52534; the actual port is in the device
  record and the mDNS SRV record.
- POST bodies MUST have `Content-Length` (no chunked transfer encoding); the length is signed (§7.2).
- JSON bodies are limited to 64 KiB; envelopes only by free disk space. A request body that delivers no data
  for 60 s is abandoned (the transfer stays retryable).

| Method and path | Auth | Request → response |
|---|---|---|
| `POST /v1/probe` | listener proves identity (§7.1) | `ProbeRequest` → `ProbeResponse` |
| `POST /v1/challenge` | none | `{}` → `{ "nonce": "<32 bytes>", "expires_in": 120 }` |
| `POST /v1/pair/join` | pairing PSK (§4) | `JoinMessage` → `AcceptMessage` |
| `POST /v1/pair/offer` | pairing PSK, reverse QR (§4.4) | `OfferMessage` → `JoinMessage` |
| `POST /v1/pair/accept` | pairing PSK, reverse QR (§4.4) | `{ psk_id, enc, ct }` → `{}` |
| `POST /v1/inbox` | signed, nonce = transfer_id | envelope bytes (`application/octet-stream`) → `{ "receipt": SignedObject }` |
| `GET /v1/receipts/{transfer_id}` | signed, challenge | → `{ "receipt": SignedObject }` or `not_found` |
| `GET /v1/outbox` | signed, challenge | → `{ "items": [{ "transfer_id", "size", "created_at" }], "group_version": N }` (caller's items only) |
| `GET /v1/outbox/{transfer_id}` | signed, challenge | → envelope bytes; `Range` supported for resume |
| `POST /v1/receipts` | signed, challenge | `SignedObject` (receipt) → `{}`; the outbox item is deleted |
| `GET /v1/group` | signed, challenge; current **or former** member (D22) | → `{ "versions": [SignedObject…], "records": [SignedObject…] }` (former members: `records` empty) |
| `POST /v1/group` | none — every version and record is verified by its signature chain (D21) | `{ "versions": [...], "records": [...] }` → `{ "group_version": N }` |

## 7. Endpoint authentication

A bare `device_id` is never enough (design §6).

### 7.1 Probe (listener proves identity)

The caller sends `{ "nonce": "<32 random bytes>" }`. The listener answers:

```json
{ "v": 1, "sig_pk": "…", "kem_pk": "…", "group_id": "…", "group_version": 2, "record_seq": 7, "ts": 1790000000,
  "sig": "<Ed25519 over frame('kitedrop/v1/probe', device_id, nonce, group_id, u64 group_version, u64 record_seq, u64 ts)>" }
```

The caller checks the signature over its own nonce and that the keys belong to the member it expected (or, while
pairing, match the QR fingerprint). A probe is cheap and is also the online check and the scan request
(§10). The response does not include the device name.

### 7.2 Signed requests (caller proves identity)

```
Authorization: KD1 device=<b64 device_id>,ts=<int>,nonce=<b64>,seq=<int>,sig=<b64>
sig = Ed25519(frame("kitedrop/v1/request", server_device_id, client_device_id, method, path_and_query,
                    u64 content_length, u64 ts, nonce, u64 seq))
```

The listener checks: the client is a current member (`not_member`); the signature (`bad_signature`); then the
nonce rule for the endpoint:

- **Challenge-bound** (all signed endpoints except `/v1/inbox`): `nonce` is a 32-byte value this listener
  issued through `/v1/challenge` less than 120 s ago (`challenge_invalid`). A challenge is bound to the first
  device that uses it, and may be used for several requests in a row with strictly increasing `seq`
  (`challenge_invalid` otherwise). One challenge covers a whole poll session (list, downloads, receipts).
- **Upload** (`/v1/inbox`): `nonce` is the 16-byte `transfer_id` of the envelope in the body and `seq = 0`;
  `|now − ts| ≤ 24 h` (`stale_request`). After reading the prefix, the listener checks that the envelope's
  `sender` and `transfer_id` equal the header's `device` and `nonce`. Replays of the same request hit the
  seen-cache (§5.4) and get the stored receipt; nothing is applied twice.

Why uploads are different: an iOS background upload is created by the share extension and may start minutes
later, run after the extension has exited, and cannot do a challenge round trip (decision D7). The signature
still binds the request to this listener, this sender, this transfer and this length.

## 8. Receipts (domain `kitedrop/v1/receipt`)

```json
{ "v": 1, "transfer_id": "…", "sender": "…", "recipient": "…", "status": "stored", "at": 1790000005 }
```

`status` is `stored`, or `rejected` with a `code` (§14) when an iPhone refuses an outbox item for good
(for example `expired`, `decrypt_failed`): the sender then marks the send `failed(rejected:<code>)` instead of
leaving it `waiting` until its TTL (D28). Direct uploads are refused with an HTTP error instead.

Signed by the recipient after step 5 of §5.4. `status: stored` means decrypted, verified and durably stored.
The sender verifies it with the recipient's key from the group list and only then marks the send `delivered`
(design §3).

- **Direct upload:** the receipt is the `/v1/inbox` response body.
- **iPhone as sender:** Apple does not document that a background upload delivers the response body, and
  developers report it does not (decision D8). The iPhone uses only the HTTP status of the background task and
  then fetches `GET /v1/receipts/{transfer_id}` — from the background-session completion handler if time allows,
  otherwise at the next app start. Until then the item stays `sending`.
- **iPhone as recipient:** after storing an item pulled from an outbox, it sends `POST /v1/receipts` to that
  sender.

## 9. Outbox (delivery to iPhone)

- A send to an iPhone is sealed to it and kept in the sender's persistent outbox (default TTL 7 days,
  configurable up to 30 days). Its state is `waiting` until a receipt arrives.
- The iPhone polls when the app opens and when a notification is tapped: for each member that has addresses,
  probe → challenge → `GET /v1/outbox` → `GET /v1/outbox/{id}` for each item → open and store →
  `POST /v1/receipts`. An item that fails for good is answered with a `rejected` receipt; one that fails for a
  passing reason (network, storage) is skipped until the next poll. One item never blocks the others. It also applies any newer group list or device records it sees.
- When the TTL passes without a receipt, the item is deleted and the send becomes `failed(expired)`.
- While its outbox is non-empty, a desktop holds a time-limited sleep assertion (Windows
  `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`, macOS `IOPMAssertionCreateWithName`). Closing
  the lid or pressing the power button still sleeps the machine (documented by Microsoft for the Windows API).

## 10. Push to iPhone

When an item enters the outbox for an iPhone that has a `push_token`, and the internet is reachable, the
sender sends:

```
POST https://exp.host/--/api/v2/push/send
{ "to": "<push_token>", "title": "kitedrop", "body": "New item from <sender device name>", "sound": "default" }
```

No file name, content, size or `data` field (design §9). At most one push per sender per 30 s
(coalescing). Expo "enhanced push security" (access token) stays off; without internet the push is skipped and
the item still waits in the outbox.

## 11. Send states and retry (local, not on the wire)

```
queued → sending → delivered
                 → waiting          (target offline and retry scheduled, or parked in an outbox for an iPhone)
                 → failed(reason)   (expired, removed_from_group, rejected:<error code>, file_unreadable, …)
```

- States are persisted before any network call and survive restarts (design §3).
- Retry with backoff 2 s, 5 s, 15 s, 30 s, 1 min, 2 min, then every 5 min (±20 % jitter), and immediately when
  the target is seen online again (probe, mDNS). Retries continue until the TTL; then `failed(expired)`.
- Errors that a retry cannot fix (`not_member`, `wrong_recipient`, `unsupported_version`, …) go straight to
  `failed` with the code as reason (`failed(rejected:<code>)`). No error is swallowed. Exception: on the first
  `not_member` the sender pushes its group state to that listener and retries once (D21).
- Before re-uploading after an earlier attempt, the sender asks `GET /v1/receipts/{transfer_id}`; the earlier
  attempt (or an iPhone background upload) may have arrived.
- Every retry of the same item reuses the same envelope and `transfer_id`, so a retry after a lost response
  is deduplicated by the recipient (§5.4).

## 12. Discovery and reachability

For each member, in order (design §7):

1. **Last known addresses** from its device record and from previous successful probes.
2. **mDNS** service `_kitedrop._tcp`, instance name = hex `device_id` (not the device name), TXT
   `v=1`, `id=<hex device_id>`.
3. **Unicast scan** of the local subnet sized by the interface's **real netmask**, capped at /22 (1,022 hosts;
   a wider network scans the /22 around the device's own address and reports `subnet_too_large`). 64 parallel
   probes, 500 ms connect timeout, at most once per 10 min per network.

A host counts as a member only after a valid probe signature (§7.1). The online indicator comes from periodic
probes: every 60 s on a desktop on AC power, every 5 min on battery and on Android, and immediately after a
network change. No multicast entitlement is needed on iOS: Bonjour through the Network framework only needs
`NSLocalNetworkUsageDescription` and `NSBonjourServices` (`_kitedrop._tcp`), placed in the containing app's
Info.plist, not the extension's (TN3179). ATS uses `NSAllowsLocalNetworking`.

## 13. Network diagnostics

Shown to the user in plain words (design §3); never silent.

| Code | Detection |
|---|---|
| `no_network` | no active non-loopback interface |
| `windows_public_profile` | Network List Manager `INetwork::GetCategory` returns Public for the active network |
| `local_network_denied` | iOS / macOS: connection fails with the Local Network policy error; Android 17+: `ACCESS_LOCAL_NETWORK` not granted |
| `different_subnet` | a member's last known address is outside every local subnet |
| `ap_isolation_suspected` | gateway answers, members are known, but neither mDNS nor a full scan finds any member while at least one member reports the same network (same gateway MAC or SSID seen earlier) |
| `subnet_too_large` | netmask wider than /22; scan limited |
| `port_in_use` | 52525–52534 all busy |

Heuristics are refined in Phase 1 and validated in Phase 4.

## 14. Error codes

Error responses: HTTP status + `{ "error": { "code": "<code>", "message": "<English, for logs>" } }`.

| Code | HTTP | Meaning |
|---|---|---|
| `malformed` | 400 | unparsable body or header |
| `unsupported_version` | 400 | unknown `v`, version byte or suite |
| `bad_magic` | 400 | body is not an envelope |
| `not_member` | 403 | client device is not in the current group list |
| `bad_signature` | 403 | request, probe, receipt or group signature invalid |
| `challenge_invalid` | 403 | challenge unknown, expired, bound to another device, or `seq` not increasing |
| `stale_request` | 403 | upload `ts` outside ±24 h |
| `code_invalid` | 403 | pairing code unknown, expired or used |
| `unknown_sender` | 403 | envelope sender not a member |
| `wrong_recipient` | 400 | envelope not addressed to this device |
| `decrypt_failed` | 400 | HPKE / AEAD failure (tampering, reordering, wrong key) |
| `truncated` | 400 | body ended before `FINAL_CHUNK` |
| `trailing_data` | 400 | bytes after `FINAL_CHUNK` |
| `record_too_large` | 400 | record longer than allowed |
| `size_mismatch` | 400 | content length differs from header `size` |
| `expired` | 400 | `created_at` outside the replay window |
| `group_conflict` | 409 | pushed group version does not chain onto the local one |
| `not_found` | 404 | unknown transfer id / outbox item |
| `storage_full` | 507 | not enough disk space to store the content |
| `rate_limited` | 429 | too many requests |
| `internal` | 500 | anything else (logged) |

## 15. Security notes

- **Trust model:** all members are the owner's devices and fully trusted; any member can add or remove
  members. A stolen unlocked device is handled by "remove device" from another member.
- **KCI:** HPKE auth mode is not resistant to key-compromise impersonation (RFC 9180 §9.1.1): whoever steals a
  device's `kem` private key can forge envelopes *to that device* that look as if they came from other members.
  That attacker already controls the device's received content, so this is accepted.
- **Network observers** see routing fields (device ids, transfer ids, sizes, timing), probe answers and the
  pairing QR's addresses — not names, content or metadata.
- **Expo push:** see design §9; the worst case is spam of empty notifications.
- **HPKE library** is unaudited (decision D2); it is checked against the official RFC 9180 vectors in CI.

## 16. Test vectors

`protocol/vectors/` (generated by `cargo run -p kitedrop-core --example gen_vectors`; `cargo test` fails
if they are stale):

| File | Content |
|---|---|
| `identities.json` | three test devices (A Mac, B Android, C iPhone): secrets, public keys, fingerprints, device ids |
| `envelope.json` | text, link, multi-chunk, exact-multiple and empty envelopes with their deterministic RNG seed; 11 negative cases (truncation, reordering, forged final flag, tampering, trailing data, sender spoofing, wrong / rewritten recipient, bad magic) |
| `signed.json` | group list v1/v2, device records, receipt; tampered / wrong-domain / wrong-signer cases |
| `pairing.json` | QR URI, psk_id, join and accept messages; reverse-QR offer/join/accept; wrong-code and tampered cases |
| `auth.json` | probe response, challenge-bound and upload request signatures; changed path / server / length cases |
| `rfc9180-subset.json` | official RFC 9180 vectors for this suite (auth and auth_psk), as an independent library check |

## 17. Settled by Phase 0b

Device results are in [spikes/phase-0b.md](spikes/phase-0b.md):

- An iOS background upload created by the share extension starts within ~50–75 ms, survives the extension
  exiting and the screen locking (679 MB in 44 s), and the extension's peak memory stays under 10 MB regardless
  of file size (§5, §7.2).
- An Android `specialUse` service answered 92/92 overnight probes, 85 % of the time in deep Doze, and starts
  again after a restart without the app being opened (§7.1, §12).
- UniFFI bindings work from the iOS share extension and the Android service without a JS runtime.

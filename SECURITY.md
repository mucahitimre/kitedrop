# Security

## Reporting a vulnerability

Please use GitHub's **private vulnerability reporting** for this repository (Security tab → *Report a
vulnerability*). Do not open a public issue for a security problem. Include the version, the platforms
involved and steps to reproduce. This is a hobby project maintained by one person: expect an answer within
a couple of weeks, not hours.

## What kitedrop is meant to protect

kitedrop moves files, text and links between devices that **one person owns**, on a local network. The group
of paired devices is the trust boundary.

- **Content and metadata of transfers.** Every transfer is an envelope sealed to the receiving device's key
  with HPKE (RFC 9180) in auth mode — DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305. The file
  name, MIME type, size and kind (file / text / link) are inside the encrypted header. Auth mode also proves
  which group member sent it. Large content is encrypted in chunks; truncation, reordering, a forged final
  chunk and trailing data are rejected ([protocol §5](docs/protocol.md)).
- **Who may talk to a device.** Requests carry signatures from a member's Ed25519 key, bound to the listener,
  the path and the body length; the listener proves its identity to callers through challenge-response
  ([protocol §7](docs/protocol.md)). A bare device id is never enough.
- **Replays.** A receiver remembers `(sender, transfer_id)` for 31 days and rejects envelopes older than 30
  days, so a captured envelope (for example one that overwrites the clipboard) cannot be played again.
- **Delivery claims.** *Delivered* is shown only after the receiver's signed receipt.
- **Pairing.** The QR code / pairing link carries a one-time 256-bit code valid for 60 seconds, the inviter's
  key fingerprint and its addresses — never a long-lived key. The code is the PSK of HPKE mode_auth_psk, which
  authenticates both sides in one round trip ([protocol §4](docs/protocol.md), why not a PAKE: §4.3).
- **Group membership.** The member list is a signed, versioned chain; device records are self-signed. A removed
  device cannot win a conflicting update ([decisions D27](docs/decisions.md)).
- **Keys at rest.** Desktop: macOS Keychain / Windows Credential Manager. iPhone: Keychain, shared with the
  share extension, `AfterFirstUnlockThisDeviceOnly`. Android: the identity file is encrypted with an AES-GCM key
  that lives in the Android Keystore.

## Threat model

| Attacker | Outcome |
|---|---|
| Someone else on the same Wi-Fi, passive | Sees that devices talk: IP addresses, ports, device ids and transfer ids (random routing values), envelope sizes and timing, mDNS announcements. Does not see content, names or file names. |
| Someone else on the same Wi-Fi, active | Can block or delay traffic (sends then wait and retry — they never claim *Delivered*). Cannot read, change, inject or replay transfers, and cannot join the group without a valid, unexpired pairing code. |
| Someone who sees a pairing link or QR within its 60 seconds | Can pair **one** device of their own into the group (the code then stops working for the real device, which notices). Send pairing links only through a channel you trust (decisions D31). |
| A removed device | Is rejected by every member once it has the new list version. It keeps whatever it already received. It may still read the list *versions* (not the device records) so that it learns it was removed (D22). |
| A stolen, unlocked device | Has everything that device has, can send to the group and could add or remove devices. Remove it from another device. |

## Known trade-offs (chosen consciously)

- **All members are fully trusted.** Any member can add or remove members, and items from members are accepted
  without a prompt (files are saved, text goes to the clipboard). This matches "my own devices"; it would not
  suit sharing with other people.
- **No forward secrecy.** Envelopes are sealed to a device's long-term X25519 key. Someone who records traffic
  and later steals that device's private key can decrypt what was sent to it.
- **Key-compromise impersonation.** With HPKE auth mode, whoever steals a device's private key can also forge
  envelopes *to that device* that look as if another member sent them (RFC 9180 §9.1.1). That attacker already
  controls the device, so this is accepted ([protocol §15](docs/protocol.md)).
- **Plain HTTP on the LAN.** There is no TLS; confidentiality and integrity come from the envelope and the
  request signatures. The routing fields listed above are visible.
- **The HPKE crate is not audited** (`hpke`, rozbb/rust-hpke). The official RFC 9180 test vectors for the exact
  suite and modes run in CI. Crate choices and their status: [decisions D2](docs/decisions.md).
- **iPhone push through Expo** (design §9). To wake the iPhone, a sender posts to Expo's push API
  (`exp.host`), which hands the notification to Apple. The notification says only "New item from *sender device
  name*" — no file name, content or size — so Expo and Apple learn that one of your devices sent something and
  that device's name. Expo's "enhanced push security" is off: anyone who learns an iPhone's push token could
  send it empty notifications; the token is shared only inside the group. Without internet no push is sent and
  the item still waits. (The iPhone app does not register for push yet; this applies once it does.)
- **Unsigned or self-signed builds.** The Windows installer is not code-signed (SmartScreen warns). The macOS
  app is signed with a Developer ID and notarized only when the release secrets are configured
  ([docs/release.md](docs/release.md)). Android APKs built without a release keystore carry the Expo template's
  debug signature, which is public: a different APK with the same signature could be installed over kitedrop.
  Install updates only from this repository's Releases page.
- **Local network exposure.** Desktop and Android listen on TCP 52525–52534 on the LAN. Every endpoint needs a
  member's request signature or a valid pairing code, except two: the probe, which reveals only the device's
  public identity, and group updates, whose content is itself signed by members (decisions D21). The Windows installer opens the port
  for **private** networks only.

- **`kitedrop://send` on macOS.** The share extension hands items to the app through this URL, and any other app
  or web page can open it too. It only fills the "Send with kitedrop" window; nothing is sent until you pick one
  of your devices there.

## Out of scope

Transfers over the internet, sharing with other people's devices, and protection against malware already
running on one of your devices.

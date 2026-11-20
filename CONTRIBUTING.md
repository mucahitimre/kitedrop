# Contributing

Thanks for looking. kitedrop is a small personal project; issues and focused pull requests are welcome.
For anything that changes how the product behaves or the wire protocol, open an issue first — the design in
[docs/design.md](docs/design.md) is deliberate, and decisions are recorded in [docs/decisions.md](docs/decisions.md).

## Repository layout

| Path | What |
|---|---|
| `core/` | Rust core: identity, group, pairing, envelope, HTTP server/client, outbox, retry, discovery ([docs/api.md](docs/api.md)) |
| `cli/` | `kitedrop daemon`: a headless device driven by stdin, used for tests and as a second device during development |
| `ffi/` | UniFFI bindings of the core for Swift and Kotlin |
| `desktop/` | Tauri v2 app for macOS and Windows ([desktop/README.md](desktop/README.md)) |
| `mobile/` | Expo app (UI) with native modules: `modules/kitedrop-native` (Kotlin service, Swift runtime), `targets/share` (iOS share extension) |
| `protocol/vectors/` | machine-readable test vectors; the core tests run against them |
| `docs/` | design, protocol, decisions, progress, test reports |
| `scripts/` | build and check scripts |

## Prerequisites

- Rust stable (1.89 or newer) with `rustfmt` and `clippy`.
- Desktop: [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) and `cargo install tauri-cli --version "^2" --locked`.
  The Windows installer can also be cross-built on a Mac (`scripts/build-desktop.sh windows`, see the script
  header for its tools).
- Mobile: Node 22.13+, then `npm ci` in `mobile/`.
  - Android: JDK 21, Android SDK with NDK 27.1.12297006, `rustup target add aarch64-linux-android x86_64-linux-android`,
    `cargo install cargo-ndk --locked`.
  - iOS (macOS only): Xcode, CocoaPods, `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios`,
    and your own Apple team: change `ios.appleTeamId` (and the bundle identifiers / app group if you are not the
    author) in `mobile/app.json`.

On macOS, `source scripts/mobile-env.sh` puts Node 22, JDK 21 and the Android tools on the path.

## Checks

Run before every commit; CI runs the same on Linux and Windows:

```sh
scripts/ci.sh                         # cargo fmt --check, clippy -D warnings, all tests
(cd mobile && npx tsc --noEmit && npx expo lint)   # mobile typecheck and lint
```

`cargo test` also fails when `protocol/vectors/*.json` is out of date. Regenerate them only for an intended
protocol change:

```sh
cargo run -p kitedrop-core --example gen_vectors
```

Two-device testing without phones: run two `kitedrop daemon` processes with different `--dir` folders
(`cli/`), or a debug desktop build with `KITEDROP_DEV_DIR` ([desktop/README.md](desktop/README.md)).

## Building

```sh
scripts/build-desktop.sh mac          # .app (+ zip); set APPLE_SIGNING_IDENTITY for a Developer ID signature
scripts/build-desktop.sh windows      # NSIS installer, cross-built from a Mac
scripts/mobile.sh android             # Rust core + release APK, installs on the connected phone
scripts/mobile.sh ios                 # Rust core + Release build, installs on the connected iPhone
```

`ios/` and `android/` inside `mobile/` are generated (Expo prebuild); change native settings through
`mobile/app.json` and config plugins, not by editing those folders.

## Rules

- Code, comments, commit messages and docs are in English.
- **No secrets in the repository**: no keys, certificates, keystores, passwords or tokens. Release signing uses
  GitHub secrets ([docs/release.md](docs/release.md)).
- Cryptography: only standard constructions from maintained crates. Never invent a primitive or a
  construction. A new crypto dependency needs a reasoned entry in [docs/decisions.md](docs/decisions.md).
- Platform behaviour (permissions, background limits, OS APIs) must be checked against official documentation;
  write "not verified" where that was not possible.
- Peer-supplied text in the desktop UI is set with `textContent` only.
- By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).

#!/usr/bin/env bash
# Builds the desktop installers on macOS: `mac` (.app + .dmg, ad-hoc signed) and `windows` (NSIS
# installer, cross-compiled with cargo-xwin; needs `brew install nsis llvm lld`, the
# x86_64-pc-windows-msvc target and `cargo install --locked cargo-xwin tauri-cli`). The Windows
# build downloads the Microsoft CRT/SDK through xwin, which accepts Microsoft's license (owner
# approved, decisions D30).
set -euo pipefail
cd "$(dirname "$0")/../desktop/src-tauri"
export PATH="/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$HOME/.cargo/bin:$PATH"

case "${1:-}" in
  mac)
    # Signing identity from the environment (never in the repo), e.g.
    #   APPLE_SIGNING_IDENTITY="Developer ID Application: NAME (TEAMID)" scripts/build-desktop.sh mac
    # Without it the app is only linker-signed, and macOS Local Network permission does not
    # apply reliably (seen 2026-11-19: outgoing LAN connections failed with the toggle on).
    # .dmg needs Finder automation (AppleScript) and is left to CI; zip the .app instead.
    # Share extension first: tauri.conf.json copies it into Contents/PlugIns (D39).
    ../../scripts/build-mac-share.sh
    cargo tauri build --bundles app
    out=../../target/release/bundle/macos
    rm -f "$out/kitedrop-macos.zip"
    ditto -c -k --keepParent "$out/kitedrop.app" "$out/kitedrop-macos.zip"
    echo "→ target/release/bundle/macos/kitedrop-macos.zip"
    ;;
  windows)
    cargo tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc
    echo "→ target/x86_64-pc-windows-msvc/release/bundle/nsis/"
    ;;
  *)
    echo "usage: $0 mac|windows" >&2
    exit 2
    ;;
esac

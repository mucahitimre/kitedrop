#!/usr/bin/env bash
# Builds the macOS share extension (desktop/macos-share, decisions D39) into
# target/mac-share/kitedrop-share.appex; tauri.conf.json copies it into the app's Contents/PlugIns.
#   ARCHS                   architectures, default the host's (CI: "arm64 x86_64" for a universal app)
#   APPLE_SIGNING_IDENTITY  signing identity; default "-" (ad hoc, for local test builds only)
# The extension is signed here with its own sandbox entitlement; Tauri signs the app around it
# without --deep, so this signature stays.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=desktop/macos-share
OUT=target/mac-share/kitedrop-share.appex
ARCHS=${ARCHS:-$(uname -m)}
IDENTITY=${APPLE_SIGNING_IDENTITY:--}
VERSION=$(python3 -c "import json; print(json.load(open('desktop/src-tauri/tauri.conf.json'))['version'])")

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
tmp=$(mktemp -d)
bins=()
for arch in $ARCHS; do
    # Same entry point and flags Xcode uses for app extensions.
    swiftc -O -target "$arch-apple-macos11.0" -application-extension -parse-as-library \
        -module-name KitedropShare -framework Cocoa -Xlinker -e -Xlinker _NSExtensionMain \
        "$SRC/ShareViewController.swift" -o "$tmp/kitedrop-share-$arch"
    bins+=("$tmp/kitedrop-share-$arch")
done
lipo -create "${bins[@]}" -output "$OUT/Contents/MacOS/kitedrop-share"
rm -rf "$tmp"

sed "s/__VERSION__/$VERSION/g" "$SRC/Info.plist" > "$OUT/Contents/Info.plist"
cp desktop/src-tauri/icons/icon.icns "$OUT/Contents/Resources/icon.icns"

flags=(--force --sign "$IDENTITY" --entitlements "$SRC/share.entitlements" --options runtime)
[ "$IDENTITY" != "-" ] && flags+=(--timestamp)
codesign "${flags[@]}" "$OUT"
echo "→ $OUT ($ARCHS, signed by ${IDENTITY})"

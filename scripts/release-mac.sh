#!/usr/bin/env bash
# Local macOS release build: universal (Apple silicon + Intel), Developer ID signed, notarized and stapled,
# as .dmg and .zip in dist/ (used while the GitHub release workflow has no Apple secrets; docs/release.md).
#   APPLE_SIGNING_IDENTITY  "Developer ID Application: NAME (TEAMID)"            (required)
#   NOTARY_PROFILE          notarytool keychain profile (`xcrun notarytool store-credentials`); without it
#                           the build is signed but not notarized
#   VERSION                 file name suffix, default v<version from tauri.conf.json>
# No credentials are read from the repository; the profile lives in the login keychain.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY}"
VERSION=${VERSION:-v$(python3 -c "import json; print(json.load(open('desktop/src-tauri/tauri.conf.json'))['version'])")}
OUT=dist
APP=target/universal-apple-darwin/release/bundle/macos/kitedrop.app
export PATH="$HOME/.cargo/bin:$PATH"

notarize() { # $1 file (.zip or .dmg)
    [ -n "${NOTARY_PROFILE:-}" ] || return 0
    xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 30m
}

ARCHS="arm64 x86_64" scripts/build-mac-share.sh
(cd desktop/src-tauri && cargo tauri build --target universal-apple-darwin --bundles app)
codesign --verify --deep --strict "$APP"

rm -rf "$OUT" && mkdir -p "$OUT"
zip="$OUT/kitedrop-$VERSION-macos-universal.zip"
ditto -c -k --keepParent "$APP" "$zip"
if [ -n "${NOTARY_PROFILE:-}" ]; then
    notarize "$zip"
    xcrun stapler staple "$APP"
    rm "$zip" && ditto -c -k --keepParent "$APP" "$zip"   # zip again with the ticket stapled
fi

stage=$(mktemp -d)
ditto "$APP" "$stage/kitedrop.app"
ln -s /Applications "$stage/Applications"
dmg="$OUT/kitedrop-$VERSION-macos-universal.dmg"
hdiutil create -quiet -volname kitedrop -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"
codesign --sign "$APPLE_SIGNING_IDENTITY" --timestamp "$dmg"
if [ -n "${NOTARY_PROFILE:-}" ]; then
    notarize "$dmg"
    xcrun stapler staple "$dmg"
fi

spctl -a -vv -t exec "$APP" 2>&1 | sed 's/^/gatekeeper: /'
(cd "$OUT" && shasum -a 256 ./*)

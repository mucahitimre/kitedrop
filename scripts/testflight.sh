#!/usr/bin/env bash
# TestFlight build of the iPhone app and its share extension (design §10: TestFlight, owner only).
#   scripts/testflight.sh            archive + export an App Store Connect package to build/testflight/
#   scripts/testflight.sh --upload   archive + upload to App Store Connect through the Apple account that is
#                                    signed in to Xcode (Settings → Accounts). No keys or passwords are read
#                                    from the repo or the environment. One-time setup: docs/release.md.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/mobile-env.sh

TEAM=${APPLE_TEAM_ID:-$(node -p "require('./mobile/app.json').expo.ios.appleTeamId")}
DEST=export
[ "${1:-}" = --upload ] && DEST=upload
OUT=${OUT:-build/testflight}
rm -rf "$OUT"
mkdir -p "$OUT"

./scripts/build-mobile-core.sh ios
(cd mobile && CI=1 npx expo prebuild --platform ios)

xcodebuild -workspace mobile/ios/kitedrop.xcworkspace -scheme kitedrop -configuration Release \
    -destination generic/platform=iOS -archivePath "$OUT/kitedrop.xcarchive" \
    -allowProvisioningUpdates DEVELOPMENT_TEAM="$TEAM" -quiet archive

# Keys: `xcodebuild -help` ("Available keys for -exportOptionsPlist").
cat > "$OUT/ExportOptions.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key><string>app-store-connect</string>
  <key>destination</key><string>$DEST</string>
  <key>teamID</key><string>$TEAM</string>
  <key>signingStyle</key><string>automatic</string>
  <key>testFlightInternalTestingOnly</key><true/>
  <key>manageAppVersionAndBuildNumber</key><true/>
</dict>
</plist>
EOF

xcodebuild -exportArchive -archivePath "$OUT/kitedrop.xcarchive" -exportPath "$OUT" \
    -exportOptionsPlist "$OUT/ExportOptions.plist" -allowProvisioningUpdates -quiet

if [ "$DEST" = upload ]; then
    echo "→ uploaded; the build appears in App Store Connect → TestFlight after processing"
else
    echo "→ $OUT/kitedrop.ipa (not uploaded; run with --upload to send it)"
fi

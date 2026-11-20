#!/usr/bin/env bash
# Build and install the mobile apps from the Mac.
#   android   Rust core + release APK (arm64), install on the USB/wireless-connected phone, open it
#   ios       Rust core + Release build of app and share extension, install on the connected iPhone
#             (signing team 4SFTG4T5YD; Xcode may ask to register the device)
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/mobile-env.sh

case "${1:-}" in
android)
    ./scripts/build-mobile-core.sh android
    (cd mobile && CI=1 npx expo prebuild --platform android)
    (cd mobile/android && ./gradlew -q :app:assembleRelease -PreactNativeArchitectures=arm64-v8a)
    adb install -r mobile/android/app/build/outputs/apk/release/app-release.apk
    adb shell am start -n com.mucahitimre.kitedrop/.MainActivity >/dev/null
    ;;
ios)
    ./scripts/build-mobile-core.sh ios
    cd mobile && npx expo run:ios --device --configuration Release
    ;;
*)
    sed -n 2,5p "$0"; exit 2 ;;
esac

#!/usr/bin/env bash
# Builds kitedrop-ffi for mobile and generates the UniFFI bindings.
#   scripts/build-mobile-core.sh ios       -> mobile/native/ios/KitedropCore/{KitedropFFI.xcframework,Sources/Generated/}
#   scripts/build-mobile-core.sh android   -> mobile/modules/kitedrop-native/android/src/main/{jniLibs,java/uniffi}
# Outputs are generated (gitignored). PROFILE=debug for faster local builds.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
PROFILE=${PROFILE:-release}
FLAG=$([ "$PROFILE" = release ] && echo --release || true)

bindgen() { # $1 language, $2 out dir
    cargo build -q -p kitedrop-ffi $FLAG
    local ext; ext=$([ "$(uname)" = Darwin ] && echo dylib || echo so)
    local lib="target/$PROFILE/libkitedrop_ffi.$ext"
    cargo run -q -p kitedrop-ffi --features bindgen --bin uniffi-bindgen -- \
        generate --library "$lib" --language "$1" --out-dir "$2" --no-format
}

ios() {
    local out="$ROOT/mobile/native/ios/KitedropCore"
    local tmp; tmp=$(mktemp -d)
    for t in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
        IPHONEOS_DEPLOYMENT_TARGET=16.4 cargo build -q -p kitedrop-ffi --lib $FLAG --target "$t"
    done
    bindgen swift "$tmp/gen"
    mkdir -p "$tmp/headers" "$tmp/sim" "$out/Sources/Generated"
    cp "$tmp/gen/kitedrop_ffiFFI.h" "$tmp/headers/"
    cp "$tmp/gen/kitedrop_ffiFFI.modulemap" "$tmp/headers/module.modulemap"
    cp "$tmp/gen/kitedrop_ffi.swift" "$out/Sources/Generated/"
    # Simulator slice: arm64 + x86_64 in one fat library.
    lipo -create "target/aarch64-apple-ios-sim/$PROFILE/libkitedrop_ffi.a" "target/x86_64-apple-ios/$PROFILE/libkitedrop_ffi.a" \
        -output "$tmp/sim/libkitedrop_ffi.a"
    rm -rf "$out/KitedropFFI.xcframework"
    xcodebuild -create-xcframework \
        -library "target/aarch64-apple-ios/$PROFILE/libkitedrop_ffi.a" -headers "$tmp/headers" \
        -library "$tmp/sim/libkitedrop_ffi.a" -headers "$tmp/headers" \
        -output "$out/KitedropFFI.xcframework" >/dev/null
    rm -rf "$tmp"
    echo "ios: $out"
}

android() {
    local mod="$ROOT/mobile/modules/kitedrop-native/android/src/main"
    export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/27.1.12297006}
    cargo ndk -t arm64-v8a -t x86_64 --platform 24 -o "$mod/jniLibs" build -q -p kitedrop-ffi --lib $FLAG
    bindgen kotlin "$mod/java"
    echo "android: $mod"
}

case "${1:-}" in
    ios) ios ;;
    android) android ;;
    all) ios; android ;;
    *) echo "usage: $0 ios|android|all" >&2; exit 2 ;;
esac

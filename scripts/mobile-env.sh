# Source before mobile work on macOS: `source scripts/mobile-env.sh`
# Node >= 22.13 (Expo SDK 58), JDK 21 (Android Gradle Plugin), Android SDK in the default location.
if command -v brew >/dev/null; then
    export PATH="$(brew --prefix node@22)/bin:$PATH"
    export JAVA_HOME="$(brew --prefix openjdk@21)/libexec/openjdk.jdk/Contents/Home"
    export PATH="$JAVA_HOME/bin:$PATH"
fi
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export PATH="$ANDROID_HOME/platform-tools:$PATH"

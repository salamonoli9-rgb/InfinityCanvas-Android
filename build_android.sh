#!/usr/bin/env bash
# Builds the APK and installs it on the connected phone.
#
#   ./build_android.sh            release build + install
#   ./build_android.sh --debug    faster debug build + install
#   ./build_android.sh --no-install
#   ./build_android.sh --no-start  install, but don't bring the app to the front
#   ./build_android.sh --to-files  also copy the APK to the phone's Download folder
#                                  (Files app → Downloads → Nieskonczone-plotno.apk)
#
# Needs: Android SDK (build-tools, platform android-36, NDK), cargo-ndk, Rust target
# aarch64-linux-android, Java runtime (for apksigner/keytool). No Gradle, no JDK:
# the app is pure native (NativeActivity), so the APK is assembled with aapt2 directly.
set -euo pipefail
cd "$(dirname "$0")"

PROFILE=release
INSTALL=1
START=1
TO_FILES=0
for a in "$@"; do
    case "$a" in
        --debug) PROFILE=debug ;;
        --no-install) INSTALL=0 ;;
        --no-start) START=0 ;;
        --to-files) TO_FILES=1 ;;
        *) echo "unknown option: $a" >&2; exit 2 ;;
    esac
done

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
NDK_DIR=$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$NDK_DIR}"
BT=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
PLATFORM_JAR=$(ls -d "$ANDROID_HOME"/platforms/android-*/android.jar | sort -V | tail -1)
ADB="$ANDROID_HOME/platform-tools/adb"
MIN_SDK=26
TARGET_SDK=35

for tool in "$BT/aapt2" "$BT/zipalign" "$BT/apksigner" "$PLATFORM_JAR" "$ANDROID_NDK_HOME"; do
    [[ -e "$tool" ]] || { echo "missing: $tool" >&2; exit 1; }
done
command -v cargo-ndk >/dev/null || { echo "missing cargo-ndk: cargo install cargo-ndk" >&2; exit 1; }

OUT=target/android
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE/lib/arm64-v8a"

echo "==> cargo ndk ($PROFILE)"
CARGO_FLAGS=()
[[ $PROFILE == release ]] && CARGO_FLAGS+=(--release)
cargo ndk -t arm64-v8a --platform "$MIN_SDK" build -p infinite_canvas --lib "${CARGO_FLAGS[@]}"
cp "target/aarch64-linux-android/$PROFILE/libinfinite_canvas.so" "$STAGE/lib/arm64-v8a/"

echo "==> aapt2"
"$BT/aapt2" compile --dir android/res -o "$OUT/res.zip"
"$BT/aapt2" link -o "$OUT/unaligned.apk" \
    --manifest android/AndroidManifest.xml \
    -I "$PLATFORM_JAR" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
    "$OUT/res.zip"

# Native libraries stored uncompressed (extractNativeLibs=false) and 16 KB aligned.
(cd "$STAGE" && zip -q -0 -r "../unaligned.apk" lib)
"$BT/zipalign" -f -P 16 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"

KEYSTORE="$HOME/.android/debug.keystore"
if [[ ! -f "$KEYSTORE" ]]; then
    echo "==> creating debug keystore"
    mkdir -p "$(dirname "$KEYSTORE")"
    keytool -genkeypair -keystore "$KEYSTORE" -storepass android -keypass android \
        -alias androiddebugkey -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=Android Debug,O=Android,C=US" >/dev/null
fi
APK="$OUT/infinite-canvas.apk"
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass pass:android --key-pass pass:android \
    --out "$APK" "$OUT/aligned.apk"
rm -f "$OUT/unaligned.apk" "$OUT/aligned.apk" "$APK.idsig"
echo "==> built $APK ($(du -h "$APK" | cut -f1))"

if [[ $INSTALL == 1 ]]; then
    if ! "$ADB" get-state >/dev/null 2>&1; then
        echo "No phone connected (adb devices is empty). Connect it with USB debugging enabled." >&2
        exit 1
    fi
    echo "==> adb install"
    "$ADB" install -r "$APK"
    if [[ $START == 1 ]]; then
        "$ADB" shell am start -n dev.infinitecanvas.app/android.app.NativeActivity >/dev/null
        echo "==> started. Logs: $ADB logcat -s infinite_canvas:V"
    fi
fi

if [[ $TO_FILES == 1 ]]; then
    "$ADB" push "$APK" /sdcard/Download/Nieskonczone-plotno.apk >/dev/null
    "$ADB" shell am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE \
        -d file:///sdcard/Download/Nieskonczone-plotno.apk >/dev/null 2>&1 || true
    echo "==> copied to the phone: Download/Nieskonczone-plotno.apk"
fi

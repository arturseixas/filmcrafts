#!/usr/bin/env bash
# Build the FilmCraft APK: libfilmcraft.so for each ABI (cargo, NDK clang as the linker), the
# activity (javac + d8), resources and manifest (aapt2), then zipalign and apksigner. No Gradle:
# the only inputs are the Android SDK (build-tools, a platform) and NDK.
#
#   apps/filmcraft-android/build.sh [--debug] [--abis arm64-v8a,x86_64] [--out dist/android]
#
# Environment:
#   ANDROID_HOME (or ANDROID_SDK_ROOT)  the SDK; build-tools 35+ and platforms/android-35+
#   ANDROID_NDK_HOME (or ANDROID_NDK_ROOT, else the newest $ANDROID_HOME/ndk/*)  NDK r27+
#   FILMCRAFT_KEYSTORE, FILMCRAFT_KEYSTORE_PASSWORD, FILMCRAFT_KEY_ALIAS, FILMCRAFT_KEY_PASSWORD
#       release signing key; without one, the APK is signed with a debug key
#       (~/.android/debug.keystore, created if missing), which installs fine but can't be
#       updated by an APK signed with another key.
#   CRAFT_FONTS_DIR  optional, as for the desktop app (Japanese UI and title fonts)
set -euo pipefail

MIN_SDK=28
TARGET_SDK=35
PACKAGE=ai.storyteller.filmcraft

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
profile=android-release
abis="arm64-v8a"
out="$root/dist/android"

while [ $# -gt 0 ]; do
    case "$1" in
        --debug) profile=dev ;;
        --abis) abis="$2"; shift ;;
        --out) out="$2"; shift ;;
        -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
        *) echo "build.sh: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

die() { echo "build.sh: $*" >&2; exit 1; }
# the last of the arguments in version order (empty without any)
newest() { [ $# -gt 0 ] && printf '%s\n' "$@" | sort -V | tail -n1; }
shopt -s nullglob

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$sdk" ] && [ -d "$sdk" ] || die "set ANDROID_HOME to the Android SDK"
ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [ -z "$ndk" ]; then
    ndk="$(newest "$sdk"/ndk/*/ || true)"
fi
[ -n "$ndk" ] && [ -d "$ndk" ] || die "set ANDROID_NDK_HOME to the Android NDK"
ndk="${ndk%/}"
build_tools="$(newest "$sdk"/build-tools/*/ || true)"
[ -n "$build_tools" ] || die "no build-tools in $sdk"
build_tools="${build_tools%/}"
platform_jar=""
while IFS= read -r p; do
    level="$(basename "$p" | sed 's/android-//; s/\..*//')"
    if [ "$level" -ge "$TARGET_SDK" ] 2>/dev/null && [ -f "$p/android.jar" ]; then
        platform_jar="$p/android.jar"
        break
    fi
done < <(printf '%s\n' "$sdk"/platforms/android-*/ | sort -V -r)
[ -n "$platform_jar" ] || die "no platforms/android-$TARGET_SDK (or newer) in $sdk"

host_tag=linux-x86_64
case "$(uname -s)" in
    Darwin) host_tag=darwin-x86_64 ;;
esac
toolchain="$ndk/toolchains/llvm/prebuilt/$host_tag/bin"
[ -d "$toolchain" ] || die "no NDK LLVM toolchain at $toolchain"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)"
IFS=. read -r v_major v_minor v_patch <<<"${version%%-*}"
version_code=$(( ${v_major:-0} * 1000000 + ${v_minor:-0} * 1000 + ${v_patch:-0} ))

work="$root/target/android-apk"
rm -rf "$work"
mkdir -p "$work/stage" "$work/classes" "$work/res" "$out"

# --------------------------------------------------------------------------- native libraries
IFS=, read -r -a abi_list <<<"$abis"
for abi in "${abi_list[@]}"; do
    case "$abi" in
        arm64-v8a) triple=aarch64-linux-android ;;
        x86_64) triple=x86_64-linux-android ;;
        armeabi-v7a) triple=armv7-linux-androideabi ;;
        *) die "unknown ABI $abi (arm64-v8a, x86_64, armeabi-v7a)" ;;
    esac
    clang_triple="$triple"
    [ "$abi" = armeabi-v7a ] && clang_triple=armv7a-linux-androideabi
    env_triple="$(echo "$triple" | tr '[:lower:]-' '[:upper:]_')"
    lower_triple="$(echo "$triple" | tr '-' '_')"
    rustup target add "$triple" >/dev/null 2>&1 || true
    echo "==> libfilmcraft.so for $abi ($triple, $profile)"
    # 16 KB-aligned segments: required on Android 15+ devices with 16 KB pages. RUSTFLAGS would
    # replace the target's flags, so it is folded into them.
    env -u RUSTFLAGS \
        "CARGO_TARGET_${env_triple}_LINKER=$toolchain/${clang_triple}${MIN_SDK}-clang" \
        "CARGO_TARGET_${env_triple}_RUSTFLAGS=${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384" \
        "CC_${lower_triple}=$toolchain/${clang_triple}${MIN_SDK}-clang" \
        "CXX_${lower_triple}=$toolchain/${clang_triple}${MIN_SDK}-clang++" \
        "AR_${lower_triple}=$toolchain/llvm-ar" \
        cargo build --manifest-path "$root/Cargo.toml" -p filmcraft-android --target "$triple" --profile "$profile"
    dir="$profile"
    [ "$profile" = dev ] && dir=debug
    mkdir -p "$work/stage/lib/$abi"
    cp "$root/target/$triple/$dir/libfilmcraft.so" "$work/stage/lib/$abi/"
done

# ------------------------------------------------------------------------------------ activity
echo "==> FilmcraftActivity (javac, d8)"
find "$here/java" -name '*.java' > "$work/sources.txt"
javac -nowarn -Xlint:-options -source 11 -target 11 -encoding UTF-8 -classpath "$platform_jar" -d "$work/classes" @"$work/sources.txt"
find "$work/classes" -name '*.class' > "$work/classes.txt"
"$build_tools/d8" --release --min-api "$MIN_SDK" --lib "$platform_jar" --output "$work/stage" @"$work/classes.txt"

# ------------------------------------------------------------------- resources and manifest
echo "==> resources (aapt2)"
cp -r "$here/res/." "$work/res/"
# the launcher icon from the app icon renders (assets/app-icon, see its README and sidecars)
icons="$root/assets/app-icon/hicolor"
for pair in mdpi:48 hdpi:64 xhdpi:128 xxhdpi:256 xxxhdpi:512; do
    density="${pair%%:*}"
    size="${pair##*:}"
    mkdir -p "$work/res/mipmap-$density"
    cp "$icons/${size}x${size}/apps/$PACKAGE.png" "$work/res/mipmap-$density/ic_launcher.png"
done
"$build_tools/aapt2" compile --dir "$work/res" -o "$work/res.zip"
"$build_tools/aapt2" link -o "$work/unsigned.apk" -I "$platform_jar" \
    --manifest "$here/AndroidManifest.xml" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
    --version-code "$version_code" --version-name "$version" \
    "$work/res.zip"

# ----------------------------------------------------------------------------------- package
echo "==> package"
(
    cd "$work/stage"
    zip -q -X "$work/unsigned.apk" classes.dex
    # stored, not compressed: the libraries are mapped straight from the APK (extractNativeLibs=false)
    zip -q -X -0 -r "$work/unsigned.apk" lib
)
"$build_tools/zipalign" -f -P 16 4 "$work/unsigned.apk" "$work/aligned.apk"

if [ -n "${FILMCRAFT_KEYSTORE:-}" ]; then
    ks="$FILMCRAFT_KEYSTORE"
    ks_pass="${FILMCRAFT_KEYSTORE_PASSWORD:?set FILMCRAFT_KEYSTORE_PASSWORD}"
    alias="${FILMCRAFT_KEY_ALIAS:?set FILMCRAFT_KEY_ALIAS}"
    key_pass="${FILMCRAFT_KEY_PASSWORD:-$ks_pass}"
else
    ks="$HOME/.android/debug.keystore"
    ks_pass=android
    alias=androiddebugkey
    key_pass=android
    if [ ! -f "$ks" ]; then
        mkdir -p "$(dirname "$ks")"
        keytool -genkeypair -keystore "$ks" -storepass android -keypass android -alias androiddebugkey \
            -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >/dev/null
    fi
    echo "    (signed with the debug key $ks)"
fi
name="FilmCraft-$version-$(echo "$abis" | tr ',' '-')"
[ "$profile" = dev ] && name="$name-debug"
apk="$out/$name.apk"
"$build_tools/apksigner" sign --ks "$ks" --ks-pass "pass:$ks_pass" --ks-key-alias "$alias" --key-pass "pass:$key_pass" --out "$apk" "$work/aligned.apk"
"$build_tools/apksigner" verify "$apk"
rm -f "$apk.idsig"
echo "==> $apk ($(du -h "$apk" | cut -f1))"

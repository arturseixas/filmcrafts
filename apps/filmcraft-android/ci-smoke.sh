#!/usr/bin/env bash
# Smoke test on a running emulator or device (adb): install the APK in <apk dir>, start FilmCraft,
# wait for it to draw, then keep a screenshot and logcat in <out dir>. Fails when the app is not
# running afterwards or logcat shows a panic or a native crash.
#
#   apps/filmcraft-android/ci-smoke.sh <apk dir> <out dir>
set -euo pipefail
apk_dir="$1"
out="$2"
pkg=ai.storyteller.filmcraft
mkdir -p "$out"
apks=("$apk_dir"/*.apk)
[ -f "${apks[0]}" ] || { echo "no APK in $apk_dir" >&2; exit 1; }
apk="${apks[0]}"
echo "installing $apk"
adb install -r -g "$apk"
adb logcat -c
adb shell am start -W -n "$pkg/.FilmcraftActivity"
# the demo project opens at the first start; give the emulator's software GPU time to draw it
for _ in $(seq 1 30); do
    sleep 2
    if adb logcat -d -s FilmCraft:I | grep -q "graphics:"; then
        break
    fi
done
sleep 20
adb exec-out screencap -p > "$out/screenshot.png"
adb logcat -d > "$out/logcat.txt"
echo "----- FilmCraft log -----"
grep -E " FilmCraft *:" "$out/logcat.txt" | tail -n 80 || true
status=0
if ! adb shell pidof "$pkg" >/dev/null; then
    echo "FilmCraft is not running" >&2
    status=1
fi
if grep -E "FilmCraft.*panic:|Fatal signal|FATAL EXCEPTION.*$pkg|>>> $pkg <<<" "$out/logcat.txt"; then
    echo "FilmCraft crashed (see logcat above)" >&2
    status=1
fi
exit "$status"

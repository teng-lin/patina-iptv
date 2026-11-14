#!/usr/bin/env bash
# Smoke test of the Android app on an emulator or device, driven by remote
# keys and checked against the app's own log lines (synthetic data only):
# first run, add the synthetic playlist, play a channel in the guide, full
# screen, multiview with a second player, background and resume (the players
# stop and reopen), and back to the guide. Fails on a crash, on a load the
# player refused outside its playback relay, or on a step that never happens.
#
#   tools/ci/android-smoke.sh [serial] [apk]
#
# The APK must be a debug build of a library made with the development
# playlist, which also needs debug assertions for private fixture addresses:
#
#   CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true ABIS=x86_64 \
#     IPTV_RS_DEV_PLAYLIST=http://10.0.2.2:8089/iptv/playlist.m3u tools/sync-host.sh
#   (cd hosts/android && ./gradlew assembleDebug)
#
# and the mock server must serve the fixtures on this machine's port 8089
# (IPTV_SMOKE_PORT), which the emulator reaches as 10.0.2.2:
#
#   tools/make-hls-fixtures.sh && python3 -I tools/make-iptv-fixtures.py --channels 300
#   python3 tools/mock-server.py 8089 fixtures/hls &
#
# The app's data on the device is replaced: it is uninstalled first when an
# APK is given, and cleared otherwise. Logs go to $IPTV_SMOKE_OUT (default
# target/android-smoke): the app's log, logcat's crash buffer and a screenshot
# at each step.
set -euo pipefail
export PATH="${ANDROID_HOME:-$HOME/Android/Sdk}/platform-tools:$PATH"
serial=${1:-${ANDROID_SERIAL:-}}
apk=${2:-}
port=${IPTV_SMOKE_PORT:-8089}
out=${IPTV_SMOKE_OUT:-target/android-smoke}
adb="adb${serial:+ -s $serial}"
pkg=com.hantekllc.iptvrs
act=$pkg/.host.TvActivity
mkdir -p "$out"
rm -f "$out"/*.png "$out"/*.log

pause() { timeout "$1" tail -f /dev/null || true; }
fail() {
  echo "FAIL: $*" >&2
  $adb exec-out screencap -p > "$out/failure.png" 2>/dev/null || true
  mark || true
  echo "last app log lines:" >&2
  tail -20 "$out/app.log" >&2 || true
  exit 1
}
# Pipes into grep read to the end (no -q): with pipefail, grep quitting at
# the first match can fail the producer, which would read as no match.
# The app's log since the last mark (`nsh` tag: the Rust app and the host).
applog() { $adb logcat -d -s nsh:I | tr -d '\r'; }
# Keeps the logs so far ($out) and starts afresh.
mark() {
  applog >> "$out/app.log"
  $adb logcat -d -b crash >> "$out/crash.log"
  $adb logcat -b main -b system -b crash -c
}
# Waits up to $2 s for a line of the app's log since the last mark matching $1.
expect() {
  local pattern=$1 limit=${2:-30} start=$SECONDS
  while ! applog | grep -E "$pattern" >/dev/null; do
    alive || fail "the app is not running (waiting for: $pattern)"
    [ $((SECONDS - start)) -lt "$limit" ] || fail "no '$pattern' within ${limit}s"
    pause 0.5
  done
  echo "  ok: $(applog | grep -E -m1 "$pattern" | sed 's/^.*: iptv/iptv/')"
}
alive() { [ -n "$($adb shell pidof $pkg | tr -d '\r')" ]; }
# Sends remote keys (one adb call: menus that hide after a moment stay up).
keys() { $adb shell input keyevent "$@"; }
shot() { $adb exec-out screencap -p > "$out/$1.png" || true; }
step() { echo "== $*"; }

[ "$(curl -sf --max-time 10 "http://127.0.0.1:$port/iptv/playlist.m3u" | head -c 7)" = "#EXTM3U" ] ||
  fail "no synthetic playlist at http://127.0.0.1:$port/iptv/playlist.m3u (see the usage)"
$adb wait-for-device
for _ in $(seq 1 120); do
  [ "$($adb shell getprop sys.boot_completed | tr -d '\r')" = 1 ] && break
  pause 2
done
# A freshly booted emulator image keeps optimizing and starting its own apps
# for a while; on a slow CI host, starting ours on top of that has made the
# emulator's system server die. Let it settle first (IPTV_SMOKE_SETTLE s).
pause "${IPTV_SMOKE_SETTLE:-0}"

step "install and first start"
if [ -n "$apk" ]; then
  $adb uninstall $pkg >/dev/null 2>&1 || true
  $adb install -r "$apk" >/dev/null
else
  $adb shell pm clear $pkg >/dev/null
fi
# No first-use hint over the immersive app, no picture in picture: Home
# must stop the activity (picture in picture only pauses it).
$adb shell settings put secure immersive_mode_confirmations confirmed
$adb shell appops set $pkg PICTURE_IN_PICTURE ignore
$adb shell input keyevent KEYCODE_WAKEUP
$adb shell wm dismiss-keyguard >/dev/null 2>&1 || true
$adb shell am force-stop $pkg
$adb logcat -b main -b system -b crash -c
$adb shell am start -W -n $act >/dev/null
expect 'iptv startup: fonts registered' 60
# The last start-up step: until then a slow emulator may drop keys.
expect 'iptv pip: supported' 60
pause 3
shot 01-first-run

step "add the synthetic playlist"
mark
# Add playlist, the first-run screen's only button. A slow emulator can
# still miss the first keys while the screen settles; pressing again is
# harmless there.
for attempt in 1 2 3 4; do
  keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER
  for _ in $(seq 1 10); do
    applog | grep -E 'key=ok .*ctx=Menu' >/dev/null && break 2
    pause 0.5
  done
  echo "  no response to Add playlist (attempt $attempt)"
done
expect 'key=ok .*ctx=Menu' 10
pause 2
keys KEYCODE_DPAD_CENTER                            # Playlist link (prefilled)
pause 2
shot 02-playlist-link
mark
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER
expect 'iptv wizard: import' 10
expect 'iptv data: guide switched to [0-9]+ imported channels' 180
pause 2
shot 03-playlist-added
keys KEYCODE_DPAD_CENTER                            # Watch
expect 'iptv wizard: closed' 10
pause 2

step "play a channel in the guide"
mark
keys KEYCODE_DPAD_CENTER
expect 'key=ok .*ctx=Guide' 10
expect 'iptv player 0 state: ready' 60
shot 04-guide-preview

step "full screen"
mark
keys KEYCODE_DPAD_CENTER
pause 3
shot 05-fullscreen

step "multiview with a second player"
# The control strip: Search, Channels, Recordings, Multiview. Complete the
# selection in one adb call: a screenshot on a loaded emulator can take
# longer than the strip's auto-hide timer.
keys KEYCODE_MENU KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT KEYCODE_DPAD_CENTER
expect 'key=menu .*mode=Fullscreen' 10
expect 'iptv multiview: open' 10
pause 2
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER          # another channel for tile 2
expect 'iptv multiview: surface 1 plays' 10
expect 'iptv player 1 state: ready' 60
pause 2
shot 06-multiview

step "background and resume"
mark
keys KEYCODE_HOME
for _ in $(seq 1 20); do
  $adb shell dumpsys activity -p $pkg activities | grep 'state=STOPPED' >/dev/null && break
  pause 0.5
done
$adb shell dumpsys activity -p $pkg activities | grep 'state=STOPPED' >/dev/null ||
  fail "the activity did not stop in the background"
echo "  ok: activity stopped"
pause 3
alive || fail "the app died in the background"
mark
$adb shell am start -W -n $act >/dev/null
expect 'iptv player 0 state: ready' 60
expect 'iptv player 1 state: ready' 60
shot 07-resumed

step "back to the guide"
mark
keys KEYCODE_BACK                                   # leaves multiview
expect 'iptv multiview: leaving to surface' 10
expect 'iptv player 0 state: ready' 30
# Back leaves full screen (a first one may only hide the channel banner;
# a TV keeps the Guide key for itself).
for _ in 1 2 3 4; do
  keys KEYCODE_BACK
  pause 2
  applog | grep 'iptv panels hidden: left full screen' >/dev/null && break
done
expect 'iptv panels hidden: left full screen' 1
keys KEYCODE_DPAD_DOWN
expect 'key=down .*ctx=Guide' 10
shot 08-guide

step "checks"
pause 2
alive || fail "the app is not running"
mark
if grep -q "$pkg" "$out/crash.log"; then fail "the app crashed (crash.log)"; fi
# The relay guard's code (RelayGuard.kt): a player load outside its relay.
if grep -q 'ERROR_CODE_UNSUPPORTED_SCHEME' "$out/app.log"; then fail "a player load was refused"; fi
echo "  ok: no crash, no refused load"
echo "  player errors (the app retries them): $(grep -c 'iptv player [0-9] error' "$out/app.log" || true)"
$adb shell appops reset $pkg
echo "PASS (logs and screenshots in $out)"

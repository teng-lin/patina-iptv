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

pause() { sleep "$1"; }
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
wait_for_boot() {
  for _ in $(seq 1 60); do
    [ "$($adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && return 0
    pause 2
  done
  return 1
}

$adb wait-for-device
if ! wait_for_boot; then
  echo "Emulator failed to finish boot; rebooting and retrying once..." >&2
  $adb reboot || true
  $adb wait-for-device
  wait_for_boot || fail "emulator failed to complete boot after retry"
fi
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
# Restore only the device settings this smoke changes, including failure paths.
restore_setting() {
  if [ "$3" = null ] || [ -z "$3" ]; then
    $adb shell settings delete "$1" "$2" >/dev/null
  else
    $adb shell settings put "$1" "$2" "$3" >/dev/null
  fi
}
saved_finish=$($adb shell settings get global always_finish_activities | tr -d '\r')
saved_font=$($adb shell settings get system font_scale | tr -d '\r')
saved_immersive=$($adb shell settings get secure immersive_mode_confirmations | tr -d '\r')
saved_pip=$($adb shell appops get "$pkg" PICTURE_IN_PICTURE | sed -n 's/.*PICTURE_IN_PICTURE: \([^; ]*\).*/\1/p' | tr -d '\r')
saved_pip=${saved_pip:-default}
cleanup() {
  local status=$?
  trap - EXIT
  set +e
  restore_setting global always_finish_activities "$saved_finish"
  restore_setting system font_scale "$saved_font"
  restore_setting secure immersive_mode_confirmations "$saved_immersive"
  $adb shell appops set "$pkg" PICTURE_IN_PICTURE "$saved_pip" >/dev/null
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# Ensure background/resume runs with ordinary activity lifetime first.
$adb shell settings put global always_finish_activities 0
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
# The control strip: Search, Channels, Recordings, Multiview.
keys KEYCODE_MENU KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT
expect 'key=menu .*mode=Fullscreen' 10
shot 05-fullscreen-controls

step "multiview with a second player"
keys KEYCODE_DPAD_CENTER
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

# The fixture starts from a clean installation and contains one playlist.
# Its resume file stores content identity, not a list index. Keep identities
# out of logs even though this script uses only generated input.
resume_file=
for _ in $(seq 1 60); do
  resume_file=$($adb shell "run-as $pkg ls files" | tr -d '\r' | sed -n '/^last_channel-[0-9][0-9]*$/p' || true)
  [[ "$resume_file" =~ ^last_channel-[0-9]+$ ]] && break
  pause 0.5
done
[[ "$resume_file" =~ ^last_channel-[0-9]+$ ]] || fail "expected one persisted playlist resume file"
source_id=${resume_file#last_channel-}
resume_identity() { $adb exec-out run-as "$pkg" cat "files/$resume_file" 2>/dev/null | tr -d '\r\n'; }
assert_resume() {
  local expected=$1 label=$2 actual
  actual=$(resume_identity) || fail "$label: cannot read persisted resume identity"
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    fail "$label: persisted channel changed"
  fi
}
wait_stopped() {
  for _ in $(seq 1 40); do
    $adb shell dumpsys activity -p "$pkg" activities | grep -E 'state=STOPPED|mAppStopped=true' >/dev/null && return 0
    pause 0.5
  done
  fail "activity did not stop and flush its queued writes"
}
wait_destroyed() {
  local activities
  for _ in $(seq 1 60); do
    activities=$($adb shell dumpsys activity -p "$pkg" activities) ||
      fail "cannot inspect activity destruction"
    if ! printf '%s\n' "$activities" | grep -E "ActivityRecord.*${pkg//./\\.}" >/dev/null; then
      return 0
    fi
    pause 0.5
  done
  fail "activity was not destroyed with don't-keep-activities enabled"
}
reopen_and_expect_resume() {
  $adb shell am start -W -n "$act" >/dev/null
  expect 'iptv startup: activity created' 60
  expect 'iptv pip: supported' 60
  expect 'iptv player: resuming the last channel' 60
  expect 'iptv player 0 state: ready' 60
  expect 'iptv (data: guide switched to|startup: guide shown)' 60
}

step "refresh during playback"
before_refresh=$(resume_identity)
[ -n "$before_refresh" ] || fail "no persisted channel before refresh"
mark
# From the confirmed Guide pane: Tab -> Groups, Left -> rail/Search.
# Down clamps at Settings; Up is Playlists even when VOD rows are present.
keys KEYCODE_TAB KEYCODE_DPAD_LEFT
expect 'key=left .*ctx=Groups' 10
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN \
  KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN
keys KEYCODE_DPAD_UP KEYCODE_DPAD_CENTER
pause 1
# The shown playlist is focused. Its first action is Show, then Refresh.
keys KEYCODE_DPAD_CENTER KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER
expect "iptv playlists: refresh $source_id([^0-9]|$)" 10
expect 'iptv data: refreshed ImportCounts' 180
assert_resume "$before_refresh" "playback refresh"
# A refresh keeps the persistent player running; it need not emit ready again.
if applog | grep -E 'iptv player 0 (open:|error)' >/dev/null; then
  fail "refresh reopened or failed the playing stream"
fi
keys KEYCODE_BACK KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT
pause 1
shot 09-refreshed

step "kill and reopen after a change"
mark
# Focus movement alone is not persisted: select a different channel to play.
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER
expect 'iptv player 0 open:' 30
expect 'iptv player 0 state: ready' 60
changed_identity=
for _ in $(seq 1 60); do
  changed_identity=$(resume_identity)
  [ -n "$changed_identity" ] && [ "$changed_identity" != "$before_refresh" ] && break
  pause 0.5
done
if [ -z "$changed_identity" ] || [ "$changed_identity" = "$before_refresh" ]; then
  fail "playing another channel did not persist its identity"
fi
keys KEYCODE_HOME
wait_stopped
assert_resume "$changed_identity" "background flush"
$adb shell am force-stop "$pkg"
! alive || fail "app failed to stop on kill"
mark
reopen_and_expect_resume
assert_resume "$changed_identity" "process restart"
shot 10-reopened

step "recreate the activity"
orig_pid=$($adb shell pidof "$pkg" | tr -d '\r')
[ -n "$orig_pid" ] || fail "app is not running before activity recreation"
# fontScale/orientation are handled by configChanges and do not recreate this
# Activity. Don't-keep-activities destroys it on background, retaining its process.
$adb shell settings put global always_finish_activities 1
keys KEYCODE_HOME
wait_destroyed
[ "$orig_pid" = "$($adb shell pidof "$pkg" | tr -d '\r')" ] || fail "process died before recreation"
mark
reopen_and_expect_resume
[ "$orig_pid" = "$($adb shell pidof "$pkg" | tr -d '\r')" ] || fail "recreation replaced the process"
assert_resume "$changed_identity" "activity recreation"
# Keep ordinary lifecycle for the final recovery case; the trap restores the original.
$adb shell settings put global always_finish_activities 0
shot 11-recreated

step "interrupt then recover a restore"
keys KEYCODE_HOME
wait_stopped
assert_resume "$changed_identity" "before interrupted restore"
$adb shell am force-stop "$pkg"
! alive || fail "app failed to stop before staged restore"
# A crash after moving current storage aside, before the new tree is
# installed: activation must recover the previous tree, even if Android has
# recreated the empty files directory. Compare content identity, not file size.
$adb shell "run-as $pkg mv files files.restore-previous"
$adb shell "run-as $pkg mkdir files"
mark
reopen_and_expect_resume
assert_resume "$changed_identity" "interrupted restore recovery"
$adb shell "run-as $pkg test ! -d files.restore-previous" || fail "restore-previous was not cleaned up"
shot 12-recovered

step "checks"
pause 2
alive || fail "the app is not running"
mark
if grep -q "$pkg" "$out/crash.log"; then fail "the app crashed (crash.log)"; fi
# The relay guard's code (RelayGuard.kt): a player load outside its relay.
if grep -q 'ERROR_CODE_UNSUPPORTED_SCHEME' "$out/app.log"; then fail "a player load was refused"; fi
echo "  ok: no crash, no refused load"
echo "  player errors (the app retries them): $(grep -c 'iptv player [0-9] error' "$out/app.log" || true)"
echo "PASS (logs and screenshots in $out)"

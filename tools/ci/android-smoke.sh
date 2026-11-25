#!/usr/bin/env bash
# Smoke test of the Android app on an owned disposable emulator, driven by remote
# keys and checked against the app's own log lines (synthetic data only):
# first run, add the synthetic playlist, confirm favorites edits with remote
# and touch, play a channel in the guide, full screen, multiview with a second
# player, background and resume (the players stop and reopen), and back to
# the guide. Fails on a crash, on a load the
# player refused outside its playback relay, or on a step that never happens.
#
#   tools/ci/disposable-avd.py ~/.android/avd/tab36.avd <validation-debug.apk>
#
# Requires a JDK with javac (Java 11 target) and Android SDK build-tools d8.
# The APK must be a debug build of a library made with the development
# playlist, which also needs debug assertions for private fixture addresses:
#
#   IPTV_VALIDATION_APP_ID=com.hantekllc.iptvrs.validation \
#     CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true ABIS=x86_64 \
#     IPTV_RS_DEV_PLAYLIST=http://10.0.2.2:8089/iptv/playlist.m3u tools/sync-host.sh
#   (cd hosts/android && IPTV_VALIDATION_APP_ID=com.hantekllc.iptvrs.validation ./gradlew assembleDebug)
#
# and the mock server must serve the fixtures on this machine's port 8089
# (IPTV_SMOKE_PORT), which the emulator reaches as 10.0.2.2:
#
#   tools/make-hls-fixtures.sh --android-smoke
#   python3 -I tools/make-iptv-fixtures.py --channels 300
#   python3 tools/mock-server.py 8089 fixtures/hls &
#
# The validation app is uninstalled first when an APK is given. Otherwise its
# data is cleared. Logs go to $IPTV_SMOKE_OUT (default
# target/android-smoke): the app's log, logcat's crash buffer and a screenshot
# at each step.
set -euo pipefail
export PATH="${ANDROID_HOME:-$HOME/Android/Sdk}/platform-tools:$PATH"
serial=${1:-${ANDROID_SERIAL:-}}
if [[ ! "$serial" =~ ^emulator-[0-9]+$ ]]; then
  echo 'Smoke requires an explicit disposable emulator serial; use disposable-avd.py.' >&2
  exit 2
fi
apk=${2:-}
# Optional manual hold (see the end): whole seconds, bounded, checked before any
# device change. disposable-avd.py adds the same hold to the smoke's budget.
hold=${IPTV_SMOKE_HOLD:-}
if [ -n "$hold" ] && { [[ ! "$hold" =~ ^[1-9][0-9]{0,3}$ ]] || [ "$hold" -gt 1800 ]; }; then
  echo 'IPTV_SMOKE_HOLD must be whole seconds from 1 through 1800.' >&2
  exit 2
fi
port=${IPTV_SMOKE_PORT:-8089}
out=${IPTV_SMOKE_OUT:-target/android-smoke}
adb_guard=$(cd "$(dirname "$0")" && pwd)/android/adb.py
ui_probe=$(cd "$(dirname "$0")" && pwd)/android/ui-snapshot.py
phase="device boot"
# Preserve timeout diagnostics even when an expected adb error is suppressed.
exec 3>&2
adb() {
  local limit=15
  case "$1" in
    install|push|wait-for-device) limit=120 ;;
    reboot) limit=30 ;;
    shell)
      case "${2:-} ${3:-}" in
        "am start") limit=60 ;;
        "uiautomator dump") limit=30 ;;
      esac ;;
  esac
  case "$phase" in "failure diagnostics"|cleanup) limit=5 ;; esac
  python3 "$adb_guard" --timeout "$limit" --phase "$phase" --category "$1" \
    --diagnostic-fd 3 -- adb -s "$serial" "$@"
}
expected_avd=${IPTV_SMOKE_DISPOSABLE_AVD:-}
if [[ ! "$expected_avd" =~ ^patina-validation-[a-zA-Z0-9_-]+$ ]]; then
  echo 'Smoke requires an owned disposable AVD name from disposable-avd.py or CI.' >&2
  exit 2
fi
actual_avd=$(adb emu avd name | tr -d '\r' | head -n 1)
if [ "$actual_avd" != "$expected_avd" ]; then
  echo 'Refusing device actions: serial does not identify the owned disposable AVD.' >&2
  exit 2
fi
pkg=com.hantekllc.iptvrs.validation
if [ -n "$apk" ]; then
  analyzer=${ANDROID_HOME:-$HOME/Android/Sdk}/cmdline-tools/latest/bin/apkanalyzer
  if [ "$("$analyzer" manifest application-id "$apk")" != "$pkg" ]; then
    echo 'Refusing to install an APK without the dedicated validation application ID.' >&2
    exit 2
  fi
fi
mkdir -p "$out"
rm -f "$out"/*.png "$out"/*.log

pause() { sleep "$1"; }
fail() {
  phase="failure diagnostics"
  echo "FAIL: $*" >&2
  adb exec-out screencap -p > "$out/failure.png" 2>/dev/null || true
  adb shell dumpsys activity -p "$pkg" activities > "$out/activities-at-failure.log" 2>/dev/null || true
  mark || true
  echo "last app log lines:" >&2
  tail -20 "$out/app.log" >&2 || true
  exit 1
}
# Pipes into grep read to the end (no -q): with pipefail, grep quitting at
# the first match can fail the producer, which would read as no match.
# The app's log since the last mark (`nsh` tag: the Rust app and the host).
applog() { adb logcat -d -s nsh:I | tr -d '\r'; }
# Keeps the logs so far ($out) and starts afresh.
mark() {
  applog >> "$out/app.log"
  # Dedicated fresh validation profile and synthetic media only. Preserve codec
  # causes before clearing buffers; the app's redacted error code alone cannot
  # distinguish an emulator renderer failure from a playback regression.
  adb logcat -d -s MediaCodec:D CCodec:D Codec2Client:D ExoPlayerImplInternal:E '*:S' >> "$out/codec.log"
  adb logcat -d -s ActivityTaskManager:I ActivityManager:I WindowManager:I '*:S' >> "$out/lifecycle.log"
  adb logcat -b events -d -s wm_create_activity:I wm_destroy_activity:I wm_relaunch_resume_activity:I \
    wm_restart_activity:I wm_resume_activity:I wm_on_destroy_called:I wm_on_create_called:I \
    am_kill:I am_low_memory:I '*:S' > "$out/lifecycle-events.log"
  adb logcat -d -b crash >> "$out/crash.log"
  adb logcat -b main -b system -b crash -c
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
alive() { [ -n "$(adb shell pidof $pkg | tr -d '\r')" ]; }
# Queue each short Down/Up pair without waiting for app handling between them.
# State/log acknowledgements below still wait for the requested app behavior.
# Deliberate long presses use the same helper with an explicit hold below.
keys() {
  local key
  for key in "$@"; do
    [[ $key =~ ^KEYCODE_[A-Z0-9_]+$ ]] || fail "invalid smoke key: $key"
  done
  adb shell "CLASSPATH=$probe app_process /system/bin ShortKeys $*"
}
shot() { adb exec-out screencap -p > "$out/$1.png" || true; }
step() { phase=$*; echo "== $*"; }

wait_stopped() {
  local attempts=${1:-40} activities=
  for _ in $(seq 1 "$attempts"); do
    if ! activities=$(adb shell dumpsys activity -p "$pkg" activities); then
      printf '%s\n' "$activities" > "$out/activities.log"
      fail "cannot inspect activity shutdown"
    fi
    # ActivityManager emits compact and full state fields on different images.
    # Keep the successful observation: a second dump can race a state change.
    if printf '%s\n' "$activities" | grep -E \
      '(^|[[:space:]{])(state|mState)=STOPPED([[:space:],}]|$)|(^|[[:space:]{])mAppStopped=true([[:space:],}]|$)' >/dev/null; then
      return 0
    fi
    pause 0.5
  done
  printf '%s\n' "$activities" > "$out/activities.log"
  fail "activity did not stop and flush its queued writes"
}

[ "$(curl -sf --max-time 10 "http://127.0.0.1:$port/iptv/playlist.m3u" | head -c 7)" = "#EXTM3U" ] ||
  fail "no synthetic playlist at http://127.0.0.1:$port/iptv/playlist.m3u (see the usage)"
wait_for_boot() {
  for _ in $(seq 1 60); do
    [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && return 0
    pause 2
  done
  return 1
}

adb wait-for-device
if ! wait_for_boot; then
  echo "Emulator failed to finish boot; rebooting and retrying once..." >&2
  adb reboot || true
  adb wait-for-device
  wait_for_boot || fail "emulator failed to complete boot after retry"
fi
# A freshly booted emulator image keeps optimizing and starting its own apps
# for a while; on a slow CI host, starting ours on top of that has made the
# emulator's system server die. Let it settle first (IPTV_SMOKE_SETTLE s).
pause "${IPTV_SMOKE_SETTLE:-0}"

# The developer option also updates ActivityManager's in-memory flag.
# A settings write alone cannot exercise activity destruction.
command -v javac >/dev/null || fail "javac is required for the lifecycle probe"
mkdir -p "$out/always-finish"
d8=
for candidate in "${ANDROID_HOME:-$HOME/Android/Sdk}"/build-tools/*/d8; do
  [ ! -x "$candidate" ] || d8=$candidate
done
[ -n "$d8" ] || fail "Android d8 is required for the lifecycle probe"
javac --release 11 -d "$out/always-finish" \
  "$(dirname "$0")/android/AlwaysFinish.java" "$(dirname "$0")/android/ShortKeys.java"
"$d8" --output "$out/always-finish" \
  "$out/always-finish/AlwaysFinish.class" "$out/always-finish/ShortKeys.class"
probe=/data/local/tmp/patina-smoke-always-finish.dex
set_always_finish() {
  adb shell "CLASSPATH=$probe app_process /system/bin AlwaysFinish $1"
}

step "install and first start"
if [ -n "$apk" ]; then
  adb uninstall $pkg >/dev/null 2>&1 || true
  adb install -r "$apk" >/dev/null
else
  adb shell pm clear $pkg >/dev/null
fi
# This fixture imports exactly one generated playlist into a fresh profile.
# Its two-player journey needs an explicit allowance for that first source;
# leave the product's conservative default and every other source unchanged.
adb shell "run-as $pkg test ! -e files/iptv.sqlite" || fail "expected a fresh synthetic profile"
adb shell "run-as $pkg mkdir -p files"
adb shell "run-as $pkg sh -c 'cat > files/settings.json'" <<'SMOKE_SETTINGS'
{
  "format": "iptv-rs.settings", "version": 2,
  "global": {}, "groups": {},
  "playlists": {"1": {"playlists.max_connections": 2}}
}
SMOKE_SETTINGS
# Restore only the device settings this smoke changes, including failure paths.
restore_setting() {
  if [ "$3" = null ] || [ -z "$3" ]; then
    adb shell settings delete "$1" "$2" >/dev/null
  else
    adb shell settings put "$1" "$2" "$3" >/dev/null
  fi
}
saved_finish=$(adb shell settings get global always_finish_activities | tr -d '\r')
saved_font=$(adb shell settings get system font_scale | tr -d '\r')
saved_immersive=$(adb shell settings get secure immersive_mode_confirmations | tr -d '\r')
saved_pip=$(adb shell appops get "$pkg" PICTURE_IN_PICTURE | sed -n 's/.*PICTURE_IN_PICTURE: \([^; ]*\).*/\1/p' | tr -d '\r')
saved_pip=${saved_pip:-default}
cleanup() {
  local status=$?
  trap - EXIT
  phase=cleanup
  set +e
  # Release the fixture app's decoders before the emulator action tears down.
  adb shell am force-stop "$pkg" >/dev/null
  if [ "$saved_finish" = 1 ]; then set_always_finish true; else set_always_finish false; fi
  restore_setting global always_finish_activities "$saved_finish"
  adb shell rm -f "$probe" /data/local/tmp/patina-smoke-ui.xml >/dev/null
  restore_setting system font_scale "$saved_font"
  restore_setting secure immersive_mode_confirmations "$saved_immersive"
  adb shell appops set "$pkg" PICTURE_IN_PICTURE "$saved_pip" >/dev/null
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
adb push "$out/always-finish/classes.dex" "$probe" >/dev/null
# Ensure background/resume runs with ordinary activity lifetime first.
set_always_finish false
# No first-use hint over the immersive app, no picture in picture: Home
# must stop the activity (picture in picture only pauses it).
adb shell settings put secure immersive_mode_confirmations confirmed
adb shell appops set $pkg PICTURE_IN_PICTURE ignore
adb shell input keyevent KEYCODE_WAKEUP
adb shell wm dismiss-keyguard >/dev/null 2>&1 || true
adb shell am force-stop $pkg
adb logcat -b main -b system -b crash -c
# pm returns a nonzero status for a valid false result. Validate its output
# so tablets proceed while an adb/query failure still fails the smoke.
is_tv=$(adb shell pm has-feature android.software.leanback | tr -d '\r' || true)
case "$is_tv" in
  true|false) ;;
  *) fail "could not determine the Android TV feature" ;;
esac
category=android.intent.category.LAUNCHER
[ "$is_tv" != true ] || category=android.intent.category.LEANBACK_LAUNCHER
act=$(adb shell cmd package resolve-activity --brief -a android.intent.action.MAIN -c "$category" -p "$pkg" | tr -d '\r' | tail -n 1)
[[ "$act" =~ ^com\.hantekllc\.iptvrs\.validation/[A-Za-z0-9_.$]+$ ]] || fail "cannot resolve the validation launcher"
adb shell am start -W -n "$act" -a android.intent.action.MAIN -c "$category" >/dev/null
expect 'iptv startup: fonts registered' 60
# The last start-up step: until then a slow emulator may drop keys.
expect 'iptv pip: supported' 60
pause 3
shot 01-first-run

step "add the synthetic playlist"
mark
# Add a playlist has initial focus on welcome. A slow emulator may
# still miss the first OK while the screen settles.
for attempt in 1 2 3 4; do
  keys KEYCODE_DPAD_CENTER
  for _ in $(seq 1 10); do
    applog | grep -E 'key=ok .*ctx=Editor' >/dev/null && break 2
    pause 0.5
  done
  echo "  no response to Add playlist (attempt $attempt)"
done
expect 'key=ok .*ctx=Editor' 10
pause 2
shot 02-playlist-link
mark
# The development link is prefilled and focused. Move through Paste and
# the optional details control to Add playlist.
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER
expect 'iptv wizard: import' 10
expect 'iptv data: guide switched to [0-9]+ imported channels' 180
pause 2
shot 03-playlist-added
keys KEYCODE_DPAD_CENTER                            # Watch
expect 'iptv wizard: closed' 10
pause 2

# Observe the real Android accessibility tree; no database keys or test-only
# app hooks are needed. Before playback, the screen can settle between dumps.
ui_xml=$out/favorites-ui.xml
device_ui=/data/local/tmp/patina-smoke-ui.xml
ui_dump() {
  adb shell rm -f "$device_ui" || fail "cannot reset native UI snapshot"
  adb shell uiautomator dump --compressed "$device_ui" >/dev/null || fail "cannot dump native favorites UI"
  adb exec-out cat "$device_ui" > "$ui_xml" || fail "cannot read native favorites UI"
}
ui_count() { python3 "$ui_probe" "$ui_xml" count; }
ui_confirmation() { python3 "$ui_probe" "$ui_xml" confirmation; }
ui_tap() {
  local coordinates tap_x tap_y
  coordinates=$(python3 "$ui_probe" "$ui_xml" tap "$1") || fail "cannot locate native favorites button: $1"
  read -r tap_x tap_y <<< "$coordinates"
  adb shell input tap "$tap_x" "$tap_y"
}
# A queued native input is not an acknowledgement of the Rust state change.
# Retry observations only: replaying a key/tap could toggle or move twice.
ui_wait() {
  local description=$1 attempt deadline=$((SECONDS + 30))
  shift
  for attempt in $(seq 1 6); do
    ui_dump
    if "$@"; then return 0; fi
    [ "$SECONDS" -lt "$deadline" ] || break
    pause 0.2
  done
  fail "native UI did not settle: $description"
}
ui_count_matches() {
  local counts
  ! ui_confirmation || return 1
  counts=$(ui_count 2>/dev/null) || return 1
  [ "$counts" = "$1 $favorites_total" ]
}
ui_initial_editor() {
  local counts
  ! ui_confirmation || return 1
  counts=$(ui_count 2>/dev/null) || return 1
  [ "${counts%% *}" = 0 ] && [ "${counts#* }" -gt 0 ] || return 1
  favorites_total=${counts#* }
}
ui_selected_changed() {
  local observed
  observed=$(python3 "$ui_probe" "$ui_xml" selected-label 2>/dev/null) || return 1
  [ "$observed" != "$1" ] || return 1
  menu_selection=$observed
}
ui_editor_closed() { python3 "$ui_probe" "$ui_xml" editor-closed; }
assert_favorites() {
  ui_wait "favorites count after $2" ui_count_matches "$1"
  cp "$ui_xml" "$out/favorites-${2// /-}.xml"
  echo "  ok: native favorites $1/$favorites_total after $2"
}
request_favorites_clear() {
  keys KEYCODE_DPAD_CENTER
  ui_wait 'remote Remove all confirmation' ui_confirmation
}

step "confirm bulk favorites removal with remote and touch"
mark
keys KEYCODE_MENU
# Menu composition depends on the current program. Follow its actual selected
# accessibility label rather than assuming an action index. Every wait is
# bounded, including adb's UI-automation idle wait.
ui_wait 'initial context menu selection' python3 "$ui_probe" "$ui_xml" selected Search
menu_selection=Search
menu_found=false
for _ in $(seq 1 30); do
  if python3 "$ui_probe" "$ui_xml" selected 'Manage favorites…'; then
    menu_found=true
    break
  fi
  previous_selection=$menu_selection
  keys KEYCODE_DPAD_DOWN
  ui_wait 'context menu selection after Down' ui_selected_changed "$previous_selection"
done
[ "$menu_found" = true ] || fail "cannot reach Manage favorites in native menu"
keys KEYCODE_DPAD_CENTER
ui_wait 'empty native favorites editor' ui_initial_editor
keys KEYCODE_DPAD_RIGHT
assert_favorites "$favorites_total" 'remote Add all'
keys KEYCODE_DPAD_LEFT
assert_favorites "$favorites_total" Left
# The first guide row owns this editor (the wizard has just closed).
keys KEYCODE_DPAD_UP KEYCODE_DPAD_RIGHT
request_favorites_clear
shot 03a-favorites-confirmation
keys KEYCODE_DPAD_CENTER                           # Cancel is focused by default.
assert_favorites "$favorites_total" 'default Cancel'
request_favorites_clear
keys KEYCODE_BACK                                  # Native Back cancels only the dialog.
assert_favorites "$favorites_total" Back
request_favorites_clear
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER
assert_favorites 0 'remote confirmation'
# Native touch targets come from the measured accessibility bounds, so tablet
# density, safe areas and TV dimensions do not hard-code tap coordinates.
# With no favorites, a leaked Add all touch would be visible after Cancel.
# Keep its coordinates from before the toolbar becomes inaccessible, and
# refuse a point that would instead activate either real dialog action.
cp "$ui_xml" "$out/favorites-before-covered-touch.xml"
ui_tap 'Remove all'
ui_wait 'covered-touch confirmation opening' ui_confirmation
covered_coordinates=$(python3 "$ui_probe" "$ui_xml" covered-tap 'Add all' \
  --before "$out/favorites-before-covered-touch.xml") || fail "cannot measure a covered toolbar touch"
read -r covered_x covered_y <<< "$covered_coordinates"
adb shell input tap "$covered_x" "$covered_y"
ui_wait 'confirmation retained after covered toolbar touch' ui_confirmation
ui_tap Cancel
assert_favorites 0 'blocked underlying Add all touch'
ui_tap 'Add all'
assert_favorites "$favorites_total" 'touch Add all'
ui_tap 'Remove all'
ui_wait 'touch Remove all confirmation opening' ui_confirmation
ui_tap Cancel
assert_favorites "$favorites_total" 'touch Cancel'
ui_tap 'Remove all'
ui_wait 'touch Remove all confirmation reopening' ui_confirmation
# The toolbar is hidden from accessibility while the modal is open: this is
# the dialog's explicit destructive action, not the button behind the scrim.
ui_tap 'Remove all'
assert_favorites 0 'touch confirmation'
keys KEYCODE_BACK                                  # Leave the editor on the original guide row.
ui_wait 'favorites editor closing' ui_editor_closed
adb shell rm -f "$device_ui"
shot 03b-favorites-cleared

step "play a channel in the guide"
mark
keys KEYCODE_DPAD_CENTER
expect 'key=ok .*ctx=Guide action=Some\(Select\).*mode=Guide' 10
expect 'iptv player 0 state: ready' 60
# Confirm the actual first-import identity before spending its fixture allowance.
resume_file=
for _ in $(seq 1 60); do
  resume_file=$(adb shell "run-as $pkg ls files" | tr -d '\r' | sed -n '/^last_channel-[0-9][0-9]*$/p' || true)
  [[ "$resume_file" =~ ^last_channel-[0-9]+$ ]] && break
  pause 0.5
done
[[ "$resume_file" =~ ^last_channel-[0-9]+$ ]] || fail "expected one persisted playlist resume file"
source_id=${resume_file#last_channel-}
[ "$source_id" = 1 ] || fail "expected synthetic first source 1 for the scoped stream allowance"
shot 04-guide-preview

step "full screen"
mark
keys KEYCODE_DPAD_CENTER
expect 'key=ok .*ctx=Guide action=Some\(Select\).*mode=Fullscreen' 10
pause 3
if applog | grep -F 'iptv player 0 open:' >/dev/null; then
  fail "opening the active channel full screen reopened the stream"
fi
shot 05-fullscreen

if [ "$is_tv" = true ]; then
  step "open shared TV controls with a tap"
  mark
  keys KEYCODE_DPAD_CENTER KEYCODE_DPAD_RIGHT KEYCODE_BACK
  expect 'key=ok .*phase=Up .*ctx=Player .*action=Some\(ShowControls\)' 10
  expect 'key=right .*ctx=Controls .*action=Some\(MoveRight\)' 10
  expect 'key=back .*ctx=Controls' 10

  step "shared TV controls and long OK release"
  mark
  # One second is deliberately above the 400 ms product default. Android's
  # input --longpress sits on that boundary and dispatch jitter can shorten it.
  adb shell "CLASSPATH=$probe app_process /system/bin ShortKeys --hold-ms 1000 KEYCODE_DPAD_CENTER"
  # Queue navigation before reading logcat: a loaded host can spend the
  # entire auto-hide interval polling or taking screenshots.
  keys KEYCODE_DPAD_RIGHT KEYCODE_BACK
  expect 'iptv (long press by timer: ShowControls|key=ok .*phase=(Repeat|Up) .*action=Some\(ShowControls\))' 10
  expect 'key=right .*ctx=Controls .*action=Some\(MoveRight\)' 10
  expect 'key=back .*ctx=Controls' 10
  pause 0.2
  if applog | grep -E 'key=ok .*phase=Up .*action=Some\((Select|ShowPanels)\)' >/dev/null; then
    fail "long OK also performed a short OK action on release"
  fi
  mark
fi

step "multiview with a second player"
if [ "$is_tv" = true ]; then
  # The live play bar starts on Play/pause. On the TV image, which has no
  # PiP, Favorite and Multiview are the next two actions to its right.
  keys KEYCODE_DPAD_CENTER \
    KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT KEYCODE_DPAD_CENTER
  expect 'iptv play bar: Multiview' 10
else
  # Tablet remote/keyboard Menu retains the existing advanced strip: Search,
  # Channels, then Multiview. A tablet shows Recordings only once its library
  # holds a recording (#275), and this fresh install has none.
  keys KEYCODE_MENU KEYCODE_DPAD_RIGHT KEYCODE_DPAD_RIGHT KEYCODE_DPAD_CENTER
  expect 'key=menu .*mode=Fullscreen' 10
fi

expect 'iptv multiview: open' 10
pause 2
keys KEYCODE_DPAD_DOWN KEYCODE_DPAD_CENTER          # another channel for tile 2
expect 'iptv multiview: surface 1 plays' 10
expect 'iptv player 1 state: ready' 60
pause 2
shot 06-multiview

step "background and resume"
adb shell dumpsys activity -p "$pkg" activities > "$out/activities-before-home.log"
mark
keys KEYCODE_HOME
wait_stopped 20
echo "  ok: activity stopped"
pause 3
alive || fail "the app died in the background"
adb shell dumpsys activity -p "$pkg" activities > "$out/activities-stopped.log"
mark
adb shell am start -W -n "$act" >/dev/null
adb shell dumpsys activity -p "$pkg" activities > "$out/activities-resumed.log"
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
resume_identity() { adb exec-out run-as "$pkg" cat "files/$resume_file" 2>/dev/null | tr -d '\r\n'; }
assert_resume() {
  local expected=$1 label=$2 actual
  actual=$(resume_identity) || fail "$label: cannot read persisted resume identity"
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    fail "$label: persisted channel changed"
  fi
}
wait_destroyed() {
  local activities
  for _ in $(seq 1 60); do
    activities=$(adb shell dumpsys activity -p "$pkg" activities) ||
      fail "cannot inspect activity destruction"
    if ! printf '%s\n' "$activities" | grep -E "ActivityRecord.*${pkg//./\\.}" >/dev/null ||
      printf '%s\n' "$activities" | grep -E 'state=DESTROYED' >/dev/null; then
      return 0
    fi
    pause 0.5
  done
  fail "activity was not destroyed with don't-keep-activities enabled"
}
reopen_and_expect_resume() {
  adb shell am start -W -n "$act" >/dev/null
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
# Closing the playlist screen runs as an app task. Later keys must wait
# for that effect, rather than landing on the screen being closed.
keys KEYCODE_BACK
expect 'iptv playlists: close' 10
# Let each movement settle into its pane before sending the next.
for _ in 1 2; do
  mark
  keys KEYCODE_DPAD_RIGHT
  expect 'key=right .*action=Some\(MoveRight\)' 10
done
pause 1
shot 09-refreshed

step "kill and reopen after a change"
mark
# Confirm the guide owns navigation before selecting another channel. A
# fixture EOF reconnect is not evidence that this selection happened.
keys KEYCODE_DPAD_DOWN
expect 'key=down .*ctx=Guide' 10
keys KEYCODE_DPAD_CENTER
expect 'key=ok .*ctx=Guide' 10
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
adb shell am force-stop "$pkg"
! alive || fail "app failed to stop on kill"
mark
reopen_and_expect_resume
assert_resume "$changed_identity" "process restart"
shot 10-reopened

step "recreate the activity"
orig_pid=$(adb shell pidof "$pkg" | tr -d '\r')
[ -n "$orig_pid" ] || fail "app is not running before activity recreation"
# fontScale/orientation are handled by configChanges and do not recreate this
# Activity. Don't-keep-activities destroys it on background, retaining its process.
set_always_finish true
keys KEYCODE_HOME
wait_destroyed
[ "$orig_pid" = "$(adb shell pidof "$pkg" | tr -d '\r')" ] || fail "process died before recreation"
mark
reopen_and_expect_resume
[ "$orig_pid" = "$(adb shell pidof "$pkg" | tr -d '\r')" ] || fail "recreation replaced the process"
assert_resume "$changed_identity" "activity recreation"
# Keep ordinary lifecycle for the final recovery case; the trap restores the original.
set_always_finish false
shot 11-recreated

step "interrupt then recover a restore"
keys KEYCODE_HOME
wait_stopped
assert_resume "$changed_identity" "before interrupted restore"
adb shell am force-stop "$pkg"
! alive || fail "app failed to stop before staged restore"
# A crash after moving current storage aside, before the new tree is
# installed: activation must recover the previous tree, even if Android has
# recreated the empty files directory. Compare content identity, not file size.
adb shell "run-as $pkg mv files files.restore-previous"
adb shell "run-as $pkg mkdir files"
mark
reopen_and_expect_resume
assert_resume "$changed_identity" "interrupted restore recovery"
adb shell "run-as $pkg test ! -d files.restore-previous" || fail "restore-previous was not cleaned up"
shot 12-recovered

step "hardware Search"
mark
keys KEYCODE_SEARCH
expect 'key=search .*action=Some\(OpenSearch\)' 10

# Optional manual journey checks on the same owned emulator (never in CI):
# hold up to IPTV_SMOKE_HOLD seconds, or until IPTV_SMOKE_HOLD_FILE is created
# (an earlier file is removed first), while an operator drives this serial.
# Logs are kept every 30 s so logcat cannot wrap past a crash or a refused
# load before the checks below.
if [ -n "$hold" ]; then
  step "hold for manual journey checks"
  [ -z "${IPTV_SMOKE_HOLD_FILE:-}" ] || rm -f "$IPTV_SMOKE_HOLD_FILE"
  echo "  holding $serial for up to ${hold}s"
  start=$SECONDS
  kept=$SECONDS
  while [ $((SECONDS - start)) -lt "$hold" ]; do
    [ -z "${IPTV_SMOKE_HOLD_FILE:-}" ] || [ ! -e "$IPTV_SMOKE_HOLD_FILE" ] || break
    alive || fail "the app died during the manual checks"
    if [ $((SECONDS - kept)) -ge 30 ]; then
      mark
      kept=$SECONDS
    fi
    pause 2
  done
fi

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

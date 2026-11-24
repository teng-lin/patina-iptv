# Accessibility journeys

Patina's automated checks inspect native names, roles, selection/checked state,
modal exposure, logical navigation, measured layout and player lifetime. They do
not establish that a platform screen reader speaks, moves or activates correctly.
Keep those results separate from the assistive-technology runs below.

## Repeatable matrix

Use generated playlists, XMLTV, provider catalogs and local mock streams only
([synthetic setup](../README.md#try-it-with-synthetic-data)). Test with the app's
Small, Medium, Large and Extra large text settings; repeat the largest setting
with system text at 1.3. Include long generated source/channel/program names
and a translated interface. Use 960×540 logical points for TV/small desktop and
1280×800 for tablet. A clipped program title may use ellipsis, but its accessible
name must retain the full title. Actions must remain visible or scroll into view.

| Journey | Expected behavior | Deterministic coverage |
| --- | --- | --- |
| First source setup | Source-type choices and fields have useful names; errors are readable; Back leaves the editor before the wizard | `screens_look`, `cov_settings_wizard`, `interface_languages` |
| Guide, groups and list changes | One selected content identity; D-pad follows the guide; sorting/reordering/refresh does not restore by old row number | `tablet_rows`, `guide_nav`, `personalization_order`, `accessibility_visibility` |
| Player and track chooser | Controls/choices expose roles and selected state; inactive controls and covered video are absent from accessibility | `tv_player`, `tv_replay`, `accessibility_visibility` |
| Parental PIN and recovery | Keypad, recovery and manager use their own focus ranges; Cancel remains visible; disabled controls cannot activate while checking; credential results remain private across a background-recording window hide and require a fresh PIN before redisplay | `parental`, `accessibility_visibility`, presentation `parental::dialog_tests` |
| VOD details, seasons and episodes | Catalog and episode selection expose content identity; Back and resume retain the intended item | `vod_library` |
| Nested modal dismissal | Covered controls cannot receive accessibility focus; Back exposes only the next layer and restores content selection | `accessibility_visibility` |
| Video contrast and lifetime | Opaque focus/text colors stay readable over black, white and bright video; UI transitions keep the same player and stream | theme and grid-cell unit tests; `accessibility_visibility` |

Run the affected suites while iterating:

```sh
cargo test -p iptv-core -p iptv-presentation parental
cargo test -p iptv-app --features test-support --lib
cargo test -p iptv-app --features test-support --test app accessibility_visibility
cargo test -p iptv-app --features test-support --test app parental
cargo test -p iptv-app --features test-support --test app vod_library
cargo test -p iptv-app --features test-support --test app recordings
```

The complete app suite in `tools/check.sh` includes these tests in Linux/macOS CI
and the Windows check. Before finishing also run the required core tests, portable
wasm build and native app build from [AGENTS.md](../AGENTS.md).

## Android TalkBack run

Use a disposable `tab36` or `tv36` emulator/profile, never an existing household
profile. Install the candidate APK and synthetic sources. Record Android version,
app commit, emulator versus hardware, resolution, text setting, system font scale,
input device, TalkBack version and enabled service. Package presence alone is not
an enabled screen-reader run.

1. Enable TalkBack in Android Accessibility settings. Verify it is active using
   `adb -s SERIAL shell dumpsys accessibility`; keep all adb commands scoped to
   that serial. For a disposable emulator only, the service can also be enabled
   with `settings put secure enabled_accessibility_services` using the exact
   installed service name, followed by `settings put secure accessibility_enabled 1`.
2. Run each matrix row using swipe navigation/double tap on tablet, then D-pad
   arrows/OK/Back on TV. Check spoken names, roles and current states; activate
   the controls through TalkBack, not just by screen coordinates. Record speech
   checks as unverified if nobody listened to the output.
3. Open each modal while video plays. Traverse all controls; hidden/covered
   controls must not be reachable. Close nested choosers/PIN/settings with Back
   and verify the origin channel/program rather than a row number.
4. Set Extra large and system font scale 1.3; repeat PIN/recovery, source setup,
   recording Save and episode selection. Reach every primary action by D-pad and
   touch, including after scrolling up again. Repeat over synthetic black/white
   and bright streams. Observe that playback does not restart on modal changes.
5. Record failures with synthetic labels, the exact transition and expected versus
   observed behavior. Restore the emulator settings or discard the profile.

Use `uiautomator dump --compressed` when checking modal exposure: the default
uncompressed dump intentionally includes nodes marked unimportant for accessibility.
For a reproducible native focus/activation check while TalkBack remains enabled,
build the [shell probe](../tools/ci/android/AccessibilityActionProbe.java):

```sh
IPTV_A11Y_OUT=/var/tmp/patina-accessibility
mkdir -p "$IPTV_A11Y_OUT"
javac --release 11 -cp "$ANDROID_HOME/platforms/android-36/android.jar" \
  -d "$IPTV_A11Y_OUT" tools/ci/android/AccessibilityActionProbe.java
"$ANDROID_HOME/build-tools/35.0.1/d8" --output "$IPTV_A11Y_OUT" \
  "$IPTV_A11Y_OUT/AccessibilityActionProbe.class"
adb -s emulator-5554 push "$IPTV_A11Y_OUT/classes.dex" /data/local/tmp/patina-accessibility.dex
adb -s emulator-5554 shell \
  "CLASSPATH=/data/local/tmp/patina-accessibility.dex app_process /system/bin AccessibilityActionProbe 'Add playlist'"
```

The label must name a control in a synthetic test profile. The probe requests
native accessibility focus and ACTION_CLICK without suppressing the active
screen-reader service. It is an Android shell test tool, not shipped app code;
check success against the resulting UI as well as its return value.

An accessibility tree dump checks native semantics, not speech output or touch
exploration. A headless `screen_reader=true` environment checks application
behavior only. Physical TV/tablet results must be identified separately from AVDs.

## Desktop keyboard and platform screen reader

On each available desktop, run setup, guide, player, settings, PIN, recordings and
VOD with keyboard only. Check Tab/Shift+Tab where supported, arrows/Enter, Escape
and Backspace without stealing text-editing keys. Resize to the small viewport,
change text size, and verify focus remains visible and actions remain reachable.
Repeat with the OS screen reader (VoiceOver on macOS, Narrator on Windows, Orca
on Linux where supported). Verify names/states, modal boundaries and activation.
Do not infer desktop screen-reader success from a passing Android or headless run.

## Evidence ledger

For each candidate, record the command/suite and result separately from manual
AT observations. Include skipped matrix rows and the reason. This document is a
procedure, not a blanket accessibility certification; untested host integrations
and unobserved speech remain open validation limits.

Candidate evidence for issue #68 (2026-11-16 UTC):

- `tab36`, Android 16/API 36, x86_64 emulator, 2560×1600 physical pixels
  (1280×800 logical points), D-pad keys plus native accessibility focus/click
  actions; TalkBack 16.0.0.738667889 enabled and bound throughout the actions.
  App Extra large and system font scale 1.3 were active.
- Imported a generated two-channel M3U through the native file chooser and
  activated Add playlist/Watch through accessibility actions. Opened the PIN
  modal, checked its named Cancel button and hidden guide underlay, then
  activated Cancel through accessibility. Opened a recording draft, moved the
  D-pad focus to Save, checked it was inside the scroll viewport and dismissed
  back to the guide. Source fields, submit and Back stayed within the window.
- Inspected compressed native trees rather than the uncompressed diagnostic
  hierarchy. The same checks are also deterministic headless scenarios; the
  player surface/stream-lifetime assertions are automated, not inferred from
  these native tree dumps.
- Limitations: speech output was not listened to; touch-exploration gestures,
  native VOD/episode playback, decoded bright/black/white video, physical devices
  and desktop platform screen readers remain unverified in this run. The
  repeatable matrix above identifies their follow-up checks. No general
  screen-reader or hardware certification is claimed.

Follow-up native checks of production commit `75588b4` used `tab36` at system
font scale 1.0 with APK SHA-256
`07c8e2de7fcfc25027425a67fed61daad680351229ecb6cf1ce7852b44bc9155`:

- With TalkBack enabled and bound, native focus/click actions activated setup
  choices, credential fields, Done and the qualified Paste button. Compressed
  trees showed separate `Paste · Username` / `Paste · Password` actions, masked
  credential values in field names, and `Server · Fill this in` after validation.
- With TalkBack disabled for direct native keyboard input, D-pad OK focused the
  recovery field. Enter after 32 synthetic characters advanced to a new, empty
  PIN rather than also entering a keypad digit. Back returned to the originating
  generated channel. This checks event routing, without validating the synthetic
  recovery credential or claiming a successful credential reset.
- Some repeated shell-probe processes reported a UiAutomation teardown failure
  after an action; these failed process exits were retained separately from
  successful action acknowledgements and resulting native trees. They were not
  counted as clean helper runs or application crashes. Speech, touch exploration,
  physical hardware and desktop AT limits above still apply.
